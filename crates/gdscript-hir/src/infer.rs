//! Gradual type inference (Playbook §3.3–§3.6 + §5): a single forward, bottom-up,
//! bidirectional walk over a lowered [`Body`]. No unification variables — types flow forward
//! from annotations, literals, and the engine API (rust-analyzer's *structure*, Pyright's
//! gradual *semantics*).
//!
//! The walk memoizes every expression's [`Ty`] in [`InferenceResult::expr_ty`] (the source of
//! hover + inlay), does flow-scoped `is`/`as` narrowing over the lexical guarded sub-tree, and
//! raises the §5 type diagnostics. The load-bearing invariant: a `Variant`/`Unknown`/`Error`
//! receiver is *uninformative* — it never fires `UNSAFE_*`, never cascades — so cross-file code
//! (which lands on `Unknown` via the seam) produces zero false diagnostics.

use gdscript_api::{EngineApi, MemberRef, TyRef};
use gdscript_base::{Diagnostic, DiagnosticSource, FileId, Severity, TextRange};
use gdscript_db::Db;
use gdscript_scene::{SceneModel, SceneNode};
use gdscript_syntax::GdNode;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use std::sync::Arc;

use crate::body::{self, BinOp, Body, Expr, ExprId, Literal, ParamBinding, Stmt, UnOp};
use crate::cst::{self, AstPtr};
use crate::flow::{self, FlowAnalysis, NarrowedTy, Place};
use crate::item_tree::{InnerClassItem, ItemTree, Member, has_annotation, item_tree};
use crate::resolve::{self, ClassItem, ClassScope, GlobalDef};
use crate::ty::{self, Assign, EnumRef, ScriptRefId, Ty};
use crate::warnings::{RawWarning, WarningCode};

// ---- diagnostic codes + message templates (Playbook §5, engine-matching) -----------------

/// `:=` / inferred binding from a statically-`Variant` value.
pub const INFERENCE_ON_VARIANT: &str = "INFERENCE_ON_VARIANT";
/// Incompatible hard types (our umbrella for the engine's `push_error`).
pub const TYPE_MISMATCH: &str = "TYPE_MISMATCH";
/// `float` stored into an `int` slot.
pub const NARROWING_CONVERSION: &str = "NARROWING_CONVERSION";
/// `int / int`.
pub const INTEGER_DIVISION: &str = "INTEGER_DIVISION";
/// A property missing on a statically-known base.
pub const UNSAFE_PROPERTY_ACCESS: &str = "UNSAFE_PROPERTY_ACCESS";
/// A method missing on a statically-known base.
pub const UNSAFE_METHOD_ACCESS: &str = "UNSAFE_METHOD_ACCESS";
/// An argument whose static type needs an unsafe implicit cast (`Variant` / a downcast) into the
/// resolved parameter type — Godot's per-argument value-prop warning.
pub const UNSAFE_CALL_ARGUMENT: &str = "UNSAFE_CALL_ARGUMENT";
/// A `$Path`/`%Unique`/`get_node("…")` whose literal path is genuinely absent in the owning scene
/// (only raised when the script attaches to exactly one scene — never on an `..`/absolute path or a
/// path that descends into an instanced sub-scene we don't see).
pub const INVALID_NODE_PATH: &str = "INVALID_NODE_PATH";
/// A declared `class_name` that shadows another global identifier — a duplicate user `class_name`,
/// an engine/native class, a builtin/utility, a global enum/const, or a `*`-autoload singleton.
/// Godot's `gdscript_analyzer.cpp` raises this (as an error) so the global namespace stays unique.
pub const SHADOWED_GLOBAL_IDENTIFIER: &str = "SHADOWED_GLOBAL_IDENTIFIER";
/// A genuine `extends` cycle: a file's base chain transitively returns to itself (`A extends B`,
/// `B extends A`). Illegal in Godot (`gdscript_analyzer.cpp` raises it). Only the `extends`
/// inheritance chain cycles — a `preload`/`load` cycle is legal at runtime and is NOT reported.
pub const CYCLIC_INHERITANCE: &str = "CYCLIC_INHERITANCE";

/// What kind of binding a [`Binding`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// A local `var` / `const`.
    Var,
    /// A function / lambda parameter.
    Param,
    /// A `for` loop variable.
    ForVar,
    /// A `var x` capture in a `match` pattern (typed `Variant`; arm-scoped).
    MatchBind,
}

/// A typed local binding — the unit hover + inlay hints read for `var`/param/`for` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The binding name (for unused-binding analysis, find-references).
    pub name: SmolStr,
    /// The name token's range.
    pub name_range: TextRange,
    /// The binding's resolved type. For an untyped `var x = e` this is the gradual `Variant`;
    /// the precise initializer type (for an "add type annotation" action) is [`Binding::init`].
    pub ty: Ty,
    /// The initializer expression, when the binding has one (a `var`/`const` with `= e`).
    pub init: Option<ExprId>,
    /// Whether the source carried an explicit `: T` annotation.
    pub annotated: bool,
    /// Whether the source used `:=` (inferred-but-hard).
    pub inferred_colon_eq: bool,
    /// Whether this is a `const` (vs a `var`) — distinguishes `UNUSED_LOCAL_CONSTANT` from
    /// `UNUSED_VARIABLE`.
    pub is_const: bool,
    /// What kind of binding this is.
    pub kind: BindingKind,
}

/// The result of inferring one body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InferenceResult {
    /// Every expression's inferred type (feeds hover + inlay).
    pub expr_ty: FxHashMap<ExprId, Ty>,
    /// The local bindings introduced by the body (params, `var`/`const`, `for` vars).
    pub bindings: Vec<Binding>,
    /// The §5 type diagnostics raised directly (the ungated analyzer-native codes:
    /// `TYPE_MISMATCH`, `INVALID_NODE_PATH` — these have no Godot warning-setting key).
    pub diagnostics: Vec<Diagnostic>,
    /// The gateable Godot warnings, recorded severity-free. Resolved into final diagnostics by
    /// [`crate::warnings::gate`] downstream of the cached `analyze_file` query (Workstream 1).
    pub raw_warnings: Vec<RawWarning>,
}

impl InferenceResult {
    /// The inferred type of an expression, if it was visited.
    #[must_use]
    pub fn type_of(&self, id: ExprId) -> Option<&Ty> {
        self.expr_ty.get(&id)
    }

    /// The binding whose name token contains `offset`, if any.
    #[must_use]
    pub fn binding_at(&self, offset: u32) -> Option<&Binding> {
        self.bindings
            .iter()
            .find(|b| b.name_range.start <= offset && offset < b.name_range.end)
    }
}

/// Infer a lowered `body` (its `tail` initializer expression and/or its statement block).
/// `return_ty` is the function's declared return type (`Variant` if none / for an
/// initializer body).
/// Every assignment-LHS `ExprId` in a body (`x = …` / `x += …`, all lowered to `BinOp::Assign`) —
/// a *write* site, excluded from the `UNASSIGNED_VARIABLE` read-before-assign check.
fn collect_assign_lhs(body: &Body) -> FxHashSet<ExprId> {
    body.exprs
        .iter()
        .filter_map(|e| match e {
            Expr::Bin {
                op: BinOp::Assign,
                lhs,
                ..
            } => Some(*lhs),
            _ => None,
        })
        .collect()
}

/// Infer one function/initializer body against a class scope: walks the lowered [`body::Body`],
/// resolving each expression's [`Ty`] (engine + cross-file members, scene-node paths, flow
/// narrowing) and recording the bindings, diagnostics, and severity-free gateable warnings.
/// Returns the [`InferenceResult`] the IDE features and the warning gate read.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the per-body inference orchestration reads best whole"
)]
pub fn infer(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    class: &ClassScope,
    body: &Body,
    return_ty: Ty,
    is_func_body: bool,
) -> InferenceResult {
    let self_ty = class.self_ty.clone();
    let mut cx = Cx {
        db,
        api,
        root,
        body,
        class,
        self_ty,
        return_ty,
        expr_ty: FxHashMap::default(),
        bindings: Vec::new(),
        diagnostics: Vec::new(),
        raw_warnings: Vec::new(),
        locals: FxHashMap::default(),
        used_locals: FxHashSet::default(),
        narrowing: FxHashMap::default(),
        flow: flow::analyze(body),
        is_func_body,
        assigned: flow::analyze_assigned(
            body,
            &body
                .params
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
        ),
        cur_stmt: None,
        needs_assignment: FxHashSet::default(),
        assign_lhs: collect_assign_lhs(body),
    };
    // Parameters bind first (their defaults can reference earlier params).
    let params = body.params.clone();
    for p in &params {
        let ty = cx.param_ty(p);
        cx.bindings.push(Binding {
            name: p.name.clone(),
            name_range: p.name_range,
            ty: ty.clone(),
            init: None,
            annotated: p.type_ref.is_some(),
            inferred_colon_eq: false,
            is_const: false,
            kind: BindingKind::Param,
        });
        cx.locals.insert(p.name.clone(), ty);
    }
    if let Some(tail) = body.tail {
        cx.infer_expr(tail, &Expectation::None);
    }
    let block = body.block.clone();
    cx.infer_block(&block);

    // UNUSED_* — a declared local/param/const never read. Only for a *function* body: a class-field
    // initializer body would otherwise false-flag every field (the member is read in other methods,
    // not in its own initializer). `_`-prefixed names + loop/match captures are excluded.
    if is_func_body {
        let unused: Vec<(TextRange, WarningCode, String)> = cx
            .bindings
            .iter()
            .filter_map(|b| {
                if b.name.starts_with('_') || cx.used_locals.contains(&b.name) {
                    return None;
                }
                let (code, what) = match b.kind {
                    BindingKind::Param => (WarningCode::UnusedParameter, "parameter"),
                    BindingKind::Var if b.is_const => {
                        (WarningCode::UnusedLocalConstant, "local constant")
                    }
                    BindingKind::Var => (WarningCode::UnusedVariable, "local variable"),
                    BindingKind::ForVar | BindingKind::MatchBind => return None,
                };
                Some((
                    b.name_range,
                    code,
                    format!("The {what} \"{}\" is declared but never used.", b.name),
                ))
            })
            .collect();
        for (range, code, msg) in unused {
            cx.warn(range, code, msg);
        }
    }

    // SHADOWED_GLOBAL_IDENTIFIER — a parameter / local / `for` / pattern-bind whose name collides
    // with a project/engine global (built-in type/function, native class, engine singleton, project
    // `class_name`, or autoload). Godot's `is_shadowing` fires for every local-scope binding. A local
    // `var`/`const` that *also* shadows a param/member emits THIS instead of `SHADOWED_VARIABLE` (the
    // global check wins in `gdscript_analyzer.cpp`; `infer_local_var` suppresses the variable-shadow
    // when a global one applies, so the two never double-fire on one declaration).
    if is_func_body {
        let global_shadows: Vec<(TextRange, String)> = cx
            .bindings
            .iter()
            .filter_map(|b| {
                let kind = shadowed_global_kind(db, api, &b.name)?;
                let what = match b.kind {
                    BindingKind::Param => "parameter",
                    BindingKind::Var if b.is_const => "constant",
                    BindingKind::Var => "variable",
                    BindingKind::ForVar => "for loop variable",
                    BindingKind::MatchBind => "pattern bind",
                };
                Some((
                    b.name_range,
                    format!("The {what} \"{}\" has the same name as a {kind}.", b.name),
                ))
            })
            .collect();
        for (range, msg) in global_shadows {
            cx.warn(range, WarningCode::ShadowedGlobalIdentifier, msg);
        }
    }

    // UNTYPED_DECLARATION / INFERRED_DECLARATION — the opt-in declaration-strictness codes (default
    // IGNORE; promoted to WARN under a strict / standalone run). Driven directly by the binding flags:
    // a `var` declared with `:=` is INFERRED_DECLARATION; a `var` / parameter with neither a `: T`
    // annotation nor `:=` is UNTYPED_DECLARATION. `const` (its value type is always statically known),
    // `for` vars, and pattern binds (fixed by the iterable / scrutinee, not user-typeable) are excluded
    // — they can't carry the static type Godot's strict check expects.
    if is_func_body {
        let decl_strictness: Vec<(TextRange, WarningCode, String)> = cx
            .bindings
            .iter()
            .filter_map(|b| match b.kind {
                BindingKind::Param if !b.annotated => Some((
                    b.name_range,
                    WarningCode::UntypedDeclaration,
                    format!("The parameter \"{}\" has no static type.", b.name),
                )),
                BindingKind::Var if !b.is_const && b.inferred_colon_eq => Some((
                    b.name_range,
                    WarningCode::InferredDeclaration,
                    format!(
                        "The variable \"{}\" uses inferred typing (`:=`); consider declaring its type explicitly.",
                        b.name
                    ),
                )),
                BindingKind::Var if !b.is_const && !b.annotated => Some((
                    b.name_range,
                    WarningCode::UntypedDeclaration,
                    format!("The variable \"{}\" has no static type.", b.name),
                )),
                _ => None,
            })
            .collect();
        for (range, code, msg) in decl_strictness {
            cx.warn(range, code, msg);
        }
    }

    // CONFUSABLE_IDENTIFIER — a parameter / local binding whose name mixes scripts in a spoofable
    // way (the same UTS #39 check used for member names; ASCII names fast-path out).
    let confusable_bindings: Vec<TextRange> = cx
        .bindings
        .iter()
        .filter(|b| is_confusable_identifier(&b.name))
        .map(|b| b.name_range)
        .collect();
    for range in confusable_bindings {
        cx.warn(
            range,
            WarningCode::ConfusableIdentifier,
            "This identifier uses confusable characters (mixed scripts).".to_owned(),
        );
    }

    // UNREACHABLE_CODE — statements after a return/break/continue / exhaustive branch (Workstream 2).
    let unreachable = cx.flow.unreachable_ranges(body);
    for range in unreachable {
        cx.warn(
            range,
            WarningCode::UnreachableCode,
            "Unreachable code (statement after a return, break, continue, or an exhaustive match)."
                .to_owned(),
        );
    }

    // UNREACHABLE_PATTERN — a `match` arm after an unconditional catch-all (Workstream 2).
    let unreachable_patterns = cx.flow.unreachable_pattern_ranges().to_vec();
    for range in unreachable_patterns {
        cx.warn(
            range,
            WarningCode::UnreachablePattern,
            "Unreachable pattern: an earlier arm's wildcard (`_`) or `var` binding always matches."
                .to_owned(),
        );
    }

    InferenceResult {
        expr_ty: cx.expr_ty,
        bindings: cx.bindings,
        diagnostics: cx.diagnostics,
        raw_warnings: cx.raw_warnings,
    }
}

/// Convenience: recover a function node from its [`AstPtr`], lower its body, resolve its
/// declared return type, and infer it.
#[must_use]
pub fn infer_func(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    class: &ClassScope,
    ptr: AstPtr,
) -> InferenceResult {
    let Some(node) = ptr.to_node(root) else {
        return InferenceResult::default();
    };
    let body = body::body_of_func(&node);
    // The return-type annotation is the FuncDecl's direct `TypeRef` child (params' type refs
    // are nested inside the ParamList, so they are not direct children).
    let return_ty = cst::first_child(&node, |k| k == gdscript_syntax::SyntaxKind::TypeRef)
        .map_or(Ty::Variant, |t| resolve::resolve_type_ref(db, api, &t));
    infer(db, api, root, class, &body, return_ty, true)
}

/// One inferred unit of a file: a function body or a class field's initializer, with its
/// lowered [`Body`] and [`InferenceResult`] (kept so position-based features — hover, inlay,
/// member completion — can map a cursor back through the source map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The source range this unit covers (the function decl or the field decl).
    pub range: TextRange,
    /// The lowered body.
    pub body: Body,
    /// The inference result.
    pub result: InferenceResult,
}

/// The full single-file inference: the item tree, every inferred unit, and the merged §5
/// diagnostics. The whole-file entry point the IDE layer consumes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileInference {
    /// The lowered item tree.
    pub tree: Arc<ItemTree>,
    /// The inferred function/field units.
    pub units: Vec<Unit>,
    /// The ungated analyzer-native diagnostics, merged across units (`TYPE_MISMATCH`,
    /// `INVALID_NODE_PATH`) plus the file-level `SHADOWED_GLOBAL_IDENTIFIER` / `CYCLIC_INHERITANCE`.
    pub diagnostics: Vec<Diagnostic>,
    /// The severity-free gateable Godot warnings, merged across units. The IDE layer resolves
    /// these via [`crate::warnings::gate`] against the project's settings (Workstream 1).
    pub raw_warnings: Vec<RawWarning>,
}

impl FileInference {
    /// The innermost unit whose range contains `offset`.
    #[must_use]
    pub fn unit_at(&self, offset: u32) -> Option<&Unit> {
        self.units
            .iter()
            .filter(|u| u.range.start <= offset && offset < u.range.end)
            .min_by_key(|u| u.range.end - u.range.start)
    }
}

/// Infer an entire file: lower its item tree, then infer every function body and every
/// class-field initializer against a shared [`ClassScope`]. The single entry point for the
/// IDE features (Playbook §6 — a pure `(api, parsed file) -> result` function).
#[must_use]
#[allow(clippy::too_many_lines)] // the two-pass field-fixpoint + function walk reads best whole
pub fn analyze_file(db: &dyn Db, api: &EngineApi, root: &GdNode, file_id: FileId) -> FileInference {
    let tree = item_tree(root);
    let mut units = Vec::new();
    let mut diagnostics = Vec::new();
    let mut raw_warnings: Vec<RawWarning> = Vec::new();

    // EMPTY_FILE — a script with no members, no `class_name`, and no `extends` (Workstream 1).
    if tree.members.is_empty() && tree.class_name.is_none() && tree.extends.is_none() {
        raw_warnings.push(RawWarning {
            range: TextRange::new(0, 0),
            code: WarningCode::EmptyFile,
            message: "Empty script file.".to_owned(),
        });
    }
    let mut member_types: FxHashMap<SmolStr, Ty> = FxHashMap::default();
    // `self` is the script's OWN class (a self-`ScriptRef`), not just its engine base — so member
    // access on an aliased `self` resolves the file's own members (see `ClassScope::self_ty`).
    let self_ref = Ty::ScriptRef(ScriptRefId(file_id.0));
    // The file's own `res://` path, for anchoring relative `preload`/`extends` to its directory.
    let res_path = db.file_text(file_id).and_then(|ft| ft.res_path(db));

    // A declared `class_name` that collides with another global identifier (W2). Mirrors Godot's
    // `gdscript_analyzer.cpp` uniqueness check over the global namespace, projected through the
    // cross-file firewall (`class_name_collisions`) and the offset-free global resolvers — so it
    // fires only when genuinely shadowing, never on the seam. Emitted once, at the decl's NAME.
    if let Some(name) = tree.class_name.clone() {
        let collides = collisions_contains(db, &name)
            || resolve::resolve_global(api, &name).is_some()
            || is_autoload_singleton(db, &name);
        if collides && let Some(range) = class_name_decl_range(root) {
            diagnostics.push(Diagnostic {
                range,
                severity: Severity::Warning,
                code: SHADOWED_GLOBAL_IDENTIFIER.to_owned(),
                message: format!(
                    "The global class \"{name}\" hides a built-in/native/global/autoload."
                ),
                source: DiagnosticSource::Type,
                fixes: Vec::new(),
            });
        }
        // CONFUSABLE_IDENTIFIER on the `class_name` itself (gated, unlike the hides-global check).
        if is_confusable_identifier(&name)
            && let Some(range) = class_name_decl_range(root)
        {
            raw_warnings.push(RawWarning {
                range,
                code: WarningCode::ConfusableIdentifier,
                message: format!(
                    "The identifier \"{name}\" uses confusable characters (mixed scripts)."
                ),
            });
        }
    }

    // A genuine `extends` cycle (D7): walk THIS file's base chain by `FileId`; if it returns to the
    // start, the inheritance is cyclic (illegal in Godot). Reported once, at the file's own `extends`
    // decl range. Only `extends` cycles are walked here (member lookup is the only thing that loops);
    // `preload`/`load` cycles are legal at runtime and never reach this resolver. We start by stepping
    // ONTO the user base — if the very first base is the start file (`extends "res://self.gd"`, or two
    // files A↔B), the revisit-of-start check fires; a deep but ACYCLIC chain bottoms out at an engine
    // `Object`/`Unknown` and never revisits, so it does not false-fire.
    if extends_chain_is_cyclic(db, file_id)
        && let Some(range) = extends_decl_range(root)
    {
        diagnostics.push(Diagnostic {
            range,
            severity: Severity::Warning,
            code: CYCLIC_INHERITANCE.to_owned(),
            message: "Cyclic class hierarchy: this class's `extends` chain returns to itself."
                .to_owned(),
            source: DiagnosticSource::Type,
            fixes: Vec::new(),
        });
    }

    // File-level (member) warnings that need the whole item-tree, not a single body.
    raw_warnings.extend(member_level_warnings(
        db,
        api,
        root,
        &tree,
        res_path.as_deref(),
    ));

    // Pass 1 — class fields. Inferring each `var`/`const` seeds `member_types` so the function
    // pass sees the *inferred* field type (`var n := 0` → `int`), not just the annotation.
    //
    // A field initializer may reference an *earlier* field (`var a := 1` then `var b := a + 1`),
    // so a single shallow round sees the referent as `Variant`/seam. We run a BOUNDED fixpoint:
    // each round re-infers every field against the prior round's `member_types`, until the map
    // stops changing or we hit the round cap. Cheap (fields are few, types settle in a round or
    // two) and deterministic. Only the final round's units/diagnostics are kept — earlier rounds
    // are throwaway probes feeding the seed.
    {
        // Bound the iteration: a linear `a -> b -> c -> …` chain settles in O(n) rounds, but a
        // small constant is enough in practice (the corpus settles in ≤2) and guarantees
        // termination even if a type oscillated.
        const MAX_ROUNDS: usize = 4;
        let mut final_units: Vec<Unit> = Vec::new();
        let mut final_diagnostics: Vec<Diagnostic> = Vec::new();
        let mut final_raw_warnings: Vec<RawWarning> = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let mut class = ClassScope::new(db, api, &tree, res_path.as_deref());
            class.self_ty = self_ref.clone();
            class.member_types.clone_from(&member_types);
            let mut next_member_types: FxHashMap<SmolStr, Ty> = FxHashMap::default();
            final_units = Vec::new();
            final_diagnostics = Vec::new();
            final_raw_warnings = Vec::new();
            for m in &tree.members {
                let (ptr, range) = match m {
                    Member::Var(v) => (v.ptr, v.range),
                    Member::Const(c) => (c.ptr, c.range),
                    _ => continue,
                };
                if let Some(unit) = unit_from_decl(db, api, root, &class, ptr, range) {
                    if let (Some(name), Some(b)) = (m.name(), unit.result.bindings.first()) {
                        next_member_types.insert(SmolStr::new(name), b.ty.clone());
                    }
                    final_diagnostics.extend(unit.result.diagnostics.iter().cloned());
                    final_raw_warnings.extend(unit.result.raw_warnings.iter().cloned());
                    final_units.push(unit);
                }
            }
            if next_member_types == member_types {
                break;
            }
            member_types = next_member_types;
        }
        diagnostics.extend(final_diagnostics);
        raw_warnings.extend(final_raw_warnings);
        units.extend(final_units);
    }

    // Pass 2 — functions, against a scope carrying the seeded field types.
    {
        let mut class = ClassScope::new(db, api, &tree, res_path.as_deref());
        class.member_types = member_types;
        class.self_ty = self_ref.clone();
        for m in &tree.members {
            let Member::Func(f) = m else { continue };
            let Some(node) = f.ptr.to_node(root) else {
                continue;
            };
            let body = body::body_of_func(&node);
            let return_ty = cst::first_child(&node, |k| k == gdscript_syntax::SyntaxKind::TypeRef)
                .map_or(Ty::Variant, |t| resolve::resolve_type_ref(db, api, &t));
            let result = infer(db, api, root, &class, &body, return_ty, true);
            diagnostics.extend(result.diagnostics.iter().cloned());
            raw_warnings.extend(result.raw_warnings.iter().cloned());
            units.push(Unit {
                range: f.range,
                body,
                result,
            });
        }
    }

    // Pass 2b — inner-class method bodies. The top-level pass skips `Member::Class`, so inner `class
    // Name:` methods were never inferred (no units / diagnostics / resolvable refs). Analyze them with
    // `self` typed as the inner class, so `self.member` / bare member refs resolve against the inner
    // item-tree + its `extends` chain; anything unresolved stays the seam (no false positive).
    infer_inner_class_bodies(
        db,
        api,
        root,
        &tree,
        file_id,
        "",
        res_path.as_deref(),
        &mut units,
        &mut diagnostics,
        &mut raw_warnings,
        0,
    );

    FileInference {
        tree,
        units,
        diagnostics,
        raw_warnings,
    }
}

/// Infer the method bodies of every inner `class Name:` in `tree` (recursively), with `self` typed as
/// the inner class. `path_prefix` is the dotted path to `tree`'s class (`""` at the top level), used
/// to build each inner class's [`crate::ty::InnerClassRef`] path. Depth-bounded against pathological
/// nesting. Inner-class *field* fixpoint pre-pass is intentionally skipped (an inner field types by
/// annotation only — lossy, like the cross-file path).
#[allow(
    clippy::too_many_arguments,
    reason = "threads the same analyze_file accumulators a free helper can't capture from a closure"
)]
fn infer_inner_class_bodies(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    tree: &ItemTree,
    file_id: FileId,
    path_prefix: &str,
    res_path: Option<&str>,
    units: &mut Vec<Unit>,
    diagnostics: &mut Vec<Diagnostic>,
    raw_warnings: &mut Vec<RawWarning>,
    depth: u32,
) {
    if depth > 16 {
        return;
    }
    for m in &tree.members {
        let Member::Class(c) = m else { continue };
        let inner_path = if path_prefix.is_empty() {
            c.name.to_string()
        } else {
            format!("{path_prefix}.{}", c.name)
        };
        let mut class = ClassScope::new(db, api, &c.tree, res_path);
        class.self_ty = Ty::InnerClass(crate::ty::InnerClassRef {
            file: file_id.0,
            path: SmolStr::new(&inner_path),
        });
        for im in &c.tree.members {
            let Member::Func(f) = im else { continue };
            let Some(node) = f.ptr.to_node(root) else {
                continue;
            };
            let body = body::body_of_func(&node);
            let return_ty = cst::first_child(&node, |k| k == gdscript_syntax::SyntaxKind::TypeRef)
                .map_or(Ty::Variant, |t| resolve::resolve_type_ref(db, api, &t));
            let result = infer(db, api, root, &class, &body, return_ty, true);
            diagnostics.extend(result.diagnostics.iter().cloned());
            raw_warnings.extend(result.raw_warnings.iter().cloned());
            units.push(Unit {
                range: f.range,
                body,
                result,
            });
        }
        infer_inner_class_bodies(
            db,
            api,
            root,
            &c.tree,
            file_id,
            &inner_path,
            res_path,
            units,
            diagnostics,
            raw_warnings,
            depth + 1,
        );
    }
}

/// Class-level annotation checks (W1), unblocked by first-class item-tree annotations:
/// `REDUNDANT_STATIC_UNLOAD` (`@static_unload` with no `static var`) and `MISSING_TOOL` (a non-`@tool`
/// class extending a `@tool` user-script base).
fn class_annotation_warnings(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    tree: &ItemTree,
    res_path: Option<&str>,
) -> Vec<RawWarning> {
    let mut out = Vec::new();
    // REDUNDANT_STATIC_UNLOAD — `@static_unload` on a class that declares no `static var`.
    if let Some(unload) = tree.annotations.iter().find(|a| a.name == "static_unload")
        && !tree
            .members
            .iter()
            .any(|m| matches!(m, Member::Var(v) if v.is_static))
    {
        out.push(RawWarning {
            range: unload.range,
            code: WarningCode::RedundantStaticUnload,
            message: "`@static_unload` is redundant on a class with no static variables."
                .to_owned(),
        });
    }
    // MISSING_TOOL — this class is not `@tool` but its (user-script) base is, so it will NOT run in
    // the editor. Only the resolvable user-script base is checked (an engine base is never `@tool`).
    if !has_annotation(&tree.annotations, "tool")
        && user_base_is_tool(db, api, tree, res_path)
        && let Some(range) = extends_decl_range(root)
    {
        out.push(RawWarning {
            range,
            code: WarningCode::MissingTool,
            message: "This class extends a `@tool` script but is not itself `@tool` (it will not run in the editor)."
                .to_owned(),
        });
    }
    out
}

/// File-level (member) warnings that need the whole item-tree, not a single body (W1):
/// `ENUM_VARIABLE_WITHOUT_DEFAULT`, `UNUSED_SIGNAL`, `UNUSED_PRIVATE_CLASS_VARIABLE`,
/// `SHADOWED_GLOBAL_IDENTIFIER`, `CONFUSABLE_IDENTIFIER`, the annotation-lifecycle checks, and
/// `NATIVE_METHOD_OVERRIDE` — each independent + conservative.
#[allow(
    clippy::too_many_lines,
    reason = "a flat sequence of independent per-member warning checks; reads best as one walk"
)]
fn member_level_warnings(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    tree: &ItemTree,
    res_path: Option<&str>,
) -> Vec<RawWarning> {
    let mut out = Vec::new();
    let has_signal = tree.members.iter().any(|m| matches!(m, Member::Signal(_)));
    // A `_`-prefixed, non-exported member var is an UNUSED_PRIVATE_CLASS_VARIABLE candidate.
    let has_private_var = tree
        .members
        .iter()
        .any(|m| matches!(m, Member::Var(v) if v.name.starts_with('_') && !v.is_exported));
    // Only pay for the whole-file name scan when there is a signal or a private var to judge.
    let uses = (has_signal || has_private_var).then(|| NameUses::collect(root));
    // The resolved ENGINE base, for NATIVE_METHOD_OVERRIDE (an unresolved/user base ⇒ no check).
    let engine_base = match resolve::resolve_base(db, api, tree, res_path) {
        Ty::Object(c) => Some(c),
        _ => None,
    };

    out.extend(class_annotation_warnings(db, api, root, tree, res_path));

    for m in &tree.members {
        // UNUSED_PRIVATE_CLASS_VARIABLE — a `_`-prefixed, non-exported member var never referenced
        // anywhere in the file (same-file scan, like UNUSED_SIGNAL). Exported vars are set externally
        // (inspector / scene), so they are excluded to keep the contract no-false-positive.
        if let Member::Var(v) = m
            && v.name.starts_with('_')
            && !v.is_exported
            && let Some(uses) = &uses
            && !uses.is_referenced(&v.name)
        {
            out.push(RawWarning {
                range: v.name_range,
                code: WarningCode::UnusedPrivateClassVariable,
                message: format!(
                    "The class variable \"{}\" is never used in this file.",
                    v.name
                ),
            });
        }
        // ONREADY_WITH_EXPORT — `@onready` and `@export` on the same member (Godot raises this).
        if let Member::Var(v) = m
            && has_annotation(&v.annotations, "onready")
            && v.is_exported
        {
            out.push(RawWarning {
                range: v.name_range,
                code: WarningCode::OnreadyWithExport,
                message: format!(
                    "The member \"{}\" has both `@onready` and `@export`; they conflict.",
                    v.name
                ),
            });
        }
        // SHADOWED_GLOBAL_IDENTIFIER — a value member (`var`/`const`/`signal`) whose name collides
        // with a project/engine global. Godot's `is_shadowing` fires for member declarations too.
        if let Some((name, range, what)) = member_value_decl(m)
            && let Some(kind) = shadowed_global_kind(db, api, name)
        {
            out.push(RawWarning {
                range,
                code: WarningCode::ShadowedGlobalIdentifier,
                message: format!("The {what} \"{name}\" has the same name as a {kind}."),
            });
        }
        // CONFUSABLE_IDENTIFIER — any member name that mixes scripts in a spoofable way.
        if let Some((name, range)) = member_decl_name(m)
            && is_confusable_identifier(name)
        {
            out.push(RawWarning {
                range,
                code: WarningCode::ConfusableIdentifier,
                message: format!(
                    "The identifier \"{name}\" uses confusable characters (mixed scripts)."
                ),
            });
        }
        match m {
            // An enum-typed field with no initializer (the local case is in `infer_local_var`).
            Member::Var(v) if !v.has_init => {
                if let Some(tref) = &v.type_ref
                    && matches!(resolve::resolve_type_name(db, api, tref), Ty::Enum(_))
                {
                    out.push(RawWarning {
                        range: v.name_range,
                        code: WarningCode::EnumVariableWithoutDefault,
                        message: format!(
                            "The enum variable \"{}\" has no default value (it defaults to 0, which may not be a valid enum value).",
                            v.name
                        ),
                    });
                }
            }
            // A signal never referenced in this file (emit/connect/string). Same-file only, like
            // Godot — a signal connected purely from a scene/other file is invisible (the known
            // limitation); the conservative scan only warns when the name appears nowhere else.
            Member::Signal(s) => {
                if let Some(uses) = &uses
                    && !uses.is_referenced(&s.name)
                {
                    out.push(RawWarning {
                        range: s.name_range,
                        code: WarningCode::UnusedSignal,
                        message: format!(
                            "The signal \"{}\" is never emitted or connected in this file.",
                            s.name
                        ),
                    });
                }
            }
            // NATIVE_METHOD_OVERRIDE (ERROR-default) — an override of an engine VIRTUAL whose
            // signature is clearly incompatible. Conservative to the extreme (a false positive is a
            // loud error): warn ONLY on a *definite type clash* at an overlapping typed parameter —
            // both the override's annotation and the virtual's param resolve to known engine types
            // that are mutually NON-assignable. Arity, defaults/vararg, and variance subtleties are
            // deliberately left to under-warn (see `TECH_DEBT.md`).
            Member::Func(f) => {
                if let Some(base) = engine_base
                    && let Some(MemberRef::Method(vsig)) = api.lookup_member(base, &f.name)
                    && vsig.is_virtual
                {
                    for (p, vp) in f.params.iter().zip(vsig.params.iter()) {
                        let Some(ann) = &p.type_ref else { continue };
                        let pty = resolve::resolve_type_name(db, api, ann);
                        let vty = ty::resolve_tyref(api, &vp.ty);
                        if types_definitely_clash(api, &pty, &vty) {
                            out.push(RawWarning {
                                range: f.name_range,
                                code: WarningCode::NativeMethodOverride,
                                message: format!(
                                    "The override of the native virtual method \"{}\" has an incompatible type for parameter \"{}\".",
                                    f.name, p.name
                                ),
                            });
                            break; // one warning per overriding function
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether two **known** engine types are mutually non-assignable — a *definite* clash. An
/// uninformative type (`Variant`/`Unknown`) never clashes (gradual), and any assignable relation in
/// either direction (subtype, widening, enum/int, …) is treated as "related" (not a clash), so the
/// conservative `NATIVE_METHOD_OVERRIDE` only fires on genuinely unrelated types.
fn types_definitely_clash(api: &EngineApi, a: &Ty, b: &Ty) -> bool {
    if a.is_uninformative() || b.is_uninformative() {
        return false;
    }
    // Enums are int-backed and their qualified name resolves differently on the annotation side
    // (`resolve_type_name` → `Class.Enum`) than on the engine-model side (`resolve_tyref`), so an
    // enum "clash" is unreliable — never clash on an enum. (Fixes a false NATIVE_METHOD_OVERRIDE on
    // a valid dotted-enum-typed override param, e.g. `p_mode: MultiplayerPeer.TransferMode`.)
    if matches!(a, Ty::Enum(_)) || matches!(b, Ty::Enum(_)) {
        return false;
    }
    matches!(ty::is_assignable(api, a, b), Assign::No)
        && matches!(ty::is_assignable(api, b, a), Assign::No)
}

/// Identifier-occurrence counts + string-literal contents across a file's CST — the file-wide
/// "is this name referenced anywhere?" check (drives `UNUSED_SIGNAL`).
struct NameUses {
    ident_counts: FxHashMap<SmolStr, u32>,
    strings: FxHashSet<SmolStr>,
}

impl NameUses {
    fn collect(root: &GdNode) -> Self {
        let mut ident_counts: FxHashMap<SmolStr, u32> = FxHashMap::default();
        let mut strings: FxHashSet<SmolStr> = FxHashSet::default();
        for node in gdscript_syntax::ast::descendants(root) {
            for el in node.children_with_tokens() {
                let Some(tok) = el.into_token() else { continue };
                match tok.kind() {
                    gdscript_syntax::SyntaxKind::Ident => {
                        *ident_counts.entry(SmolStr::new(tok.text())).or_insert(0) += 1;
                    }
                    gdscript_syntax::SyntaxKind::String => {
                        strings.insert(SmolStr::new(tok.text().trim_matches(['"', '\''])));
                    }
                    _ => {}
                }
            }
        }
        Self {
            ident_counts,
            strings,
        }
    }

    /// Whether `name` is referenced beyond its single declaration — a 2nd identifier occurrence, or
    /// any string literal naming it (covering `emit_signal("name")` / `connect("name", …)`).
    fn is_referenced(&self, name: &str) -> bool {
        self.ident_counts.get(name).copied().unwrap_or(0) > 1 || self.strings.contains(name)
    }
}

/// Whether `name` is declared as a `class_name` by more than one file in the project (W2). Reads
/// the cross-file `class_name_collisions` firewall; `false` (no warning) when no source root is set
/// — single-file analysis cannot observe a duplicate.
fn collisions_contains(db: &dyn Db, name: &SmolStr) -> bool {
    db.source_root()
        .is_some_and(|root| crate::queries::class_name_collisions(db, root).contains(name))
}

/// Whether `name` is a `*`-flagged autoload singleton (a bare global). `false` when no
/// `project.godot` is loaded — the seam, no warning.
fn is_autoload_singleton(db: &dyn Db, name: &str) -> bool {
    db.project_config().is_some_and(|config| {
        crate::queries::autoload_registry(db, config)
            .resolve_path(name)
            .is_some()
    })
}

/// Whether the file's resolved **user-script** base carries the `@tool` annotation (for
/// `MISSING_TOOL`). An engine base or an unresolved base is never `@tool`. Firewall-safe: reads the
/// base's `item_tree` (signature-level — a base *body* edit leaves the annotation set unchanged).
fn user_base_is_tool(
    db: &dyn Db,
    api: &EngineApi,
    tree: &ItemTree,
    res_path: Option<&str>,
) -> bool {
    let Ty::ScriptRef(sref) = resolve::resolve_base(db, api, tree, res_path) else {
        return false;
    };
    let Some(ft) = db.file_text(FileId(sref.0)) else {
        return false;
    };
    has_annotation(&crate::queries::item_tree(db, ft).annotations, "tool")
}

/// Whether `name` is registered as a global `class_name` by some file in the project. `false` when
/// no source root is set (single-file analysis can't observe the registry). Reads the firewalled
/// [`crate::queries::global_registry`].
fn is_registered_global_class(db: &dyn Db, name: &str) -> bool {
    db.source_root().is_some_and(|root| {
        crate::queries::global_registry(db, root)
            .resolve(name)
            .is_some()
    })
}

/// Whether a declared identifier `name` shadows a project/engine **global**, returning Godot's
/// category label for the `SHADOWED_GLOBAL_IDENTIFIER` message, else `None`. Mirrors
/// `gdscript_analyzer.cpp`'s `is_shadowing` global checks: a built-in function, a built-in (Variant)
/// type, a native class, an engine singleton, a project `class_name` global, or a `*`-autoload
/// singleton. **Conservative:** bare global pseudo-constants (`PI`/`TAU`) and global enum namespaces
/// (`Error`/`Key`) are deliberately excluded — they are rare as user identifiers (and the tokenizer
/// treats the math constants as literals), so this only ever *under*-warns vs. Godot, never a false
/// positive. With no source root / `project.godot`, the cross-file/autoload arms are silent (seam).
fn shadowed_global_kind(db: &dyn Db, api: &EngineApi, name: &str) -> Option<&'static str> {
    match resolve::resolve_global(api, name) {
        Some(GlobalDef::Builtin | GlobalDef::Utility) => return Some("built-in function"),
        Some(GlobalDef::BuiltinType(_)) => return Some("built-in type"),
        Some(GlobalDef::ClassType(_)) => return Some("native class"),
        Some(GlobalDef::Singleton(_)) => return Some("engine singleton"),
        // Bare pseudo-constants / global enums: intentionally not flagged (see doc above).
        Some(GlobalDef::Const(_) | GlobalDef::GlobalEnum) | None => {}
    }
    if is_registered_global_class(db, name) {
        return Some("global class");
    }
    if is_autoload_singleton(db, name) {
        return Some("autoload");
    }
    None
}

/// The `(name, name range, kind noun)` of a *value-declaring* member (`var`/`const`/`signal`) — the
/// members that `SHADOWED_GLOBAL_IDENTIFIER` checks at the class level. `None` for funcs / enums /
/// inner classes (where the "shadow" framing is weaker, matching Godot's `is_shadowing` callers).
fn member_value_decl(m: &Member) -> Option<(&SmolStr, TextRange, &'static str)> {
    match m {
        Member::Var(v) => Some((&v.name, v.name_range, "variable")),
        Member::Const(c) => Some((&c.name, c.name_range, "constant")),
        Member::Signal(s) => Some((&s.name, s.name_range, "signal")),
        _ => None,
    }
}

/// The declared `(name, name range)` of any named member (`func`/`var`/`const`/`signal`/named
/// `enum`/inner `class`), for the `CONFUSABLE_IDENTIFIER` scan. An anonymous `enum { … }` has no
/// name → `None`.
fn member_decl_name(m: &Member) -> Option<(&SmolStr, TextRange)> {
    match m {
        Member::Func(f) => Some((&f.name, f.name_range)),
        Member::Var(v) => Some((&v.name, v.name_range)),
        Member::Const(c) => Some((&c.name, c.name_range)),
        Member::Signal(s) => Some((&s.name, s.name_range)),
        Member::Class(c) => Some((&c.name, c.name_range)),
        Member::Enum(e) => e.name.as_ref().map(|n| (n, e.name_range)),
    }
}

/// Whether `name` is a `CONFUSABLE_IDENTIFIER` — a non-ASCII identifier that mixes scripts in a
/// spoofable way (UTS #39 restriction level ≥ `MinimallyRestrictive`, e.g. a Latin identifier with a
/// Cyrillic/Greek homoglyph like `pаypal`). Pure-ASCII (the overwhelming majority) and legitimate
/// single-script / CJK-plus-Latin identifiers are never flagged. Mirrors the intent of Godot's
/// `TextServer` confusable check with zero false positives on ordinary code.
fn is_confusable_identifier(name: &str) -> bool {
    use unicode_security::RestrictionLevel as RL;
    use unicode_security::RestrictionLevelDetection;
    if name.is_ascii() {
        return false; // the fast path for ~all real identifiers
    }
    name.detect_restriction_level() >= RL::MinimallyRestrictive
}

/// The NAME range of the file's `class_name` declaration, trimmed to the bare identifier (the
/// `Name` CST node absorbs leading inter-token trivia). `None` if the file declares no `class_name`
/// or the decl has no name token. Mirrors `item_tree::trimmed_name_range` / navigation's
/// `class_decl_target` (which lives in the IDE crate, hence this local CST scan).
fn class_name_decl_range(root: &GdNode) -> Option<TextRange> {
    use gdscript_syntax::SyntaxKind;
    let decl = gdscript_syntax::ast::descendants(root)
        .into_iter()
        .find(|n| n.kind() == SyntaxKind::ClassNameDecl)?;
    let name_node = decl.children().find(|c| c.kind() == SyntaxKind::Name)?;
    let r = cst::text_range_of(name_node);
    let text = name_node.text().to_string();
    let lead = u32::try_from(text.len() - text.trim_start().len()).unwrap_or(0);
    let len = u32::try_from(text.trim().len()).unwrap_or(0);
    Some(TextRange::new(r.start + lead, r.start + lead + len))
}

/// The byte range of the file's top-level `extends` declaration — the anchor for `CYCLIC_INHERITANCE`.
/// Two surface forms: a standalone `extends Target` (an [`ExtendsClause`] child of the `SourceFile`),
/// or the inline `class_name Name extends Target` (the `extends` keyword + target inside the
/// [`ClassNameDecl`]). Scans only the `SourceFile`'s DIRECT children, so an inner class's `extends`
/// (nested under `Class`/`ClassBody`) is never mistaken for the file's own. `None` if the file has no
/// top-level `extends`.
fn extends_decl_range(root: &GdNode) -> Option<TextRange> {
    use gdscript_syntax::SyntaxKind;
    for child in root.children() {
        match child.kind() {
            // Standalone `extends Target` — the whole clause is the anchor.
            SyntaxKind::ExtendsClause => return Some(cst::text_range_of(child)),
            // Inline `class_name Name extends Target` — anchor the `extends` keyword onward.
            SyntaxKind::ClassNameDecl => {
                if let Some(kw) = child.children().find(|c| c.kind() == SyntaxKind::ExtendsKw) {
                    let start = cst::text_range_of(kw).start;
                    let end = cst::text_range_of(child).end;
                    return Some(TextRange::new(start, end));
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the file's `extends` inheritance chain transitively returns to itself (a genuine cycle).
/// Walks base-by-base by `FileId` from `start`, stepping only across user `ScriptRef` bases (an
/// engine `Object`/`Unknown` base ends the chain). A `FileId` revisit means a cycle. We stop as soon
/// as we either revisit a file (cycle) or hit a non-script base (acyclic) — a deep but acyclic chain
/// terminates without a revisit and is NOT flagged. Depth is also hard-capped as belt-and-suspenders
/// (the visited set already guarantees termination).
fn extends_chain_is_cyclic(db: &dyn Db, start: FileId) -> bool {
    use std::collections::HashSet;
    let mut visited: HashSet<FileId> = HashSet::new();
    visited.insert(start);
    let mut current = start;
    for _ in 0..=64 {
        let Some(file) = db.file_text(current) else {
            return false;
        };
        let base = crate::queries::script_class(db, file).base().clone();
        let Ty::ScriptRef(next) = base else {
            return false; // engine `Object` / `Unknown` base — chain ends, no cycle.
        };
        let next_id = FileId(next.0);
        if !visited.insert(next_id) {
            // Revisiting an already-seen file closes a cycle. We report the cycle for every file ON
            // it (each file's own `extends` is genuinely cyclic), so no need to special-case `start`.
            return true;
        }
        current = next_id;
    }
    false
}

/// Infer a class field declaration as a single local-var statement (full annotation checks).
fn unit_from_decl(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    class: &ClassScope,
    ptr: AstPtr,
    range: TextRange,
) -> Option<Unit> {
    let node = ptr.to_node(root)?;
    let body = body::body_of_decl_stmt(&node);
    let result = infer(db, api, root, class, &body, Ty::Variant, false);
    Some(Unit {
        range,
        body,
        result,
    })
}

/// What type is expected of an expression (bidirectional checking).
enum Expectation {
    /// No expectation — pure synthesis.
    None,
    /// The expression is checked against this declared type.
    Has(Ty),
}

/// Navigate a dotted inner-class path (`Inner` / `Outer.Inner`) from a file's top item-tree to the
/// target [`InnerClassItem`]. `None` if any segment isn't an inner class.
fn find_inner_class<'a>(tree: &'a ItemTree, path: &str) -> Option<&'a InnerClassItem> {
    let mut members: &'a [Member] = &tree.members;
    let mut found: Option<&'a InnerClassItem> = None;
    for seg in path.split('.') {
        found = members.iter().find_map(|m| match m {
            Member::Class(c) if c.name == seg => Some(c),
            _ => None,
        });
        members = &found?.tree.members;
    }
    found
}

struct Cx<'a> {
    db: &'a dyn Db,
    api: &'a EngineApi,
    root: &'a GdNode,
    body: &'a Body,
    class: &'a ClassScope<'a>,
    self_ty: Ty,
    return_ty: Ty,
    expr_ty: FxHashMap<ExprId, Ty>,
    bindings: Vec<Binding>,
    diagnostics: Vec<Diagnostic>,
    /// Severity-free gateable warnings (Workstream 1), resolved by `gate()` downstream.
    raw_warnings: Vec<RawWarning>,
    /// Function-scoped local bindings (GDScript locals are function-, not block-, scoped).
    locals: FxHashMap<SmolStr, Ty>,
    /// The names of locals/params that were *read* during the walk — drives the `UNUSED_*` family (a
    /// declared binding whose name never appears here is unused). A bare assignment LHS (`x = …`) is a
    /// write, NOT a read, and is excluded (so an assigned-but-never-read local is correctly unused);
    /// a compound `x += …` still reads via its RHS, and a receiver / index target reads the base.
    used_locals: FxHashSet<SmolStr>,
    /// The active narrowing env for the current statement, keyed by a dotted access path. Rebuilt
    /// per statement from [`Cx::flow`] (Workstream 2) — not mutated ad-hoc anymore.
    narrowing: FxHashMap<String, Ty>,
    /// The precomputed per-body control-flow narrowing facts (Workstream 2). The checker consults
    /// `facts_before(stmt)` to build [`Cx::narrowing`]; it survives `else`/early-return/`and`-`or`.
    flow: FlowAnalysis,
    /// Whether this is a real function body (vs a class-field initializer body). Gates the
    /// body-only checks (`UNUSED_*`, `SHADOWED_VARIABLE`) so a field initializer doesn't, e.g.,
    /// "shadow itself" against its own member entry.
    is_func_body: bool,
    /// Definite-assignment facts (Workstream 2) — the locals assigned before each statement, for
    /// `UNASSIGNED_VARIABLE`. Consulted at a read via [`Cx::cur_stmt`].
    assigned: flow::AssignedAnalysis,
    /// The statement currently being inferred (set in `infer_stmt`), so a read can look up
    /// [`Cx::assigned`].
    cur_stmt: Option<body::StmtId>,
    /// Typed locals declared **without** an initializer — the only locals `UNASSIGNED_VARIABLE`
    /// considers (an untyped/`:=`/initialized local is never read-before-assign). Grows as the walk
    /// passes each declaration.
    needs_assignment: FxHashSet<SmolStr>,
    /// Names that are the direct LHS of an assignment (`x = …`/`x += …`) — a *write*, not a read, so
    /// excluded from the `UNASSIGNED_VARIABLE` check even though inference resolves the LHS.
    assign_lhs: FxHashSet<ExprId>,
}

impl Cx<'_> {
    // ---- small type constructors ----

    fn builtin(&self, name: &str) -> Ty {
        self.api
            .builtin_by_name(name)
            .map_or(Ty::Variant, Ty::Builtin)
    }
    fn int_ty(&self) -> Ty {
        self.builtin("int")
    }
    fn float_ty(&self) -> Ty {
        self.builtin("float")
    }
    fn bool_ty(&self) -> Ty {
        self.builtin("bool")
    }
    fn is_int(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Builtin(b) if self.api.builtin(*b).name == "int")
    }
    fn is_float(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Builtin(b) if self.api.builtin(*b).name == "float")
    }
    fn is_numeric(&self, ty: &Ty) -> bool {
        self.is_int(ty) || self.is_float(ty)
    }

    // ---- diagnostics ----

    fn emit(&mut self, range: TextRange, severity: Severity, code: &str, message: String) {
        self.diagnostics.push(Diagnostic {
            range,
            severity,
            code: code.to_owned(),
            message,
            source: DiagnosticSource::Type,
            fixes: Vec::new(),
        });
    }

    /// Record a gateable Godot warning, severity-free. The resolved severity (and whether it fires
    /// at all) is decided later by [`crate::warnings::gate`], keyed on the project's warning
    /// settings — so a settings edit never re-runs inference (Workstream 1, the salsa firewall).
    fn warn(&mut self, range: TextRange, code: WarningCode, message: String) {
        self.raw_warnings.push(RawWarning {
            range,
            code,
            message,
        });
    }

    fn range_of(&self, id: ExprId) -> TextRange {
        self.body.source_map.expr_range(id)
    }

    /// Run `is_assignable(from, to)` and raise the matching diagnostic. Safe to call
    /// unconditionally: `to` being `Variant`/`Unknown` yields `Ok`/no diagnostic.
    fn check_assign(&mut self, from: &Ty, to: &Ty, range: TextRange) {
        match ty::is_assignable(self.api, from, to) {
            Assign::Narrowing => self.warn(
                range,
                WarningCode::NarrowingConversion,
                "Narrowing conversion (float is converted to int and loses precision).".to_owned(),
            ),
            Assign::No => {
                let to_label = to.label(self.api).unwrap_or_else(|| "?".to_owned());
                let from_label = from.label(self.api).unwrap_or_else(|| "?".to_owned());
                self.emit(
                    range,
                    Severity::Error,
                    TYPE_MISMATCH,
                    format!(
                        "Cannot assign a value of type \"{from_label}\" to a target of type \"{to_label}\"."
                    ),
                );
            }
            // `int` assigned to an enum slot without an explicit cast (the previously-dead arm).
            Assign::IntAsEnum => self.warn(
                range,
                WarningCode::IntAsEnumWithoutCast,
                "Integer used when an enum value is expected. Cast the value to the enum type."
                    .to_owned(),
            ),
            Assign::Ok | Assign::OkUnsafe => {}
        }
    }

    /// Flag a statement whose expression has no effect: a bare value (`STANDALONE_EXPRESSION`) or a
    /// ternary used as a statement (`STANDALONE_TERNARY`). A call / await / assignment / `preload`
    /// has an effect and is never flagged.
    fn check_standalone(&mut self, e: ExprId) {
        if self.expr_has_side_effect(e) {
            return;
        }
        match self.body.expr(e) {
            Expr::Ternary { .. } => self.warn(
                self.range_of(e),
                WarningCode::StandaloneTernary,
                "Standalone ternary conditional: the return value is discarded.".to_owned(),
            ),
            // Not value-like statements / forms with subtle effects — never flag.
            Expr::Missing | Expr::Lambda { .. } | Expr::GetNode { .. } | Expr::Preload { .. } => {}
            _ => self.warn(
                self.range_of(e),
                WarningCode::StandaloneExpression,
                "Standalone expression (the line has no effect).".to_owned(),
            ),
        }
    }

    /// Whether evaluating an expression may have a side effect — a call, an `await`, a `preload`,
    /// or an assignment anywhere in the subtree. Used to suppress `STANDALONE_*` on effectful lines.
    fn expr_has_side_effect(&self, e: ExprId) -> bool {
        match self.body.expr(e) {
            Expr::Call { .. }
            | Expr::Await(_)
            | Expr::Preload { .. }
            | Expr::Bin {
                op: BinOp::Assign, ..
            } => true,
            Expr::Bin { lhs, rhs, .. }
            | Expr::In { lhs, rhs, .. }
            | Expr::Index {
                base: lhs,
                index: rhs,
            } => self.expr_has_side_effect(*lhs) || self.expr_has_side_effect(*rhs),
            Expr::Unary { operand, .. }
            | Expr::Paren(operand)
            | Expr::Cast { operand, .. }
            | Expr::Is { operand, .. } => self.expr_has_side_effect(*operand),
            Expr::Field { receiver, .. } => self.expr_has_side_effect(*receiver),
            Expr::Ternary {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr_has_side_effect(*cond)
                    || self.expr_has_side_effect(*then_branch)
                    || self.expr_has_side_effect(*else_branch)
            }
            Expr::Array(items) => items.iter().any(|&i| self.expr_has_side_effect(i)),
            Expr::Dict(entries) => entries.iter().any(|(k, v)| {
                self.expr_has_side_effect(*k) || v.is_some_and(|e| self.expr_has_side_effect(e))
            }),
            _ => false,
        }
    }

    // ---- statements ----

    fn infer_block(&mut self, block: &[body::StmtId]) {
        for &stmt in block {
            self.infer_stmt(stmt);
        }
    }

    fn infer_stmt(&mut self, id: body::StmtId) {
        // Install the narrowing in force *before* this statement (Workstream 2). Recomputed per
        // statement from the precomputed flow facts — replaces the old ad-hoc `in_branch` frames.
        self.narrowing = self.facts_to_narrowing(id);
        self.cur_stmt = Some(id); // for the read-before-assign (UNASSIGNED_VARIABLE) check
        match self.body.stmt(id).clone() {
            Stmt::Expr(e) => {
                self.infer_expr(e, &Expectation::None);
                self.check_standalone(e);
            }
            Stmt::Var(v) => self.infer_local_var(&v),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    let expected = if self.return_ty.is_uninformative() {
                        Expectation::None
                    } else {
                        Expectation::Has(self.return_ty.clone())
                    };
                    let t = self.infer_expr(e, &expected);
                    if let Expectation::Has(ret) = expected {
                        self.check_assign(&t, &ret, self.range_of(e));
                    }
                }
            }
            Stmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
            } => {
                // The branch narrowing now lives in the flow facts, so each sub-statement installs
                // its own via `infer_stmt`. Restore the if-level facts before each guard (a block
                // walk overwrites `self.narrowing`).
                let at_if = self.narrowing.clone();
                self.infer_expr(cond, &Expectation::None);
                self.infer_block(&then_branch);
                for (econd, eblock) in elifs {
                    self.narrowing.clone_from(&at_if);
                    self.infer_expr(econd, &Expectation::None);
                    self.infer_block(&eblock);
                }
                if let Some(eb) = else_branch {
                    self.infer_block(&eb);
                }
            }
            Stmt::While { cond, body } => {
                self.infer_expr(cond, &Expectation::None);
                self.infer_block(&body);
            }
            Stmt::For(f) => {
                let iter_ty = self.infer_expr(f.iter, &Expectation::None);
                let var_ty = f.var_type.as_ref().map_or_else(
                    || self.loop_var_ty(&iter_ty),
                    |ptr| self.resolve_ptr_ty(*ptr),
                );
                self.bindings.push(Binding {
                    name: f.var.clone(),
                    name_range: f.var_range,
                    ty: var_ty.clone(),
                    init: None,
                    annotated: f.var_type.is_some(),
                    inferred_colon_eq: false,
                    is_const: false,
                    kind: BindingKind::ForVar,
                });
                self.locals.insert(f.var.clone(), var_ty);
                self.infer_block(&f.body);
            }
            Stmt::Match { scrutinee, arms } => {
                let at_match = self.narrowing.clone();
                self.infer_expr(scrutinee, &Expectation::None);
                for arm in arms {
                    // Restore the match-level facts before each arm's guard (a prior arm's body
                    // walk overwrote `self.narrowing`).
                    self.narrowing.clone_from(&at_match);
                    for b in &arm.binds {
                        // Record the capture as a binding so navigation (find-refs / rename) sees
                        // it as a local that shadows a same-named member; the type is the Phase-2
                        // `Variant`.
                        self.bindings.push(Binding {
                            name: b.name.clone(),
                            name_range: b.range,
                            ty: Ty::Variant,
                            init: None,
                            annotated: false,
                            inferred_colon_eq: false,
                            is_const: false,
                            kind: BindingKind::MatchBind,
                        });
                        self.locals.insert(b.name.clone(), Ty::Variant);
                    }
                    if let Some(g) = arm.guard {
                        self.infer_expr(g, &Expectation::None);
                    }
                    self.infer_block(&arm.body);
                }
            }
            Stmt::Break | Stmt::Continue | Stmt::Pass => {}
            Stmt::Assert(cond) => {
                if let Some(cond) = cond {
                    self.infer_expr(cond, &Expectation::None);
                    self.check_assert_constant(cond);
                }
            }
        }
    }

    /// `ASSERT_ALWAYS_TRUE` / `ASSERT_ALWAYS_FALSE` — fire when the assert condition is a constant
    /// with a known boolean value (Godot `resolve_assert`: a constant condition is booleanized and
    /// warned). Sound subset via [`Cx::const_bool_of`]: a literal `true`/`false`, or `null` (false).
    fn check_assert_constant(&mut self, cond: ExprId) {
        let Some(always) = self.const_bool_of(cond) else {
            return;
        };
        let (code, msg) = if always {
            (
                WarningCode::AssertAlwaysTrue,
                "The assert condition is always true, so this assert has no effect.",
            )
        } else {
            (
                WarningCode::AssertAlwaysFalse,
                "The assert condition is always false, so this assert will always fail.",
            )
        };
        self.warn(self.range_of(cond), code, msg.to_owned());
    }

    /// The constant boolean value of `expr`, when it is a literal whose booleanization is known — a
    /// bool literal, or `null` (false). `None` for any other / non-constant expression (the sound
    /// default: no false `ASSERT_ALWAYS_*`). Mirrors Godot's `reduced_value.booleanize()` restricted
    /// to the literal forms (named-constant / arithmetic folding is deliberately not attempted).
    fn const_bool_of(&self, expr: ExprId) -> Option<bool> {
        match self.body.expr(expr) {
            Expr::Literal(Literal::Bool(b)) => Some(*b),
            Expr::Literal(Literal::Null) => Some(false),
            _ => None,
        }
    }

    fn infer_local_var(&mut self, v: &body::LocalVar) {
        let annotated = v.type_ref.map(|p| self.resolve_ptr_ty(p));
        let init_ty = v.init.map(|e| {
            let expected = annotated
                .as_ref()
                .map_or(Expectation::None, |t| Expectation::Has(t.clone()));
            self.infer_expr(e, &expected)
        });
        let range = v.init.map_or(v.name_range, |e| self.range_of(e));

        let binding_ty = match (&annotated, &init_ty) {
            // `var x: T = e` — hard slot; check the initializer against it.
            (Some(t), Some(init)) => {
                self.check_assign(init, t, range);
                t.clone()
            }
            // `var x: T` (no init).
            (Some(t), None) => t.clone(),
            // `var x := e` — inferred (hard); guard the Variant / null cases.
            (None, Some(init)) if v.is_inferred => {
                if init.is_variant() {
                    self.warn(
                        range,
                        WarningCode::InferenceOnVariant,
                        inference_on_variant_msg(if v.is_const { "constant" } else { "variable" }),
                    );
                    Ty::Variant
                } else {
                    // `Unknown` (the seam) stays `Unknown` with no warning.
                    init.clone()
                }
            }
            // `var x = e` — untyped, soft → Variant. `const X = e` keeps the inferred type.
            (None, Some(init)) => {
                if v.is_const {
                    init.clone()
                } else {
                    Ty::Variant
                }
            }
            (None, None) => Ty::Variant,
        };
        // SHADOWED_VARIABLE — a local `var`/`const` whose name shadows a parameter or an own class
        // member (a redeclared *local* is a Godot error, not handled here). Sound: only fires on a
        // genuine outer-scope shadow. The binding isn't pushed yet, so the `Param` scan can't see it.
        // Gated to a real function body — a class-field initializer's own `var n` is not a shadow.
        let shadows_param = self
            .bindings
            .iter()
            .any(|b| b.kind == BindingKind::Param && b.name == v.name);
        // Only a *value* member (var/const/signal, or an anon-enum constant) — not a method or a
        // type name, where the "shadow" framing is weaker — counts, to stay conservative.
        let shadows_member = match self.class.lookup(&v.name) {
            Some(ClassItem::EnumVariant) => true,
            Some(item) => matches!(
                self.class.member(item),
                Some(Member::Var(_) | Member::Const(_) | Member::Signal(_))
            ),
            None => false,
        };
        if self.is_func_body {
            let what = if v.is_const { "constant" } else { "variable" };
            // A global-identifier shadow takes precedence over a variable/base-class shadow (Godot's
            // `is_shadowing` checks globals first and returns), so the variable-shadow is emitted only
            // when the name does NOT shadow a global — the global one is emitted by the binding
            // post-pass in `infer`, keeping a single warning per declaration.
            if shadowed_global_kind(self.db, self.api, &v.name).is_none() {
                if shadows_param || shadows_member {
                    let outer = if shadows_param {
                        "parameter"
                    } else {
                        "class member"
                    };
                    self.warn(
                        v.name_range,
                        WarningCode::ShadowedVariable,
                        format!(
                            "The local {what} \"{}\" shadows a {outer} of the same name.",
                            v.name
                        ),
                    );
                } else if self.engine_base_has_value_member(&v.name) {
                    // An own-member shadow already won above; only a *base*-member shadow reaches here.
                    self.warn(
                        v.name_range,
                        WarningCode::ShadowedVariableBaseClass,
                        format!(
                            "The local {what} \"{}\" shadows a member of a base class.",
                            v.name
                        ),
                    );
                }
            }
            // ENUM_VARIABLE_WITHOUT_DEFAULT — a local typed as an enum with no initializer (the
            // implicit `0` may not name a valid enum value). Only an explicit `Ty::Enum` annotation.
            if v.init.is_none() && matches!(annotated.as_ref(), Some(Ty::Enum(_))) {
                self.warn(
                    v.name_range,
                    WarningCode::EnumVariableWithoutDefault,
                    format!(
                        "The enum variable \"{}\" has no default value (it defaults to 0, which may not be a valid enum value).",
                        v.name
                    ),
                );
            }
            // A typed local declared WITHOUT an initializer is the only `UNASSIGNED_VARIABLE`
            // candidate (an untyped / `:=` / initialized local is never read-before-assign).
            if v.type_ref.is_some() && v.init.is_none() {
                self.needs_assignment.insert(v.name.clone());
            }
        }
        self.bindings.push(Binding {
            name: v.name.clone(),
            name_range: v.name_range,
            ty: binding_ty.clone(),
            init: v.init,
            annotated: v.type_ref.is_some(),
            inferred_colon_eq: v.is_inferred,
            is_const: v.is_const,
            kind: BindingKind::Var,
        });
        // A (re-)declaration's narrowing invalidation is handled by the flow analysis (Workstream 2).
        self.locals.insert(v.name.clone(), binding_ty);
    }

    // ---- expressions ----

    fn infer_expr(&mut self, id: ExprId, expected: &Expectation) -> Ty {
        let ty = self.synth_expr(id, expected);
        self.expr_ty.insert(id, ty.clone());
        ty
    }

    #[allow(clippy::too_many_lines)]
    fn synth_expr(&mut self, id: ExprId, expected: &Expectation) -> Ty {
        match self.body.expr(id).clone() {
            Expr::Missing => Ty::Error,
            Expr::Literal(lit) => self.literal_ty(lit),
            Expr::Name(name) => self.resolve_name(id, &name),
            Expr::SelfExpr => self.self_ty.clone(),
            Expr::Super => self.class.base.clone(),
            Expr::Paren(inner) => self.infer_expr(inner, expected),
            Expr::Bin { op, lhs, rhs } => self.infer_bin(id, op, lhs, rhs),
            Expr::Unary { op, operand } => {
                let t = self.infer_expr(operand, &Expectation::None);
                match op {
                    UnOp::Not => self.bool_ty(),
                    UnOp::BitNot => self.int_ty(),
                    UnOp::Neg | UnOp::Pos => {
                        if t.is_uninformative() || self.is_numeric(&t) {
                            t
                        } else {
                            Ty::Variant
                        }
                    }
                }
            }
            Expr::Ternary {
                cond,
                then_branch,
                else_branch,
            } => {
                self.infer_expr(cond, &Expectation::None);
                let a = self.infer_expr(then_branch, expected);
                let b = self.infer_expr(else_branch, expected);
                // A `null` branch does not poison the other: `x if c else null` is nullable-`x`.
                if self.is_null(else_branch) {
                    a
                } else if self.is_null(then_branch) {
                    b
                } else {
                    let r = self.join(&a, &b);
                    // Both arms informative but with no common type (the join widened to Variant) —
                    // the ternary's two values are mutually incompatible.
                    if r.is_variant() && !a.is_uninformative() && !b.is_uninformative() {
                        self.warn(
                            self.range_of(id),
                            WarningCode::IncompatibleTernary,
                            "The values of the ternary conditional are not mutually compatible."
                                .to_owned(),
                        );
                    }
                    r
                }
            }
            Expr::Call { callee, args } => self.infer_call(callee, &args),
            Expr::Field {
                receiver,
                name,
                name_range,
            } => {
                self.infer_field(receiver, &name, name_range, /*as_method=*/ false)
            }
            Expr::Index { base, index } => {
                let base_ty = self.infer_expr(base, &Expectation::None);
                self.infer_expr(index, &Expectation::None);
                self.index_ty(&base_ty, index)
            }
            Expr::Is { operand, .. } => {
                self.infer_expr(operand, &Expectation::None);
                self.bool_ty()
            }
            Expr::Cast { operand, ty } => {
                self.infer_expr(operand, &Expectation::None);
                ty.map_or(Ty::Variant, |p| self.resolve_ptr_ty(p))
            }
            Expr::In { lhs, rhs, .. } => {
                self.infer_expr(lhs, &Expectation::None);
                self.infer_expr(rhs, &Expectation::None);
                self.bool_ty()
            }
            Expr::Await(operand) => {
                let operand_ty = self.infer_expr(operand, &Expectation::None);
                // `await coroutine()` yields the call's value, so await is **identity** on the operand
                // type (`await f()` for `func f() -> int` is `int`) — recovered here. `await signal`
                // instead yields the signal's emitted payload, which needs the Phase-3+ signal-signature
                // table; until then it's the seam (never `Variant`, so `var x := await sig` never warns).
                if matches!(operand_ty, Ty::Signal(_)) {
                    Ty::Unknown
                } else {
                    operand_ty
                }
            }
            Expr::Array(elems) => {
                // Checking mode: an expected `Array[T]` is pushed down onto the literal (so
                // `var a: Array[String] = []` / `[...]` is accepted). Otherwise the engine does
                // not infer a literal's element type past `Variant`.
                let pushed = match expected {
                    Expectation::Has(Ty::Array(e)) => Some((**e).clone()),
                    _ => None,
                };
                let elem_exp = pushed.clone().map_or(Expectation::None, Expectation::Has);
                for e in elems {
                    self.infer_expr(e, &elem_exp);
                }
                pushed.map_or_else(Ty::array_of_variant, |e| Ty::Array(Box::new(e)))
            }
            Expr::Dict(entries) => {
                let pushed = match expected {
                    Expectation::Has(Ty::Dict(k, v)) => Some(((**k).clone(), (**v).clone())),
                    _ => None,
                };
                let (kx, vx) = pushed
                    .clone()
                    .map_or((Expectation::None, Expectation::None), |(k, v)| {
                        (Expectation::Has(k), Expectation::Has(v))
                    });
                for (k, v) in entries {
                    self.infer_expr(k, &kx);
                    if let Some(v) = v {
                        self.infer_expr(v, &vx);
                    }
                }
                pushed.map_or_else(Ty::dict_of_variant, |(k, v)| {
                    Ty::Dict(Box::new(k), Box::new(v))
                })
            }
            Expr::Lambda { params, body } => {
                self.infer_lambda(&params, &body);
                Ty::Callable
            }
            Expr::Preload { arg, path } => {
                if let Some(arg) = arg {
                    self.infer_expr(arg, &Expectation::None);
                }
                // A constant string-literal path resolves to the declaring file's `ScriptRef`
                // (M3 — a SCRIPT meta-type in Godot; `X.new()`/`X.member` then resolve via the
                // usual `ScriptRef` walk). A non-constant argument (`preload(var)`) — which Godot
                // itself rejects — stays the seam, never a false diagnostic.
                match path {
                    // Anchor a relative `preload("sibling.gd")` to the importing file's directory
                    // before resolving (Godot anchors relative resource paths); absolute paths pass
                    // through, and a relative path with no anchor stays the seam.
                    Some(p) => {
                        match resolve::anchor_res_path(self.self_res_path().as_deref(), &p) {
                            Some(abs) => resolve::resolve_external(
                                self.db,
                                &resolve::ExternalRef::Preload(abs),
                            ),
                            None => Ty::Unknown,
                        }
                    }
                    None => Ty::Unknown,
                }
            }
            // `$Path`/`%Unique` — resolve the literal path against the owning scene to the node's
            // concrete type (Phase-4 M1); a computed/unresolvable path stays `Object(Node)`.
            Expr::GetNode { path, unique } => self.resolve_node_path(id, path.as_deref(), unique),
        }
    }

    /// Whether `id` is the `null` literal.
    fn is_null(&self, id: ExprId) -> bool {
        matches!(self.body.expr(id), Expr::Literal(Literal::Null))
    }

    fn literal_ty(&self, lit: Literal) -> Ty {
        match lit {
            Literal::Int(_) => self.int_ty(),
            Literal::Float | Literal::MathConst => self.float_ty(),
            Literal::Bool(_) => self.bool_ty(),
            Literal::Str => self.builtin("String"),
            Literal::StringName => self.builtin("StringName"),
            Literal::NodePath => self.builtin("NodePath"),
            // `null` is compatible everywhere; typing it `Variant` avoids false mismatches.
            Literal::Null => Ty::Variant,
        }
    }

    fn node_ty(&self) -> Ty {
        self.api
            .class_by_name("Node")
            .map_or(Ty::Unknown, Ty::Object)
    }

    // ---- scene-aware node-path typing (Phase-4 M1) ----

    /// Resolve a `$Path`/`%Unique`/`get_node("…")` literal node path against the owning scene to the
    /// node's concrete type. A computed (`None`) path, no owning scene, an `..`/absolute escape, or a
    /// path that descends into an instanced sub-scene all degrade to `Object(Node)` — never a false
    /// positive. A *genuinely* absent in-scene node raises `INVALID_NODE_PATH` (M2), but only when
    /// the script attaches to exactly one scene (an ambiguous multi-scene attachment stays silent).
    fn resolve_node_path(&mut self, id: ExprId, path: Option<&str>, unique: bool) -> Ty {
        use gdscript_scene::NodePathResolution as R;
        let fallback = self.node_ty();
        let Some(path) = path else {
            return fallback; // computed `get_node(var)` — stays `Node`
        };
        // An absolute `/root/<Autoload>` access resolves to the autoload's type — singleton OR
        // loaded-but-not-global (both live at `/root/Name`). Independent of any owning scene. A deeper
        // tail (`/root/Name/Child`) would need to walk the autoload's own scene; left as the seam.
        if !unique && let Some(ty) = self.resolve_root_autoload_path(path) {
            return ty;
        }
        let Some(ctx) = self.owning_scene() else {
            return fallback; // no scene attaches this script (dynamic UI / single-file)
        };
        // Multi-scene attachment (M2 §6.3): a `$Path` may resolve to a different node type in each
        // attaching scene, so type it as the COMMON BASE across all of them (never the first-scene
        // type, which could be wrong for another scene). If any scene can't resolve it identically,
        // degrade to `Node` — never a false positive and never a false `INVALID_NODE_PATH`.
        if ctx.ambiguous {
            return self.union_node_ty(path, unique).unwrap_or(fallback);
        }
        let resolution = if unique {
            ctx.model.classify_unique(path)
        } else {
            ctx.model.classify_path_from(ctx.attach, path)
        };
        match resolution {
            R::Resolved(idx) => ctx
                .model
                .node(idx)
                .and_then(|n| self.scene_node_ty(&ctx.model, n, 0))
                .unwrap_or(fallback),
            R::Missing => {
                let what = if unique { "unique name" } else { "node path" };
                let sigil = if unique { "%" } else { "$" };
                self.emit(
                    self.range_of(id),
                    Severity::Warning,
                    INVALID_NODE_PATH,
                    format!("no {what} `{sigil}{path}` in the owning scene"),
                );
                fallback
            }
            // The path descends into an instanced sub-scene (`$Enemy/Sprite`): resolve the tail in
            // the sub-scene's own tree (`Sprite` typed by `enemy.tscn`). Any failure → `Node`.
            R::IntoInstance => {
                let walked = if unique {
                    ctx.model.resolve_unique_into_instance(path)
                } else {
                    ctx.model.resolve_into_instance(ctx.attach, path)
                };
                walked
                    .and_then(|(inst, tail)| {
                        let inst_node = ctx.model.node(inst)?;
                        self.resolve_into_instance_ty(&ctx.model, inst_node, &tail, 0)
                    })
                    .unwrap_or(fallback)
            }
            // An `..`/absolute escape out of the slice → `Node`, never a false warning.
            R::Escaped => fallback,
        }
    }

    /// Union-type a `$Path`/`%Unique` across **every** scene that attaches this script (the rare
    /// multi-scene case): resolve the path in each scene, then take the COMMON BASE of the per-scene
    /// node types. `None` (→ caller degrades to `Node`) if any scene fails to resolve the path the
    /// same way — keeping the no-false-positive contract on an ambiguous attachment.
    fn union_node_ty(&self, path: &str, unique: bool) -> Option<Ty> {
        use gdscript_scene::NodePathResolution as R;
        let res_path = self.self_res_path()?;
        let root = self.db.source_root()?;
        let attaches = crate::queries::script_scene_attachments(self.db, root)
            .get(res_path.as_str())
            .cloned()?;
        let mut acc: Option<Ty> = None;
        for (scene_file, attach) in &attaches {
            let ft = self.db.file_text(*scene_file)?;
            let model = crate::queries::scene_model(self.db, ft);
            let resolution = if unique {
                model.classify_unique(path)
            } else {
                model.classify_path_from(*attach, path)
            };
            let R::Resolved(idx) = resolution else {
                return None; // a miss / escape / into-instance in some scene → bail to `Node`
            };
            let ty = model
                .node(idx)
                .and_then(|n| self.scene_node_ty(&model, n, 0))?;
            acc = Some(match acc {
                None => ty,
                Some(prev) => self.common_base(&prev, &ty),
            });
        }
        acc
    }

    /// The common base of two scene-node types — the lowest engine class both descend from (walk
    /// `a`'s ancestor chain, return the first that `b` is a subclass of). Identical types collapse to
    /// themselves; a `ScriptRef` or any mixed pair degrades to the `Node` floor (the engine base for
    /// every scene node), which is always a sound supertype.
    fn common_base(&self, a: &Ty, b: &Ty) -> Ty {
        if a == b {
            return a.clone();
        }
        if let (Ty::Object(ca), Ty::Object(cb)) = (a, b) {
            let mut cur = Some(*ca);
            while let Some(c) = cur {
                if self.api.is_subclass(*cb, c) {
                    return Ty::Object(c);
                }
                cur = self.api.class(c).base;
            }
        }
        self.node_ty()
    }

    /// Resolve an absolute `/root/<Autoload>` node path to the autoload's type (singleton or
    /// loaded-but-not-global — both are children of the scene-tree root). `None` for any other path,
    /// including a deeper tail (`/root/Name/Child`, which would need the autoload's own scene) — those
    /// degrade to the `Node` seam with no false positive.
    fn resolve_root_autoload_path(&self, path: &str) -> Option<Ty> {
        let name = path.strip_prefix("/root/")?;
        // Only the autoload node itself (no trailing segment) for now.
        if name.is_empty() || name.contains('/') {
            return None;
        }
        let ty = resolve::resolve_autoload_any(self.db, name);
        (!ty.is_uninformative()).then_some(ty)
    }

    /// The owning-scene context for the current file (scene + attach node + multi-scene ambiguity).
    /// Recovered from `self_ty`, which `analyze_file` sets to the file's own `ScriptRef` (so no extra
    /// `FileId` threading).
    fn owning_scene(&self) -> Option<crate::queries::SceneContext> {
        let Ty::ScriptRef(sref) = &self.self_ty else {
            return None;
        };
        let ft = self.db.file_text(FileId(sref.0))?;
        crate::queries::scene_context(self.db, ft)
    }

    /// The importing file's own `res://` path (from `self_ty`), for anchoring relative
    /// `preload`/`extends` paths to its directory. `None` when the file has no resource path.
    fn self_res_path(&self) -> Option<SmolStr> {
        let Ty::ScriptRef(sref) = &self.self_ty else {
            return None;
        };
        self.db.file_text(FileId(sref.0))?.res_path(self.db)
    }

    /// The concrete `Ty` of a scene node, by precedence: an attached script's own class (most
    /// specific) wins; else the declared `type=` (native class or `class_name`); else — an instanced
    /// node (`instance=`, no own `type=`/script) — the **instanced sub-scene's root** type (M3,
    /// recursive). `None` for a node we can't sharpen (the caller degrades to `Node`).
    fn scene_node_ty(&self, scene: &SceneModel, node: &SceneNode, depth: u32) -> Option<Ty> {
        if let Some(script_ty) = self.node_script_ref(scene, node) {
            return Some(script_ty);
        }
        if let Some(decl) = node.decl_type.as_ref() {
            let ty = resolve::resolve_type_name(self.db, self.api, decl);
            if !ty.is_uninformative() {
                return Some(ty);
            }
        }
        self.instance_root_ty(scene, node, depth)
            .or_else(|| self.override_child_ty(scene, node, depth))
    }

    /// An **override child** *under* an instance: a node added/overridden in the outer scene beneath
    /// an `instance=` boundary (`[node name="Sprite" parent="Enemy"]` over an instanced `enemy.tscn`),
    /// carrying no own `type=`/`script`/`instance=` — so its real type lives in the instanced
    /// sub-scene. Walk up to the nearest instance-boundary ancestor, then type the node by its same
    /// path *inside* that sub-scene (so the outer override of `enemy.tscn`'s `Sprite` types as the
    /// sub-scene's `Sprite`, not bare `Node`). `None` if the node is not under an instance (the
    /// caller then floors to `Node`, unchanged). Depth-bounded against an instancing cycle.
    fn override_child_ty(&self, scene: &SceneModel, node: &SceneNode, depth: u32) -> Option<Ty> {
        if depth >= 16 {
            return None;
        }
        let mut segs_rev: Vec<String> = vec![node.name.to_string()];
        let mut parent_idx = node.parent_idx?;
        let mut guard = 0u32;
        loop {
            let parent = scene.node(parent_idx)?;
            if parent.instance.is_some() {
                segs_rev.reverse();
                let rel = segs_rev.join("/");
                return self.resolve_into_instance_ty(scene, parent, &rel, depth + 1);
            }
            segs_rev.push(parent.name.to_string());
            parent_idx = parent.parent_idx?;
            guard += 1;
            if guard > 4096 {
                return None;
            }
        }
    }

    /// An instanced node (`instance=ExtResource(id)`) takes the type of the instanced sub-scene's
    /// ROOT node — resolved recursively, so the root's own script / `type=` / nested instance all
    /// flow through (so `$Enemy` types as `enemy.tscn`'s root class, not bare `Node`). Depth-bounded
    /// against an instancing cycle (scene A instances B instances A).
    fn instance_root_ty(&self, scene: &SceneModel, node: &SceneNode, depth: u32) -> Option<Ty> {
        if depth >= 16 {
            return None;
        }
        let (sub, sub_root) = self.instance_subscene(scene, node)?;
        let root_node = sub.node(sub_root)?;
        self.scene_node_ty(&sub, root_node, depth + 1)
    }

    /// The instanced sub-scene's model + its root index, for an instance node (`instance=ExtResource`
    /// → `res://` path → `FileId` → `scene_model`). The shared resolution step for both
    /// [`instance_root_ty`](Self::instance_root_ty) (the node's own type) and
    /// [`resolve_into_instance_ty`](Self::resolve_into_instance_ty) (paths that go *into* it).
    fn instance_subscene(
        &self,
        scene: &SceneModel,
        node: &SceneNode,
    ) -> Option<(Arc<SceneModel>, gdscript_scene::NodeIdx)> {
        let inst = node.instance.as_ref()?;
        let path = scene.ext_resources.get(inst)?.path.as_ref()?;
        let root = self.db.source_root()?;
        let file = crate::queries::res_path_registry(self.db, root)
            .get(path.as_str())
            .copied()?;
        let ft = self.db.file_text(file)?;
        let sub = crate::queries::scene_model(self.db, ft);
        let sub_root = sub.root?;
        Some((sub, sub_root))
    }

    /// Type a node path that descends INTO an instanced sub-scene: `instance_node` is the boundary
    /// (an `instance=` node) and `tail` is the remaining path. Resolve `tail` from the sub-scene's
    /// root, recursing through further instance boundaries inside it. Depth-bounded against an
    /// instancing cycle. `None` (→ `Node`, no false warning) if the tail genuinely can't be typed.
    fn resolve_into_instance_ty(
        &self,
        scene: &SceneModel,
        instance_node: &SceneNode,
        tail: &str,
        depth: u32,
    ) -> Option<Ty> {
        if depth >= 16 {
            return None;
        }
        let (sub, sub_root) = self.instance_subscene(scene, instance_node)?;
        if let Some(idx) = sub.resolve_path_from(sub_root, tail) {
            let n = sub.node(idx)?;
            return self.scene_node_ty(&sub, n, depth + 1);
        }
        // The tail crosses a further instance boundary *inside* the sub-scene — keep descending.
        let (inner, inner_tail) = sub.resolve_into_instance(sub_root, tail)?;
        let inner_node = sub.node(inner)?;
        self.resolve_into_instance_ty(&sub, inner_node, &inner_tail, depth + 1)
    }

    /// The `ScriptRef` of a node's attached `.gd` script (`script = ExtResource(id)` → its `res://`
    /// path → `FileId`), or `None` if it has no resolvable external script.
    fn node_script_ref(&self, scene: &SceneModel, node: &SceneNode) -> Option<Ty> {
        let path = scene
            .ext_resources
            .get(node.script.as_ref()?)?
            .path
            .as_ref()?;
        let root = self.db.source_root()?;
        let file = crate::queries::res_path_registry(self.db, root)
            .get(path.as_str())
            .copied()?;
        Some(Ty::ScriptRef(ScriptRefId(file.0)))
    }

    fn infer_bin(&mut self, id: ExprId, op: BinOp, lhs: ExprId, rhs: ExprId) -> Ty {
        if op == BinOp::Assign {
            return self.infer_assign(lhs, rhs);
        }
        // Short-circuit narrowing (Workstream 2): the RHS of `a and b` is typed under `a`'s
        // then-facts; `a or b`'s RHS under `a`'s else-facts. Restore the env afterward.
        if matches!(op, BinOp::And | BinOp::Or) {
            self.infer_expr(lhs, &Expectation::None);
            let saved = self.narrowing.clone();
            self.apply_condition_facts(lhs, op == BinOp::And);
            self.infer_expr(rhs, &Expectation::None);
            self.narrowing = saved;
            return self.bool_ty();
        }
        let lt = self.infer_expr(lhs, &Expectation::None);
        let rt = self.infer_expr(rhs, &Expectation::None);
        if op.is_boolean() {
            return self.bool_ty();
        }
        // `int / int` discards the fractional part.
        if op == BinOp::Div && self.is_int(&lt) && self.is_int(&rt) {
            self.warn(
                self.range_of(id),
                WarningCode::IntegerDivision,
                "Integer division. Decimal part will be discarded.".to_owned(),
            );
            return self.int_ty();
        }
        self.bin_result(op, &lt, &rt)
    }

    fn infer_assign(&mut self, lhs: ExprId, rhs: ExprId) -> Ty {
        let slot = self.infer_expr(lhs, &Expectation::None);
        let expected = if slot.is_uninformative() {
            Expectation::None
        } else {
            Expectation::Has(slot.clone())
        };
        let value = self.infer_expr(rhs, &expected);
        if !slot.is_uninformative() {
            self.check_assign(&value, &slot, self.range_of(rhs));
        }
        // Assignment *invalidates* the place's narrowing (handled by the flow analysis, Workstream
        // 2); re-narrowing from the assigned value's type is a post-1.0 precision item.
        slot
    }

    /// Resolve a binary operator's result type via the builtin operator table, with a numeric
    /// fallback. Comparison/logical operators are handled by the caller.
    fn bin_result(&self, op: BinOp, lt: &Ty, rt: &Ty) -> Ty {
        if let (Ty::Builtin(b), Some(sym)) = (lt, op_symbol(op)) {
            for o in self.api.builtin_operators(*b) {
                if o.op == sym
                    && let Some(right) = &o.right
                    && self.tyref_matches(right, rt)
                {
                    return ty::resolve_tyref(self.api, &o.result);
                }
            }
        }
        if self.is_numeric(lt) && self.is_numeric(rt) {
            return if self.is_float(lt) || self.is_float(rt) {
                self.float_ty()
            } else {
                self.int_ty()
            };
        }
        // A seam operand keeps the result on the seam (`a + unknown` is `Unknown`, not the
        // gradual `Variant`, so `var x := a + unknown` never warns).
        if lt.is_unknown() || rt.is_unknown() || lt.is_error() || rt.is_error() {
            return Ty::Unknown;
        }
        Ty::Variant
    }

    fn tyref_matches(&self, tyref: &TyRef, ty: &Ty) -> bool {
        let resolved = ty::resolve_tyref(self.api, tyref);
        resolved.is_variant() || &resolved == ty
    }

    fn infer_call(&mut self, callee: ExprId, args: &[ExprId]) -> Ty {
        // Argument expressions are always inferred (their own diagnostics + hover).
        for &a in args {
            self.infer_expr(a, &Expectation::None);
        }
        let ret = match self.body.expr(callee).clone() {
            Expr::Field {
                receiver,
                name,
                name_range,
            } => {
                self.infer_field(receiver, &name, name_range, /*as_method=*/ true)
            }
            Expr::Name(name) => {
                // Locals first (BUG A2), matching `resolve_name`'s canonical local→member→… order:
                // a bare-name call on a local/param (`var f = func(): …` then `f(0)`, a
                // `cb: Callable` param, a hook alias `var useState = Hooks.useState`) is a READ of
                // that binding — record it for `UNUSED_*`, which only the value-read path used to
                // do, so a binding that was only ever *called* fired a false UNUSED. The local also
                // shadows a same-named own/inherited method (real GDScript scoping). Its call
                // result is the seam: `Ty::Callable` carries no signature, and `Variant` here
                // would fire false `INFERENCE_ON_VARIANT` on `var x := f()`.
                let ret = if self.locals.contains_key(name.as_str()) {
                    self.used_locals.insert(name.clone());
                    Ty::Unknown
                } else if let Some(t) = self.resolve_call_name(&name) {
                    t
                } else {
                    // Every same-file tier missed. With a COMPLETE workspace this is provably a
                    // typo (`usseState(0)`) unless a cross-file global/autoload/member exists —
                    // `UNDEFINED_FUNCTION` (BUG A1); otherwise the silent seam as before.
                    self.maybe_warn_undefined_call(callee, &name);
                    Ty::Unknown
                };
                self.expr_ty.insert(callee, Ty::Callable);
                ret
            }
            // Calling an arbitrary expression — a `Callable` value or an immediately-invoked
            // lambda (`(func(): …).call()`): the callee's return type isn't tracked, so the
            // result is the seam (not `Variant`), and `var x := f()()` never warns.
            _ => {
                self.infer_expr(callee, &Expectation::None);
                Ty::Unknown
            }
        };
        // UNSAFE_CALL_ARGUMENT (Phase-2 §5): args + receiver are now inferred (in `expr_ty`), so
        // check each argument against the statically-resolved callee's parameter types.
        self.check_call_args(callee, args);
        ret
    }

    /// Raise `UNSAFE_CALL_ARGUMENT` for each argument whose static type needs an unsafe implicit
    /// cast (`Variant` / a downcast) into the resolved parameter type — Godot's per-argument
    /// value-prop warning. Only fires when the callee resolves to a concrete signature here; an
    /// uninformative argument (the cross-file seam) is `Assign::Ok` and correctly silent, and an
    /// untyped parameter accepts anything.
    fn check_call_args(&mut self, callee: ExprId, args: &[ExprId]) {
        let Some(params) = self.call_param_tys(callee) else {
            return;
        };
        for (i, &arg) in args.iter().enumerate() {
            let Some(param_ty) = params.get(i) else {
                break; // a vararg tail or an arity mismatch — not an argument-type concern
            };
            if param_ty.is_uninformative() || param_ty.is_variant() {
                continue; // an untyped parameter accepts anything safely
            }
            // A missing arg type defaults to the seam (never warns), not `Variant` (would warn).
            let arg_ty = self.expr_ty.get(&arg).cloned().unwrap_or(Ty::Unknown);
            if ty::is_assignable(self.api, &arg_ty, param_ty) == Assign::OkUnsafe {
                let pl = param_ty.label(self.api).unwrap_or_else(|| "?".to_owned());
                let al = arg_ty.label(self.api).unwrap_or_else(|| "?".to_owned());
                self.warn(
                    self.range_of(arg),
                    WarningCode::UnsafeCallArgument,
                    format!(
                        "The argument {} requires a value of type \"{pl}\" but is passed \"{al}\", which is unsafe.",
                        i + 1
                    ),
                );
            }
        }
    }

    /// Parameter types of a statically-resolved callee, for [`Self::check_call_args`]. `None` when
    /// the callee isn't concretely resolvable here (a cross-file script method — params aren't
    /// modeled —, a builtin/utility, a `Callable` value): those raise no argument warning.
    fn call_param_tys(&self, callee: ExprId) -> Option<Vec<Ty>> {
        match self.body.expr(callee) {
            Expr::Name(name) => self.name_call_param_tys(name),
            Expr::Field { receiver, name, .. } => match self.expr_ty.get(receiver)? {
                Ty::Object(class) => match self.api.lookup_member(*class, name)? {
                    MemberRef::Method(sig) => Some(
                        sig.params
                            .iter()
                            .map(|p| ty::resolve_tyref(self.api, &p.ty))
                            .collect(),
                    ),
                    _ => None,
                },
                // ScriptRef / builtin / seam receivers: params not uniformly modeled — skip.
                _ => None,
            },
            _ => None,
        }
    }

    /// Parameter types for a bare-name call (`foo(...)` / an inherited `method(...)`): an own `func`
    /// first, then the `self` engine base's method. Utilities/builtins are skipped (looser, often
    /// variadic typing — out of the conservative MVP slice).
    fn name_call_param_tys(&self, name: &str) -> Option<Vec<Ty>> {
        // A local/param shadows a same-named method for a bare-name call (BUG A2, mirroring
        // `infer_call`'s locals-first resolution) — the local `Callable`'s params aren't modeled,
        // so the shadowed method's signature must not arg-check the call.
        if self.locals.contains_key(name) {
            return None;
        }
        if let Some(item) = self.class.lookup(name)
            && let Some(Member::Func(f)) = self.class.member(item)
        {
            return Some(
                f.params
                    .iter()
                    .map(|p| {
                        p.type_ref.as_deref().map_or(Ty::Variant, |t| {
                            resolve::resolve_type_name(self.db, self.api, t)
                        })
                    })
                    .collect(),
            );
        }
        if let Ty::Object(base) = self.class.base
            && let Some(MemberRef::Method(sig)) = self.api.lookup_member(base, name)
        {
            return Some(
                sig.params
                    .iter()
                    .map(|p| ty::resolve_tyref(self.api, &p.ty))
                    .collect(),
            );
        }
        None
    }

    /// Resolve a bare-name call (`foo(...)`): own method → utility/builtin fn → constructor.
    /// A local/param callee never reaches here — `infer_call`'s `Expr::Name` arm resolves locals
    /// first (BUG A2; this fn is `&self` and could not record the read into `used_locals`).
    /// `None` when every tier missed — the caller decides between the silent `Ty::Unknown` seam
    /// (cross-file / incomplete workspace) and `UNDEFINED_FUNCTION` (BUG A1; this fn is `&self`
    /// and cannot emit).
    fn resolve_call_name(&self, name: &str) -> Option<Ty> {
        if let Some(item) = self.class.lookup(name)
            && let Some(Member::Func(f)) = self.class.member(item)
        {
            return Some(self.func_call_return_ty(f));
        }
        // A bare call inside the class is `self.name(...)` — resolve against the inherited base.
        if let Ty::Object(base) = self.class.base
            && let Some(MemberRef::Method(sig)) = self.api.lookup_member(base, name)
        {
            return Some(ty::resolve_tyref(self.api, &sig.return_ty));
        }
        if let Some(u) = self.api.utility(name) {
            return Some(ty::resolve_tyref(self.api, &u.return_ty));
        }
        if let Some(f) = self.api.gdscript_builtin(name) {
            return Some(resolve::layer_to_ty(self.api, f.ret));
        }
        // A builtin / class name used as a constructor: `Vector2(...)` / `Array(...)`.
        // Normalize via `resolve_tyref` so `Array`/`Dictionary`/`Callable`/`Signal` land on
        // their dedicated `Ty` variants rather than `Builtin(...)`.
        if let Some(b) = self.api.builtin_by_name(name) {
            return Some(ty::resolve_tyref(self.api, &TyRef::Builtin(b)));
        }
        // Unresolved — a cross-file global / autoload / a method on a `class_name` base we can't
        // see, or a genuine typo. The caller disambiguates.
        None
    }

    /// The soundness gate for the absence-based `UNDEFINED_*` codes (BUG A1). Emitting "defined
    /// nowhere" requires being able to SEE everywhere a definition could live, so all of:
    /// - the loader declared the workspace COMPLETE ([`gdscript_db::SourceRoot::complete`]) — a
    ///   lone file (or a partial load) can never prove absence of a cross-file `class_name`;
    /// - this is a top-level script class (`self_ty` is a `ScriptRef`) — an inner class's bare
    ///   names may bind to outer-class members this scope's lookup does not chain to;
    /// - the base chain is fully engine-native (`Ty::Object`) — a user-script base (`ScriptRef`)
    ///   or an unresolved `extends` could hide an inherited definition the walk can't rule out;
    /// - the project does not target an engine NEWER than the bundled API model — a newer minor
    ///   adds classes/utilities the model can't rule out (corpus: `DrawableTexture2D` on a 4.5
    ///   model). An undeclared version is trusted at the bundled model's own version.
    fn undefined_symbols_provable(&self) -> bool {
        matches!(self.self_ty, Ty::ScriptRef(_))
            && matches!(self.class.base, Ty::Object(_))
            && self.db.source_root().is_some_and(|r| r.complete(self.db))
            && crate::queries::project_engine_version(self.db)
                .is_none_or(|v| v <= crate::warnings::bundled_version())
    }

    /// The trailing `name`-sized slice of `id`'s range — the exact identifier token. An
    /// `Expr::Name` node's range may include LEADING trivia (the CST attaches it to the node) but
    /// always ends at the identifier's last byte, so anchoring on the tail yields a precise
    /// squiggle instead of one that bleeds onto the previous line.
    fn name_token_range(&self, id: ExprId, name: &str) -> TextRange {
        let r = self.range_of(id);
        let len = u32::try_from(name.len()).unwrap_or(0);
        TextRange::new(r.end.saturating_sub(len).max(r.start), r.end)
    }

    /// `UNDEFINED_FUNCTION` (BUG A1) for a bare-name call whose every same-file tier already
    /// missed (`resolve_call_name` returned `None`, and the name is not a local — the A2 branch).
    /// Cross-file tiers are checked here: a project `class_name`, a `*`-autoload, or an engine
    /// global keeps the silent seam (calling those is a *different* mistake than "undefined", and
    /// Godot reports it differently). A member of ANY kind (var/const/signal/enum/inner class) on
    /// the class or its engine base also keeps the seam — "defined but not callable" is not
    /// "undefined". Emission is gated by [`Self::undefined_symbols_provable`].
    fn maybe_warn_undefined_call(&mut self, callee: ExprId, name: &str) {
        if !self.undefined_symbols_provable() || self.class.lookup(name).is_some() {
            return;
        }
        if let Ty::Object(base) = self.class.base
            && self.api.lookup_member(base, name).is_some()
        {
            return;
        }
        if resolve::resolve_global(self.api, name).is_some()
            || is_registered_global_class(self.db, name)
            || is_autoload_singleton(self.db, name)
        {
            return;
        }
        self.warn(
            self.name_token_range(callee, name),
            WarningCode::UndefinedFunction,
            format!(
                "The function \"{name}()\" is not declared in this class, its base, or anywhere in the loaded project."
            ),
        );
    }

    /// `UNDEFINED_IDENTIFIER` (BUG A1) at `resolve_name`'s final fallthrough — every tier
    /// (locals, own members, the engine base, engine globals, the project `class_name` registry,
    /// autoload singletons) already missed structurally by the time this is called, so with the
    /// [`Self::undefined_symbols_provable`] gate the name is provably undeclared.
    fn maybe_warn_undefined_identifier(&mut self, id: ExprId, name: &str) {
        if !self.undefined_symbols_provable() {
            return;
        }
        self.warn(
            self.name_token_range(id, name),
            WarningCode::UndefinedIdentifier,
            format!(
                "The identifier \"{name}\" is not declared in the current scope or anywhere in the loaded project."
            ),
        );
    }

    fn func_return_ty(&self, annotation: Option<&str>) -> Ty {
        annotation.map_or(Ty::Variant, |t| {
            resolve::resolve_type_name(self.db, self.api, t)
        })
    }

    /// The return type of CALLING an own `func`: a `## @return-tuple(T0, T1, …)` doc-tag (BUG A3)
    /// wins — its positional element types become a [`Ty::Tuple`], so a constant index projects
    /// the element's real type — else the plain annotation.
    fn func_call_return_ty(&self, f: &crate::item_tree::FuncItem) -> Ty {
        if let Some(names) = &f.tuple_return {
            return Ty::Tuple(
                names
                    .iter()
                    .map(|n| resolve::resolve_type_name(self.db, self.api, n))
                    .collect(),
            );
        }
        self.func_return_ty(f.return_type.as_deref())
    }

    /// Member access `receiver.name`. When `as_method`, resolve a method (and use its return
    /// type); otherwise resolve a property/const/etc. Raises `UNSAFE_*` only on a statically
    /// **known** receiver.
    fn infer_field(
        &mut self,
        receiver: ExprId,
        name: &str,
        name_range: TextRange,
        as_method: bool,
    ) -> Ty {
        let is_self = matches!(self.body.expr(receiver), Expr::SelfExpr);
        let recv_ty = self.infer_expr(receiver, &Expectation::None);

        // `self.member` consults this file's own members first (Playbook §3.2).
        if is_self && let Some(item) = self.class.lookup(name) {
            return self.own_member_ty(item, as_method);
        }

        match &recv_ty {
            // Uninformative receivers are unchecked and **propagate the seam**: a member of an
            // `Unknown` (cross-file) value is itself `Unknown` (never warns), a member of a
            // `Variant` is `Variant`, of an `Error` is `Error`. Collapsing `Unknown` to
            // `Variant` here would wrongly fire `INFERENCE_ON_VARIANT` on `var x := other.field`.
            t if t.is_uninformative() => recv_ty.clone(),
            Ty::Object(class) => {
                if name == "new" {
                    // `Class.new(...)` always constructs an instance of the class (some classes,
                    // e.g. GDScript, also carry a modeled `new` member — the constructor wins).
                    recv_ty.clone()
                } else if let Some(m) = self.api.lookup_member(*class, name) {
                    self.check_member_kind_misuse(&m, as_method, name, name_range);
                    self.check_static_on_instance(receiver, &m, as_method, name_range);
                    self.member_ref_ty(&m, as_method)
                } else if let Some(t) = self.class_enum_value(*class, name) {
                    // A statically-accessed enum value (`Control.PRESET_FULL_RECT`).
                    t
                } else {
                    // Self with an Object base already checked own members above.
                    self.emit_unsafe(name, &recv_ty, name_range, as_method);
                    Ty::Variant
                }
            }
            // A tuple's members are its runtime `Array`'s (`size`/`append`/… via builtin_id_of).
            Ty::Builtin(_)
            | Ty::Array(_)
            | Ty::Tuple(_)
            | Ty::Dict(..)
            | Ty::Callable
            | Ty::Signal(_) => self.builtin_member_ty(&recv_ty, name, name_range, as_method),
            // Accessing a member of an enum namespace (`State.IDLE`) yields the enum type itself —
            // an enum value (freely int-assignable via `ty::is_assignable`). Was `int`, which lost
            // the enum type and false-`INFERENCE_ON_VARIANT`'d a same-file `var x := State.IDLE`.
            Ty::Enum(er) => Ty::Enum(er.clone()),
            // A cross-file script reference: resolve the member against its (own) member table.
            Ty::ScriptRef(sref) => self.script_member_ty(*sref, name, as_method),
            // An inner-class value/instance: resolve against its own item-tree + `extends` chain.
            Ty::InnerClass(iref) => self.inner_class_member_ty(iref, name, as_method),
            _ => Ty::Variant,
        }
    }

    /// Resolve `name` on an inner-class value/instance (`Ty::InnerClass`). `Inner.new()` constructs an
    /// instance (the same `InnerClass`); otherwise the inner class's own members (typed by their
    /// annotation — lossy, like the cross-file `ScriptRef` path: an inferred/unannotated member seams)
    /// then its `extends` chain. The seam (`Unknown`) for an unresolved member — never a false
    /// `UNSAFE_*`.
    fn inner_class_member_ty(
        &self,
        iref: &crate::ty::InnerClassRef,
        name: &str,
        as_method: bool,
    ) -> Ty {
        if name == "new" && as_method {
            return Ty::InnerClass(iref.clone());
        }
        self.inner_member_walk(iref, name, as_method, 0)
            .unwrap_or(Ty::Unknown)
    }

    /// Walk an inner class's own members, then its `extends` base (an engine class, a `class_name`, or
    /// another inner/script class), for `name`. Depth-bounded like [`script_member_walk`].
    fn inner_member_walk(
        &self,
        iref: &crate::ty::InnerClassRef,
        name: &str,
        as_method: bool,
        depth: u32,
    ) -> Option<Ty> {
        if depth > 32 {
            return None;
        }
        let ft = self.db.file_text(FileId(iref.file))?;
        let tree = crate::queries::item_tree(self.db, ft);
        let inner = find_inner_class(&tree, &iref.path)?;
        if let Some(m) = inner.tree.member(name) {
            return self.inner_member_item_ty(m, as_method, iref);
        }
        // Not an own member — walk the inner class's `extends` base.
        let res_path = self.self_res_path();
        match resolve::resolve_base(self.db, self.api, &inner.tree, res_path.as_deref()) {
            Ty::Object(class) => self
                .api
                .lookup_member(class, name)
                .map(|m| self.member_ref_ty(&m, as_method)),
            Ty::ScriptRef(base) => self.script_member_walk(base, name, as_method, depth + 1),
            Ty::InnerClass(base) => self.inner_member_walk(&base, name, as_method, depth + 1),
            _ => None,
        }
    }

    /// Type an inner class's own member by its written **annotation** (the inner body isn't inferred
    /// here — Increment 2 adds that). An unannotated `var`/`const` or an untyped `func` return seams.
    fn inner_member_item_ty(
        &self,
        m: &Member,
        as_method: bool,
        iref: &crate::ty::InnerClassRef,
    ) -> Option<Ty> {
        Some(match m {
            Member::Func(f) => {
                if as_method {
                    f.return_type.as_deref().map_or(Ty::Variant, |t| {
                        resolve::resolve_type_name(self.db, self.api, t)
                    })
                } else {
                    Ty::Callable
                }
            }
            Member::Var(v) => resolve::resolve_type_name(self.db, self.api, v.type_ref.as_deref()?),
            Member::Const(c) => {
                resolve::resolve_type_name(self.db, self.api, c.type_ref.as_deref()?)
            }
            Member::Signal(_) => Ty::Signal(None),
            Member::Enum(e) => Ty::Enum(EnumRef {
                qualified: e.name.clone()?,
                bitfield: false,
            }),
            // A nested inner class → `Ty::InnerClass` with the extended dotted path.
            Member::Class(c) => Ty::InnerClass(crate::ty::InnerClassRef {
                file: iref.file,
                path: SmolStr::new(format!("{}.{}", iref.path, c.name)),
            }),
        })
    }

    /// A member of a cross-file script (`ScriptRef`): looked up in the script's own member table
    /// (M1). A member we don't model — e.g. one inherited from a base we don't resolve until M2 —
    /// yields the seam (`Unknown`), **never** an `UNSAFE_*` warning. `Class.new(...)` constructs
    /// an instance of the class.
    fn script_member_ty(&self, sref: ScriptRefId, name: &str, as_method: bool) -> Ty {
        if name == "new" {
            return Ty::ScriptRef(sref);
        }
        self.script_member_walk(sref, name, as_method, 0)
            .unwrap_or(Ty::Unknown)
    }

    /// Walk a script class's `extends` chain for `name`: own members first, then a user base
    /// (another `ScriptRef`), then an engine base (the API table). Depth-bounded so a cyclic
    /// `extends` cannot loop. `None` = not found anywhere in the chain (the seam).
    fn script_member_walk(
        &self,
        sref: ScriptRefId,
        name: &str,
        as_method: bool,
        depth: u32,
    ) -> Option<Ty> {
        if depth > 32 {
            return None;
        }
        let file = self.db.file_text(FileId(sref.0))?;
        let sc = crate::queries::script_class(self.db, file);
        if let Some(m) = sc.member(name) {
            return Some(match m {
                crate::queries::MemberSig::Method(ret) => {
                    if as_method {
                        ret.clone()
                    } else {
                        Ty::Callable
                    }
                }
                crate::queries::MemberSig::Field(t) => t.clone(),
                crate::queries::MemberSig::Signal => Ty::Signal(None),
            });
        }
        // Not an own member — continue up the inheritance chain.
        match sc.base() {
            Ty::ScriptRef(base) => self.script_member_walk(*base, name, as_method, depth + 1),
            Ty::Object(class) => self
                .api
                .lookup_member(*class, name)
                .map(|m| self.member_ref_ty(&m, as_method)),
            _ => None,
        }
    }

    /// Whether a value of type `sub` is statically a subtype of `sup` — composing user `ScriptRef`
    /// `extends` chains with the engine class table (M4, for `is`/`as` widen-only narrowing). A
    /// `ScriptRef` IS-A its native base (so `script_value is Node` holds), but Godot's asymmetry is
    /// honored: a native/script value is **not** a subtype of an *unrelated* user script.
    fn is_subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        match (sub, sup) {
            (Ty::Object(a), Ty::Object(b)) => self.api.is_subclass(*a, *b),
            (Ty::ScriptRef(a), Ty::ScriptRef(b)) => self.script_is_subtype(*a, *b, 0),
            (Ty::ScriptRef(a), Ty::Object(b)) => self.script_extends_engine(*a, *b, 0),
            _ => false,
        }
    }

    /// Whether script `sub` is `sup` or transitively extends it — walk the `extends` base chain by
    /// script identity (depth-bounded, like [`script_member_walk`](Self::script_member_walk)).
    fn script_is_subtype(&self, sub: ScriptRefId, sup: ScriptRefId, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        if sub == sup {
            return true;
        }
        let Some(file) = self.db.file_text(FileId(sub.0)) else {
            return false;
        };
        match crate::queries::script_class(self.db, file).base() {
            Ty::ScriptRef(base) => self.script_is_subtype(*base, sup, depth + 1),
            _ => false,
        }
    }

    /// Whether script `sub`'s `extends` chain reaches engine class `sup_native` at its native base.
    fn script_extends_engine(
        &self,
        sub: ScriptRefId,
        sup_native: gdscript_api::ClassId,
        depth: u32,
    ) -> bool {
        if depth > 32 {
            return false;
        }
        let Some(file) = self.db.file_text(FileId(sub.0)) else {
            return false;
        };
        match crate::queries::script_class(self.db, file).base() {
            Ty::ScriptRef(base) => self.script_extends_engine(*base, sup_native, depth + 1),
            Ty::Object(native) => self.api.is_subclass(*native, sup_native),
            _ => false,
        }
    }

    fn emit_unsafe(&mut self, name: &str, recv: &Ty, range: TextRange, as_method: bool) {
        let recv_label = recv.label(self.api).unwrap_or_else(|| "?".to_owned());
        let (code, message) = if as_method {
            (
                WarningCode::UnsafeMethodAccess,
                format!(
                    "The method \"{name}()\" is not present on the inferred type \"{recv_label}\" (but may be present on a subtype)."
                ),
            )
        } else {
            (
                WarningCode::UnsafePropertyAccess,
                format!(
                    "The property \"{name}\" is not present on the inferred type \"{recv_label}\" (but may be present on a subtype)."
                ),
            )
        };
        self.warn(range, code, message);
    }

    /// Whether the class's RESOLVED **engine** base declares a *value* member (var/const/signal)
    /// named `name` — the sound floor for `SHADOWED_VARIABLE_BASE_CLASS`. Only the engine base is
    /// consulted: an unresolved base (the cross-file seam) returns `false` (no warning), and the
    /// cross-file *user*-base `MemberSig` is lossy (no kind detail) so user-base shadowing stays
    /// deferred (see `TECH_DEBT.md`). Methods are excluded (matches the own-member shadow rule).
    fn engine_base_has_value_member(&self, name: &str) -> bool {
        let Ty::Object(base) = &self.class.base else {
            return false;
        };
        matches!(
            self.api.lookup_member(*base, name),
            Some(MemberRef::Property(_) | MemberRef::Const(_) | MemberRef::Signal(_))
        )
    }

    /// Flag a deprecated member-kind misuse on a statically-resolved engine member:
    /// `PROPERTY_USED_AS_FUNCTION` / `CONSTANT_USED_AS_FUNCTION` when a property/const is *called*.
    /// Guarded against a Callable/Signal/uninformative-typed member (those can legitimately be
    /// invoked). `FUNCTION_USED_AS_PROPERTY` is intentionally NOT emitted — a bare `obj.method` is an
    /// idiomatic `Callable` reference (every signal `.connect`), indistinguishable from a misuse
    /// without call-context, so it would false-positive everywhere (see `TECH_DEBT.md`).
    fn check_member_kind_misuse(
        &mut self,
        m: &MemberRef,
        as_method: bool,
        name: &str,
        range: TextRange,
    ) {
        if !as_method {
            return;
        }
        let (code, kind, ty) = match m {
            MemberRef::Property(p) => (
                WarningCode::PropertyUsedAsFunction,
                "property",
                ty::resolve_tyref(self.api, &p.ty),
            ),
            MemberRef::Const(c) => (
                WarningCode::ConstantUsedAsFunction,
                "constant",
                ty::resolve_tyref(self.api, &c.ty),
            ),
            _ => return,
        };
        // A Callable/Signal-typed (or uninformative) member can be invoked — never flag it.
        if ty.is_uninformative() || matches!(ty, Ty::Callable | Ty::Signal(_)) {
            return;
        }
        self.warn(
            range,
            code,
            format!("The {kind} \"{name}\" is being called as if it were a function."),
        );
    }

    /// Flag `STATIC_CALLED_ON_INSTANCE`: an engine static method called through an instance value
    /// rather than the type. Conservative + sound — fires only when the receiver is a **typed local
    /// instance** (a `Name` bound in `locals`), never a bare class name (`Class.static()` is
    /// correct) nor an expression we can't classify. Under-warns by design; zero false positives.
    fn check_static_on_instance(
        &mut self,
        receiver: ExprId,
        m: &MemberRef,
        as_method: bool,
        range: TextRange,
    ) {
        if !as_method {
            return;
        }
        let MemberRef::Method(sig) = m else {
            return;
        };
        if !sig.is_static {
            return;
        }
        let Expr::Name(rname) = self.body.expr(receiver) else {
            return;
        };
        if !self.locals.contains_key(rname) {
            return;
        }
        // A local that ALIASES a type/var (`var t := JSON; t.stringify()`) is not an instance —
        // calling a static method through it is valid (`t` holds the type, not an object). A bare
        // `Name` initializer marks such an alias; only a constructor/call init (or a param/field
        // with no init) is a true instance. Skipping the alias case fixes a false positive.
        if let Some(b) = self.bindings.iter().rev().find(|b| &b.name == rname)
            && let Some(init) = b.init
            && matches!(self.body.expr(init), Expr::Name(_))
        {
            return;
        }
        self.warn(
            range,
            WarningCode::StaticCalledOnInstance,
            "A static method is being called on an instance; call it on the type instead."
                .to_owned(),
        );
    }

    fn member_ref_ty(&self, m: &MemberRef, as_method: bool) -> Ty {
        match m {
            MemberRef::Method(sig) => {
                if as_method {
                    ty::resolve_tyref(self.api, &sig.return_ty)
                } else {
                    Ty::Callable
                }
            }
            MemberRef::Property(p) => p.enum_of.as_ref().map_or_else(
                || ty::resolve_tyref(self.api, &p.ty),
                |q| {
                    Ty::Enum(EnumRef {
                        qualified: SmolStr::new(q),
                        bitfield: false,
                    })
                },
            ),
            MemberRef::Const(c) => ty::resolve_tyref(self.api, &c.ty),
            MemberRef::Signal(_) => Ty::Signal(None),
            MemberRef::Enum(_) => Ty::Variant,
            // A class enum's VALUE (`Control.SIZE_EXPAND_FILL` / bare in a subclass): its
            // declaring ENUM type (mirroring `class_enum_value`), so an enum member into its own
            // enum slot stays `Assign::Ok` — never a false `INT_AS_ENUM_WITHOUT_CAST`. (Still
            // freely assignable to `int` via `ty::is_assignable`.)
            MemberRef::EnumValue { class, decl, .. } => Ty::Enum(EnumRef {
                qualified: SmolStr::new(format!("{}.{}", class, decl.name)),
                bitfield: decl.is_bitfield,
            }),
        }
    }

    fn builtin_member_ty(
        &mut self,
        recv: &Ty,
        name: &str,
        range: TextRange,
        as_method: bool,
    ) -> Ty {
        let Some(bid) = self.builtin_id_of(recv) else {
            return Ty::Variant;
        };
        if as_method {
            return if let Some(sig) = self.api.builtin_method(bid, name) {
                ty::resolve_tyref(self.api, &sig.return_ty)
            } else {
                self.emit_unsafe(name, recv, range, true);
                Ty::Variant
            };
        }
        if let Some(member) = self.api.builtin_member(bid, name) {
            return ty::resolve_tyref(self.api, &member.ty);
        }
        // Static constants (`Vector2.ZERO`, `Color.WHITE`) and enum values (`Variant.Type.*`).
        let data = self.api.builtin(bid);
        if let Some(c) = data.constants.iter().find(|c| c.name == name) {
            return ty::resolve_tyref(self.api, &c.ty);
        }
        if data
            .enums
            .iter()
            .any(|e| e.values.iter().any(|v| v.name == name))
        {
            return self.int_ty();
        }
        if self.api.builtin_method(bid, name).is_some() {
            return Ty::Callable;
        }
        self.emit_unsafe(name, recv, range, false);
        Ty::Variant
    }

    /// The type of a class enum **value** accessed statically (`Control.PRESET_FULL_RECT`):
    /// the engine exposes enum values as class members, so search every (inherited) enum's
    /// values. Returns the value's **declaring enum type** (`Ty::Enum`) — mirroring how a
    /// `Class.Enum` *annotation* resolves (`resolve::resolve_named`), so an enum member assigned
    /// to a slot of that same enum is `Assign::Ok`, not a false `INT_AS_ENUM_WITHOUT_CAST`. (An
    /// enum value is still freely assignable to `int` — see `ty::is_assignable`.)
    fn class_enum_value(&self, class: gdscript_api::ClassId, name: &str) -> Option<Ty> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.api.class(cid);
            if let Some(e) = c
                .enums
                .iter()
                .find(|e| e.values.iter().any(|v| v.name == name))
            {
                return Some(Ty::Enum(EnumRef {
                    qualified: SmolStr::new(format!("{}.{}", c.name, e.name)),
                    bitfield: e.is_bitfield,
                }));
            }
            cur = c.base;
        }
        None
    }

    /// The builtin id backing a builtin / `Array` / `Dictionary` receiver.
    fn builtin_id_of(&self, ty: &Ty) -> Option<gdscript_api::BuiltinId> {
        match ty {
            Ty::Builtin(b) => Some(*b),
            // A tuple IS an `Array` at runtime — its methods (`size`/`append`/…) are Array's.
            Ty::Array(_) | Ty::Tuple(_) => self.api.builtin_by_name("Array"),
            Ty::Dict(..) => self.api.builtin_by_name("Dictionary"),
            Ty::Callable => self.api.builtin_by_name("Callable"),
            Ty::Signal(_) => self.api.builtin_by_name("Signal"),
            _ => None,
        }
    }

    /// The element type of an indexing expression (Playbook §2 switch). `index` is the subscript
    /// expression — a CONSTANT integer literal selects a [`Ty::Tuple`]'s positional element type
    /// (BUG A3: `useState(...)[1]` is the setter `Callable`, not `Variant`).
    fn index_ty(&self, base: &Ty, index: ExprId) -> Ty {
        match base {
            Ty::Array(elem) => (**elem).clone(),
            // A tuple projects the element at a constant in-bounds index; a dynamic or
            // out-of-bounds index degrades to the runtime element type (`Variant`), exactly like
            // the untyped `Array` the tuple is at runtime.
            Ty::Tuple(elems) => match self.body.expr(index) {
                Expr::Literal(body::Literal::Int(Some(i))) => usize::try_from(*i)
                    .ok()
                    .and_then(|i| elems.get(i).cloned())
                    .unwrap_or(Ty::Variant),
                _ => Ty::Variant,
            },
            Ty::Builtin(b) => self
                .api
                .builtin(*b)
                .indexing_return
                .as_ref()
                .map_or(Ty::Variant, |r| ty::resolve_tyref(self.api, r)),
            // Indexing through the seam stays on the seam (never warns).
            Ty::Unknown => Ty::Unknown,
            Ty::Error => Ty::Error,
            _ => Ty::Variant,
        }
    }

    /// The loop variable's type for `for v in iter:` (Playbook §2 switch).
    fn loop_var_ty(&self, iter: &Ty) -> Ty {
        match iter {
            Ty::Array(elem) => (**elem).clone(),
            // Iterating a tuple visits its runtime array's elements (`Variant` — positions are
            // meaningless during iteration).
            Ty::Tuple(_) => Ty::Variant,
            Ty::Builtin(b) => {
                let data = self.api.builtin(*b);
                if data.name == "int" {
                    // `for i in 5` / `for i in range(...)` → int.
                    self.int_ty()
                } else if let Some(r) = &data.indexing_return {
                    // `for c in "abc"` → String; `for s in packed_string_array` → String; …
                    ty::resolve_tyref(self.api, r)
                } else {
                    Ty::Variant
                }
            }
            // Iterating a seam value keeps the loop var on the seam (never warns).
            Ty::Unknown => Ty::Unknown,
            Ty::Error => Ty::Error,
            _ => Ty::Variant,
        }
    }

    fn infer_lambda(&mut self, params: &[ParamBinding], body: &[body::StmtId]) {
        // Lambda params shadow within the body; restore the outer locals afterward. A `return`
        // inside the lambda is the *lambda's* return, not the enclosing function's — so disable
        // return checking (set the expected return to `Variant`) while walking the body.
        let saved_locals = self.locals.clone();
        let saved_ret = std::mem::replace(&mut self.return_ty, Ty::Variant);
        for p in params {
            let ty = self.param_ty(p);
            self.bindings.push(Binding {
                name: p.name.clone(),
                name_range: p.name_range,
                ty: ty.clone(),
                init: None,
                annotated: p.type_ref.is_some(),
                inferred_colon_eq: false,
                is_const: false,
                kind: BindingKind::Param,
            });
            self.locals.insert(p.name.clone(), ty);
        }
        self.infer_block(body);
        self.return_ty = saved_ret;
        self.locals = saved_locals;
    }

    fn param_ty(&mut self, p: &ParamBinding) -> Ty {
        if let Some(ptr) = p.type_ref {
            return self.resolve_ptr_ty(ptr);
        }
        // An unannotated param infers from its default, else `Variant`.
        p.default
            .map_or(Ty::Variant, |e| self.infer_expr(e, &Expectation::None))
    }

    // ---- name resolution (local → class member → inherited → global) ----

    /// The `Ty`-producing half of the bare-name lookup. Its precedence is the **canonical order**
    /// documented on [`crate::def::resolve_name_to_def`] (local → own member → inherited member →
    /// engine global → `class_name` global → autoload) — kept in lockstep with that identity-producing
    /// copy by the `classify_and_infer_agree_*` tests (gdscript-ide). Unlike that copy, this one is
    /// woven with flow-narrowing and the `UNUSED`/`UNASSIGNED` side-effects (it runs mid-inference),
    /// which is why the two are intentionally separate functions rather than one.
    fn resolve_name(&mut self, id: ExprId, name: &str) -> Ty {
        // Record a *read* of a local/param for the `UNUSED_*` analysis (before the narrowing check,
        // so a narrowed read still counts as used). The direct LHS of an assignment (`x = …`) is a
        // WRITE, not a read — excluding it lets `UNUSED_VARIABLE` catch an assigned-but-never-read
        // local (Godot's precise behaviour). A compound `x += …` still reads `x` via its RHS NameRef
        // (a distinct expr), and a receiver / index target (`x.f()`, `x[i] = …`) is a read of `x`.
        if self.locals.contains_key(name) && !self.assign_lhs.contains(&id) {
            self.used_locals.insert(SmolStr::new(name));
        }
        // UNASSIGNED_VARIABLE (Workstream 2) — a *read* of a typed-no-init local that is not
        // definitely assigned on every path reaching here. Excludes the LHS of an assignment (a
        // write) and reads inside a lambda body (which `assigned_before` leaves `None`, unchecked).
        if self.is_func_body
            && self.needs_assignment.contains(name)
            && !self.assign_lhs.contains(&id)
            && let Some(cur) = self.cur_stmt
            && self
                .assigned
                .assigned_before(cur)
                .is_some_and(|a| !a.contains(name))
        {
            self.warn(
                self.range_of(id),
                WarningCode::UnassignedVariable,
                format!("The variable \"{name}\" may be used before it is assigned a value."),
            );
        }
        // Flow narrowing wins over the binding's declared type.
        if let Some(key) = self.narrow_key(id)
            && let Some(t) = self.narrowing.get(&key)
        {
            return t.clone();
        }
        if let Some(t) = self.locals.get(name) {
            return t.clone();
        }
        if let Some(item) = self.class.lookup(name) {
            return self.own_member_ty(item, false);
        }
        // Inherited members: an engine `Object` base via the API table, or a user `ScriptRef`
        // base via the script member walk (M2 — so a class extending another class_name sees its
        // inherited members).
        match self.class.base.clone() {
            Ty::Object(base) => {
                if let Some(m) = self.api.lookup_member(base, name) {
                    return self.member_ref_ty(&m, false);
                }
            }
            Ty::ScriptRef(base) => {
                if let Some(t) = self.script_member_walk(base, name, false, 0) {
                    return t;
                }
            }
            _ => {}
        }
        if let Some(g) = resolve::resolve_global(self.api, name) {
            return global_ty(&g);
        }
        // A project-global `class_name` used as a value — the class itself, for static access
        // (`V.fc()`) or as a constructor (`Player.new()`). Resolves to a `ScriptRef` via the
        // registry. Precedence (Godot `reduce_identifier`): `class_name` global ≫ autoload
        // singleton. So try `class_name` first, then a `*`-autoload, then the seam.
        let by_class = resolve::resolve_external(
            self.db,
            &resolve::ExternalRef::ClassName(SmolStr::new(name)),
        );
        if !by_class.is_unknown() {
            return by_class;
        }
        let by_autoload =
            resolve::resolve_external(self.db, &resolve::ExternalRef::Autoload(SmolStr::new(name)));
        if by_autoload.is_unknown() {
            // EVERY tier missed. With a complete workspace this name is provably undeclared —
            // `UNDEFINED_IDENTIFIER` (BUG A1); otherwise the silent seam as before.
            self.maybe_warn_undefined_identifier(id, name);
        }
        by_autoload
    }

    fn own_member_ty(&self, item: ClassItem, as_method: bool) -> Ty {
        match item {
            ClassItem::EnumVariant => self.int_ty(),
            ClassItem::Member(_) => match self.class.member(item) {
                Some(Member::Var(v)) => self.field_ty(&v.name, v.ptr),
                Some(Member::Const(c)) => self.field_ty(&c.name, c.ptr),
                Some(Member::Func(f)) => {
                    if as_method {
                        self.func_call_return_ty(f)
                    } else {
                        Ty::Callable
                    }
                }
                Some(Member::Signal(_)) => Ty::Signal(None),
                // An inner `class Name:` used as a value → `Ty::InnerClass` (was the `Unknown` seam),
                // so `Inner.CONST` / `Inner.new()` / a typed instance's members resolve against its own
                // item-tree. The path is the inner class's name (resolved from the top-level scope;
                // nested inner classes get their dotted path once inner bodies are inferred).
                Some(Member::Class(c)) => match &self.self_ty {
                    Ty::ScriptRef(sref) => Ty::InnerClass(crate::ty::InnerClassRef {
                        file: sref.0,
                        path: c.name.clone(),
                    }),
                    _ => Ty::Unknown,
                },
                // A same-file named `enum State` used as a value/namespace → the enum type, so
                // `State.IDLE` (member access below) types as `State`, not a false-`INFERENCE_ON_
                // VARIANT` seam. An anonymous enum has no namespace name (its variants are direct
                // class constants), so it stays the seam.
                Some(Member::Enum(e)) => e.name.as_ref().map_or(Ty::Variant, |n| {
                    Ty::Enum(EnumRef {
                        qualified: n.clone(),
                        bitfield: false,
                    })
                }),
                None => Ty::Variant,
            },
        }
    }

    /// The type of an own field (`var`/`const`): the type seeded by the field pre-pass (which
    /// captures the inferred type of `var n := 0`), falling back to the written annotation.
    fn field_ty(&self, name: &str, ptr: AstPtr) -> Ty {
        if let Some(t) = self.class.member_types.get(name) {
            return t.clone();
        }
        self.resolve_decl_annotation(ptr)
    }

    /// Resolve a declaration's annotation (recovering its `TypeRef` node), else `Variant`.
    fn resolve_decl_annotation(&self, ptr: AstPtr) -> Ty {
        let Some(node) = ptr.to_node(self.root) else {
            return Ty::Variant;
        };
        cst::first_child(&node, |k| k == gdscript_syntax::SyntaxKind::TypeRef)
            .map_or(Ty::Variant, |t| {
                resolve::resolve_type_ref(self.db, self.api, &t)
            })
    }

    // ---- narrowing ----

    /// Build the narrowing env for a statement from the precomputed flow facts (Workstream 2).
    ///
    /// Only `Is` facts contribute a type (`NotNull`/`Not` are recorded by the flow pass but not yet
    /// consumed for typing — the 1.0 cut). The **widen-only + `is_uninformative`** soundness gate is
    /// preserved verbatim from the old `apply_narrowing`: `is`-narrowing is a deliberate divergence
    /// from upstream Godot (whose `is` does not flow-narrow), kept widen-only so it never produces a
    /// type Godot would reject — narrow only when the tested type is a downcast of the place's
    /// declared type, or the declared type is uninformative; never un-narrow a known subtype
    /// (`d: Derived; if d is Base` keeps `Derived`), never narrow to a type we couldn't resolve.
    fn facts_to_narrowing(&self, id: body::StmtId) -> FxHashMap<String, Ty> {
        let mut out = FxHashMap::default();
        if let Some(facts) = self.flow.facts_before(id) {
            for (place, nt) in facts.iter() {
                if let Some((key, ty)) = self.narrowing_entry(place, nt) {
                    out.insert(key, ty);
                }
            }
        }
        out
    }

    /// Resolve one flow fact into a `(dotted-key, narrowed-type)` narrowing entry, applying the
    /// widen-only + `is_uninformative` soundness gate. `None` if the fact doesn't narrow a type
    /// (a `NotNull`/`Not`, an unresolvable/uninformative type, or an un-narrowing of a known subtype).
    fn narrowing_entry(&self, place: &Place, nt: &NarrowedTy) -> Option<(String, Ty)> {
        let NarrowedTy::Is(ptr) = nt else {
            return None;
        };
        let narrowed = self.resolve_ptr_ty(*ptr);
        if narrowed.is_uninformative() {
            return None;
        }
        // Gate against a local/param's declared type; for `self`-members / field chains the
        // `is_uninformative` check above is the soundness floor.
        if let Place::Local(n) = place
            && let Some(cur) = self.locals.get(n)
            && !cur.is_uninformative()
            && !self.is_subtype(&narrowed, cur)
        {
            return None;
        }
        Some((place.dotted_key(), narrowed))
    }

    /// Apply a condition's short-circuit narrowing to the active env, for typing the RHS of an
    /// `and`/`or` (Workstream 2): `if x is T and x.method():` narrows `x` for `x.method()`.
    fn apply_condition_facts(&mut self, cond: ExprId, truthy: bool) {
        for (place, nt) in flow::condition_facts(self.body, cond, truthy) {
            if let Some((key, ty)) = self.narrowing_entry(&place, &nt) {
                self.narrowing.insert(key, ty);
            }
        }
    }

    /// A dotted access-path key for narrowing (`x`, `self.field`, `a.b.c`), or `None` for a
    /// non-path expression.
    fn narrow_key(&self, id: ExprId) -> Option<String> {
        match self.body.expr(id) {
            Expr::Name(n) => Some(n.to_string()),
            Expr::SelfExpr => Some("self".to_owned()),
            Expr::Paren(inner) => self.narrow_key(*inner),
            Expr::Field { receiver, name, .. } => {
                Some(format!("{}.{name}", self.narrow_key(*receiver)?))
            }
            _ => None,
        }
    }

    fn resolve_ptr_ty(&self, ptr: AstPtr) -> Ty {
        ptr.to_node(self.root).map_or(Ty::Variant, |n| {
            resolve::resolve_type_ref(self.db, self.api, &n)
        })
    }

    // ---- helpers ----

    /// The join (least upper bound) of two branch types — conservative: equal types collapse,
    /// a subtype widens to its supertype, else `Variant`.
    ///
    /// The three uninformative markers do NOT collapse to `Variant` — that would defeat the
    /// seam. They propagate by priority: `Error` (already diagnosed) → `Unknown` (the cross-file
    /// seam — must never warn or cascade) → `Variant` (the gradual top). So
    /// `x if c else <unknown>` stays `Unknown`, and `var y := (x if c else unknown)` does not
    /// fire a false `INFERENCE_ON_VARIANT`.
    fn join(&self, a: &Ty, b: &Ty) -> Ty {
        if a == b {
            return a.clone();
        }
        if a.is_error() || b.is_error() {
            return Ty::Error;
        }
        if a.is_unknown() || b.is_unknown() {
            return Ty::Unknown;
        }
        if a.is_variant() || b.is_variant() {
            return Ty::Variant;
        }
        if ty::is_assignable(self.api, a, b) == Assign::Ok {
            return b.clone();
        }
        if ty::is_assignable(self.api, b, a) == Assign::Ok {
            return a.clone();
        }
        Ty::Variant
    }
}

/// Map a resolved global definition to the type of a bare reference to it.
fn global_ty(g: &GlobalDef) -> Ty {
    match g {
        GlobalDef::Const(t) => t.clone(),
        GlobalDef::Singleton(c) | GlobalDef::ClassType(c) => Ty::Object(*c),
        GlobalDef::BuiltinType(b) => Ty::Builtin(*b),
        // A bare function referenced as a value is a `Callable`; an enum namespace is opaque.
        GlobalDef::Builtin | GlobalDef::Utility => Ty::Callable,
        GlobalDef::GlobalEnum => Ty::Variant,
    }
}

fn inference_on_variant_msg(kind: &str) -> String {
    format!(
        "The {kind} type is being inferred from a Variant value, so it will be typed as Variant."
    )
}

/// The `extension_api.json` operator spelling for a binary operator.
fn op_symbol(op: BinOp) -> Option<&'static str> {
    Some(match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::item_tree;
    use gdscript_syntax::{SyntaxKind, parse};

    struct Harness {
        result: InferenceResult,
        body: Body,
    }

    /// Infer the (first) function in `src` against a fresh class scope.
    fn infer_first_func(src: &str) -> Harness {
        let api = gdscript_api::bundled();
        let db = gdscript_db::RootDatabase::default();
        let root = parse(src).syntax_node();
        let tree = item_tree(&root);
        let class = ClassScope::new(&db, api, &tree, None);
        let func = gdscript_syntax::ast::descendants(&root)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::FuncDecl)
            .expect("a function");
        let body = body::body_of_func(&func);
        let return_ty = cst::first_child(&func, |k| k == SyntaxKind::TypeRef)
            .map_or(Ty::Variant, |t| resolve::resolve_type_ref(&db, api, &t));
        let result = infer(&db, api, &root, &class, &body, return_ty, true);
        Harness { result, body }
    }

    /// Every code inference produced — the ungated `diagnostics` plus the severity-free
    /// `raw_warnings` (the gateable Godot codes, post-W1-M0). Infer-level tests assert what the
    /// checker *records*; the gate-level resolution is tested in `crate::warnings`.
    /// The opt-in declaration-strictness codes — filtered out of [`codes`] / [`file_codes`] so they
    /// don't pollute the hundreds of focused fixtures (they fire on essentially every untyped /
    /// inferred local). A test that targets them reads the raw warnings directly (see
    /// `untyped_and_inferred_declarations_warn`).
    const DECLARATION_STRICTNESS: &[&str] = &["UNTYPED_DECLARATION", "INFERRED_DECLARATION"];

    fn codes(h: &Harness) -> Vec<&str> {
        h.result
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .chain(h.result.raw_warnings.iter().map(|w| w.code.as_str()))
            .filter(|c| !DECLARATION_STRICTNESS.contains(c))
            .collect()
    }

    /// Whole-file codes for `files[0]`, analyzed in PROJECT mode (BUG A1 harness): every
    /// `(res_path, text)` pair is loaded into the db, the source root is synced, and the loader's
    /// completeness claim is applied. `project_godot`, when given, feeds the autoload registry.
    fn project_codes(
        files: &[(&str, &str)],
        project_godot: Option<&str>,
        complete: bool,
    ) -> Vec<String> {
        let api = gdscript_api::bundled();
        let mut db = gdscript_db::RootDatabase::default();
        for (i, (path, text)) in files.iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap());
            db.set_file_text(id, text, salsa::Durability::LOW);
            db.set_file_path(id, path);
        }
        if let Some(cfg) = project_godot {
            db.set_project_config(cfg);
        }
        db.sync_source_root();
        db.set_workspace_complete(complete);
        let root = parse(files[0].1).syntax_node();
        let fi = analyze_file(&db, api, &root, FileId(0));
        fi.diagnostics
            .iter()
            .map(|d| d.code.clone())
            .chain(fi.raw_warnings.iter().map(|w| w.code.as_str().to_owned()))
            .filter(|c| !DECLARATION_STRICTNESS.contains(&c.as_str()))
            .collect()
    }

    /// Run the whole-file pass (Pass 1 field fixpoint + Pass 2 functions) and collect every
    /// diagnostic code (ungated diagnostics + raw gateable warnings). Drives `analyze_file`
    /// directly so the bounded member fixpoint runs.
    fn file_codes(src: &str) -> Vec<String> {
        let api = gdscript_api::bundled();
        let db = gdscript_db::RootDatabase::default();
        let root = parse(src).syntax_node();
        let fi = analyze_file(&db, api, &root, FileId(0));
        fi.diagnostics
            .iter()
            .map(|d| d.code.clone())
            .chain(fi.raw_warnings.iter().map(|w| w.code.as_str().to_owned()))
            .filter(|c| !DECLARATION_STRICTNESS.contains(&c.as_str()))
            .collect()
    }

    // ── BUG A3: `## @return-tuple(...)` → Ty::Tuple + constant-index projection ──────────────

    const HOOK_DECL: &str =
        "## @return-tuple(Variant, Callable)\nstatic func useState(v):\n\treturn [v, Callable()]\n";

    #[test]
    fn return_tuple_tag_projects_a_constant_index() {
        // `useState(0)[1]` is the setter: a REAL `Callable`, so a typo'd method on it is caught
        // (UNSAFE_METHOD_ACCESS on the Callable builtin) while the correct `.call` stays silent.
        // Both direct indexing and an `:=`-inferred local carry the tuple.
        let src = format!(
            "{HOOK_DECL}func f():\n\tvar s := useState(0)\n\ts[1].call(1)\n\ts[1].casll(1)\n\tuseState(0)[1].casll(2)\n"
        );
        let c = file_codes(&src);
        assert_eq!(
            c.iter().filter(|x| *x == "UNSAFE_METHOD_ACCESS").count(),
            2,
            "exactly the two typo'd setter methods flag: {c:?}"
        );
    }

    #[test]
    fn return_tuple_untyped_local_stays_variant_by_godot_semantics() {
        // An UNTYPED `var s = useState(0)` local is a `Variant` variable in GDScript (only `:=`
        // infers) — Godot itself cannot check through it, and neither do we. Seeing through an
        // untyped local would need assignment-carried flow narrowing (a Workstream-2 extension,
        // documented follow-up), not a projection change.
        let src =
            format!("{HOOK_DECL}func f():\n\tvar s = useState(0)\n\ts[1].casll(1)\n\treturn s\n");
        let c = file_codes(&src);
        assert!(
            !c.iter().any(|x| x == "UNSAFE_METHOD_ACCESS"),
            "an untyped local is Variant — never checked: {c:?}"
        );
    }

    #[test]
    fn return_tuple_dynamic_or_oob_index_degrades_to_variant() {
        // A dynamic index (or an out-of-bounds constant) cannot select a position — it degrades
        // to the runtime element type (`Variant`), which never fires member checks.
        let src = format!(
            "{HOOK_DECL}func f(i: int):\n\tvar s := useState(0)\n\ts[i].casll(1)\n\ts[7].casll(1)\n"
        );
        let c = file_codes(&src);
        assert!(
            !c.iter().any(|x| x == "UNSAFE_METHOD_ACCESS"),
            "no confident projection without a constant in-bounds index: {c:?}"
        );
    }

    #[test]
    fn return_tuple_widens_to_array_for_assignment_and_methods() {
        // At runtime the tuple IS an untyped Array: it assigns to an `Array` slot without a
        // mismatch, and Array methods (`size`) resolve on it.
        let src = format!(
            "{HOOK_DECL}func f():\n\tvar s := useState(0)\n\tvar a: Array = s\n\treturn [a, s.size()]\n"
        );
        let c = file_codes(&src);
        assert!(
            !c.iter()
                .any(|x| x == TYPE_MISMATCH || x == "UNSAFE_METHOD_ACCESS"),
            "a tuple behaves as its runtime Array: {c:?}"
        );
    }

    #[test]
    fn return_tuple_resolves_cross_file_via_the_member_table() {
        // The guitkx shape once the virtual doc emits field calls: `Hooks.useState(...)` in ONE
        // file, the tagged hook in ANOTHER — the tuple flows through the script member table.
        let files = [
            (
                "res://main.gd",
                "func f():\n\tvar s := Hooks.useState(0)\n\ts[1].casll(1)\n",
            ),
            (
                "res://hooks.gd",
                "class_name Hooks\n## @return-tuple(Variant, Callable)\nstatic func useState(v):\n\treturn [v, Callable()]\n",
            ),
        ];
        let c = project_codes(&files, None, true);
        assert!(
            c.iter().any(|x| x == "UNSAFE_METHOD_ACCESS"),
            "the typo'd setter method must flag cross-file: {c:?}"
        );
    }

    #[test]
    fn return_tuple_tag_is_ignored_when_malformed() {
        // One name is not a tuple; the tag degrades to the plain (annotationless) return.
        let src = "## @return-tuple(Variant)\nstatic func one(v):\n\treturn [v]\nfunc f():\n\tvar s = one(0)\n\ts[0].casll(1)\n";
        let c = file_codes(src);
        assert!(
            !c.iter().any(|x| x == "UNSAFE_METHOD_ACCESS"),
            "a malformed tag must not project: {c:?}"
        );
    }

    // ── BUG A1: UNDEFINED_FUNCTION / UNDEFINED_IDENTIFIER (complete-workspace gated) ─────────

    #[test]
    fn undefined_function_fires_with_a_complete_workspace() {
        let c = project_codes(
            &[("res://main.gd", "func f():\n\tusseState(0)\n")],
            None,
            true,
        );
        assert!(
            c.iter().any(|x| x == "UNDEFINED_FUNCTION"),
            "a provable typo call must fire: {c:?}"
        );
    }

    #[test]
    fn undefined_function_is_silent_without_the_completeness_claim() {
        // The same typo with the workspace NOT declared complete (a lone file / partial load, even
        // with a source root present) keeps the silent seam — absence is not provable.
        let c = project_codes(
            &[("res://main.gd", "func f():\n\tusseState(0)\n")],
            None,
            false,
        );
        assert!(
            !c.iter().any(|x| x.starts_with("UNDEFINED_")),
            "incomplete workspace must stay silent: {c:?}"
        );
    }

    #[test]
    fn undefined_function_skips_cross_file_class_names_and_autoloads() {
        // `Enemy` is a registered global class in ANOTHER file, `Music` a `*`-autoload singleton:
        // calling/reading them is never "undefined" (misusing them is a different mistake).
        let files = [
            (
                "res://main.gd",
                "func f():\n\tvar e = Enemy.new()\n\tMusic.play()\n\treturn e\n",
            ),
            ("res://enemy.gd", "class_name Enemy\nfunc hit(): pass\n"),
            ("res://music.gd", "func play(): pass\n"),
        ];
        let c = project_codes(
            &files,
            Some("[autoload]\nMusic=\"*res://music.gd\"\n"),
            true,
        );
        assert!(
            !c.iter().any(|x| x.starts_with("UNDEFINED_")),
            "cross-file globals + autoloads must resolve: {c:?}"
        );
    }

    #[test]
    fn undefined_function_skips_locals_engine_methods_utilities_and_members() {
        // A local Callable call (A2), an inherited engine method, a utility fn, and a bare call on
        // a member that exists as a VAR (defined-but-not-callable is not "undefined").
        let src = "extends Node\nvar handler: Callable\nfunc f():\n\tvar cb = func(): return 1\n\tcb()\n\tget_child(0)\n\tprint(\"x\")\n\thandler()\n";
        let c = project_codes(&[("res://main.gd", src)], None, true);
        assert!(
            !c.iter().any(|x| x == "UNDEFINED_FUNCTION"),
            "locals / engine methods / utilities / members must never flag: {c:?}"
        );
    }

    #[test]
    fn undefined_function_is_silent_under_a_user_script_base() {
        // `extends Base` (a class_name in another file): the base chain is not engine-native, so
        // absence of an inherited method is not provable — the gate keeps the seam even though
        // `base_method` is defined only on the base.
        let files = [
            (
                "res://main.gd",
                "extends Base\nfunc f():\n\tbase_method()\n",
            ),
            (
                "res://base.gd",
                "class_name Base\nfunc base_method(): pass\n",
            ),
        ];
        let c = project_codes(&files, None, true);
        assert!(
            !c.iter().any(|x| x.starts_with("UNDEFINED_")),
            "a ScriptRef base must gate emission off: {c:?}"
        );
    }

    #[test]
    fn undefined_identifier_fires_and_respects_the_gate() {
        let fire = project_codes(
            &[("res://main.gd", "func f():\n\treturn nonexistent_thing\n")],
            None,
            true,
        );
        assert!(
            fire.iter().any(|x| x == "UNDEFINED_IDENTIFIER"),
            "a provable undeclared read must fire: {fire:?}"
        );
        let silent = project_codes(
            &[("res://main.gd", "func f():\n\treturn nonexistent_thing\n")],
            None,
            false,
        );
        assert!(
            !silent.iter().any(|x| x.starts_with("UNDEFINED_")),
            "incomplete workspace must stay silent: {silent:?}"
        );
    }

    #[test]
    fn undefined_identifier_skips_lambda_captures_and_inner_class_bodies() {
        // A lambda reading an outer local (a capture) is declared; an inner-class body is outside
        // the gate (its bare names may bind outer-class members its scope doesn't chain to).
        let src = "static func outer_helper(): pass\nclass Inner:\n\tfunc g():\n\t\touter_helper()\nfunc f():\n\tvar captured = 1\n\tvar l = func(): return captured\n\treturn l\n";
        let c = project_codes(&[("res://main.gd", src)], None, true);
        assert!(
            !c.iter().any(|x| x.starts_with("UNDEFINED_")),
            "captures + inner-class bodies must never flag: {c:?}"
        );
    }

    #[test]
    fn integer_division_warns() {
        let h = infer_first_func("func f():\n\tvar x = 5 / 2\n");
        assert!(codes(&h).contains(&INTEGER_DIVISION));
    }

    #[test]
    fn float_div_does_not_warn() {
        let h = infer_first_func("func f():\n\tvar x = 5.0 / 2\n");
        assert!(!codes(&h).contains(&INTEGER_DIVISION));
    }

    #[test]
    fn type_mismatch_on_hard_annotation() {
        let h = infer_first_func("func f():\n\tvar s: String = 5\n");
        assert!(codes(&h).contains(&TYPE_MISMATCH));
    }

    #[test]
    fn vector_scalar_compound_assign_is_not_a_mismatch() {
        // `v *= 0.5` desugars to `v = v * 0.5` : Vector2 — not the scalar float (the old collapse).
        let h = infer_first_func(
            "func f() -> Vector2:\n\tvar v := Vector2()\n\tv *= 0.5\n\treturn v\n",
        );
        assert!(!codes(&h).contains(&TYPE_MISMATCH), "{:?}", codes(&h));
    }

    #[test]
    fn array_literal_to_packed_array_is_allowed() {
        let h = infer_first_func("func f():\n\tvar p: PackedStringArray = [\"a\", \"b\"]\n");
        assert!(!codes(&h).contains(&TYPE_MISMATCH), "{:?}", codes(&h));
    }

    #[test]
    fn vector2i_to_vector2_is_allowed() {
        let h = infer_first_func("func f():\n\tvar v: Vector2 = Vector2i(1, 2)\n");
        assert!(!codes(&h).contains(&TYPE_MISMATCH), "{:?}", codes(&h));
    }

    #[test]
    fn local_enum_member_access_types_as_the_enum_not_variant() {
        // `var x := State.IDLE` (a same-file enum) infers the enum type, not a Variant seam — so no
        // false INFERENCE_ON_VARIANT.
        let h = infer_first_func(
            "enum State { IDLE, RUN }\nfunc f():\n\tvar x := State.IDLE\n\treturn x\n",
        );
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn lua_style_dict_key_is_not_an_assignment() {
        // `{ pos = "x" }` is a dict entry (key `pos`), not the statement `pos = "x"` — so it must not
        // check the value against the member `pos`'s type.
        let h = infer_first_func(
            "var pos: Vector2\nfunc f():\n\tvar d = { pos = \"x\" }\n\treturn d\n",
        );
        assert!(!codes(&h).contains(&TYPE_MISMATCH), "{:?}", codes(&h));
    }

    #[test]
    fn narrowing_conversion_float_to_int() {
        let h = infer_first_func("func f():\n\tvar n: int = 1.5\n");
        assert!(codes(&h).contains(&NARROWING_CONVERSION));
    }

    #[test]
    fn int_to_float_is_silent() {
        let h = infer_first_func("func f():\n\tvar x: float = 3\n\treturn x\n");
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
    }

    #[test]
    fn local_shadowing_a_param_warns_shadowed_variable() {
        let h = infer_first_func("func f(x):\n\tvar x = 1\n\treturn x\n");
        assert!(codes(&h).contains(&"SHADOWED_VARIABLE"), "{:?}", codes(&h));
    }

    #[test]
    fn local_shadowing_a_class_member_warns_shadowed_variable() {
        // The class scope (built from the whole file) sees the member `health`; the local shadows it.
        let h =
            infer_first_func("var health = 100\nfunc f():\n\tvar health = 1\n\treturn health\n");
        assert!(codes(&h).contains(&"SHADOWED_VARIABLE"), "{:?}", codes(&h));
    }

    #[test]
    fn non_shadowing_local_does_not_warn_shadowed_variable() {
        let h = infer_first_func("func f(x):\n\tvar y = 1\n\treturn x + y\n");
        assert!(!codes(&h).contains(&"SHADOWED_VARIABLE"), "{:?}", codes(&h));
    }

    #[test]
    fn local_shadowing_a_base_member_warns_base_class() {
        // `position` is a Node2D property; a local of that name shadows the base member.
        let h =
            infer_first_func("extends Node2D\nfunc f():\n\tvar position = 1\n\treturn position\n");
        assert!(
            codes(&h).contains(&"SHADOWED_VARIABLE_BASE_CLASS"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn shadowing_an_unresolved_base_is_silent() {
        // No false positive when the base can't be resolved (the cross-file seam).
        let h = infer_first_func(
            "extends SomeUnknownThirdPartyClass\nfunc f():\n\tvar position = 1\n\treturn position\n",
        );
        assert!(
            !codes(&h).contains(&"SHADOWED_VARIABLE_BASE_CLASS"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn local_named_after_a_native_class_warns_shadowed_global() {
        let h = infer_first_func("func f():\n\tvar Node = 1\n\treturn Node\n");
        assert!(
            codes(&h).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn param_named_after_a_builtin_type_warns_shadowed_global() {
        let h = infer_first_func("func f(Vector2):\n\treturn Vector2\n");
        assert!(
            codes(&h).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn member_named_after_a_native_class_warns_shadowed_global() {
        let cs = file_codes("var Timer = null\n");
        assert!(
            cs.iter().any(|c| c == "SHADOWED_GLOBAL_IDENTIFIER"),
            "{cs:?}"
        );
    }

    #[test]
    fn ordinary_local_does_not_warn_shadowed_global() {
        // A no-false-positive guard: a normal identifier is not a global.
        let h = infer_first_func("func f():\n\tvar count = 1\n\treturn count\n");
        assert!(
            !codes(&h).contains(&SHADOWED_GLOBAL_IDENTIFIER),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn global_shadow_takes_precedence_over_variable_shadow() {
        // A member named after a built-in type (`Color`), shadowed by a local of the same name:
        // Godot emits SHADOWED_GLOBAL_IDENTIFIER (global wins), NOT SHADOWED_VARIABLE on the local.
        let cs = file_codes("var Color = null\nfunc f():\n\tvar Color = 1\n\treturn Color\n");
        assert!(
            cs.iter().any(|c| c == "SHADOWED_GLOBAL_IDENTIFIER"),
            "{cs:?}"
        );
        assert!(
            !cs.iter().any(|c| c == "SHADOWED_VARIABLE"),
            "the local's variable-shadow must be suppressed in favor of the global one: {cs:?}"
        );
    }

    #[test]
    fn assert_true_warns_always_true() {
        let h = infer_first_func("func f():\n\tassert(true)\n");
        assert!(codes(&h).contains(&"ASSERT_ALWAYS_TRUE"), "{:?}", codes(&h));
    }

    #[test]
    fn assert_false_warns_always_false() {
        let h = infer_first_func("func f():\n\tassert(false, \"nope\")\n");
        assert!(
            codes(&h).contains(&"ASSERT_ALWAYS_FALSE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn assert_null_warns_always_false() {
        let h = infer_first_func("func f():\n\tassert(null)\n");
        assert!(
            codes(&h).contains(&"ASSERT_ALWAYS_FALSE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn assert_on_a_variable_is_silent() {
        // No false positive: a runtime condition is not a constant.
        let h = infer_first_func("func f(x):\n\tassert(x)\n");
        assert!(
            !codes(&h).iter().any(|c| c.starts_with("ASSERT_ALWAYS")),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn untyped_and_inferred_declarations_warn() {
        // The opt-in (default IGNORE) declaration-strictness codes, read from the raw warnings —
        // `codes()` filters them out so they don't pollute every other fixture.
        let h = infer_first_func("func f(p):\n\tvar a = 1\n\tvar b := 2\n\tvar c: int = 3\n");
        let raw: Vec<&str> = h
            .result
            .raw_warnings
            .iter()
            .map(|w| w.code.as_str())
            .collect();
        // The untyped param `p` and untyped `var a` — not the typed `var c` nor the inferred `var b`.
        let untyped = raw.iter().filter(|c| **c == "UNTYPED_DECLARATION").count();
        assert_eq!(untyped, 2, "only `p` and `a` are untyped: {raw:?}");
        let inferred = raw.iter().filter(|c| **c == "INFERRED_DECLARATION").count();
        assert_eq!(inferred, 1, "only `b` uses `:=`: {raw:?}");
    }

    #[test]
    fn confusable_identifier_warns_on_a_mixed_script_local() {
        // `p\u{0430}ypal` — Latin letters with a Cyrillic `а` (U+0430): a homoglyph of ASCII `paypal`.
        let h = infer_first_func("func f():\n\tvar p\u{0430}ypal = 1\n\treturn p\u{0430}ypal\n");
        assert!(
            codes(&h).contains(&"CONFUSABLE_IDENTIFIER"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn an_ordinary_ascii_identifier_is_not_confusable() {
        let h = infer_first_func("func f():\n\tvar paypal = 1\n\treturn paypal\n");
        assert!(
            !codes(&h).contains(&"CONFUSABLE_IDENTIFIER"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn a_member_with_a_confusable_name_warns() {
        // Cyrillic `а` inside `balance`.
        let cs = file_codes("var b\u{0430}lance = 0\n");
        assert!(cs.iter().any(|c| c == "CONFUSABLE_IDENTIFIER"), "{cs:?}");
    }

    #[test]
    fn an_assigned_but_never_read_local_is_unused() {
        // Precise read-vs-write: `x` is only assigned, never read → UNUSED_VARIABLE.
        let h = infer_first_func("func f():\n\tvar x = 1\n\tx = 2\n");
        assert!(codes(&h).contains(&"UNUSED_VARIABLE"), "{:?}", codes(&h));
    }

    #[test]
    fn a_read_local_is_not_unused() {
        let h = infer_first_func("func f() -> int:\n\tvar x = 1\n\tx = 2\n\treturn x\n");
        assert!(!codes(&h).contains(&"UNUSED_VARIABLE"), "{:?}", codes(&h));
    }

    #[test]
    fn unused_private_class_variable_warns() {
        let cs = file_codes("var _cache = 0\nfunc f():\n\tpass\n");
        assert!(
            cs.iter().any(|c| c == "UNUSED_PRIVATE_CLASS_VARIABLE"),
            "{cs:?}"
        );
    }

    #[test]
    fn a_read_private_class_variable_is_silent() {
        let cs = file_codes("var _cache = 0\nfunc f() -> int:\n\treturn _cache\n");
        assert!(
            !cs.iter().any(|c| c == "UNUSED_PRIVATE_CLASS_VARIABLE"),
            "{cs:?}"
        );
    }

    #[test]
    fn an_exported_private_var_is_not_unused_private() {
        // No false positive: an `@export`'d `_`-var is set externally (inspector / scene).
        let cs = file_codes("@export var _hidden = 0\n");
        assert!(
            !cs.iter().any(|c| c == "UNUSED_PRIVATE_CLASS_VARIABLE"),
            "{cs:?}"
        );
    }

    #[test]
    fn onready_with_export_warns() {
        let cs = file_codes("@onready @export var n = null\n");
        assert!(cs.iter().any(|c| c == "ONREADY_WITH_EXPORT"), "{cs:?}");
    }

    #[test]
    fn redundant_static_unload_warns_without_a_static_var() {
        let cs = file_codes("@static_unload\nclass_name Foo\nvar x = 1\n");
        assert!(cs.iter().any(|c| c == "REDUNDANT_STATIC_UNLOAD"), "{cs:?}");
    }

    #[test]
    fn static_unload_with_a_static_var_is_silent() {
        let cs = file_codes("@static_unload\nstatic var pool = []\n");
        assert!(!cs.iter().any(|c| c == "REDUNDANT_STATIC_UNLOAD"), "{cs:?}");
    }

    #[test]
    fn typed_local_read_before_assignment_warns() {
        let h = infer_first_func("func f() -> int:\n\tvar x: int\n\treturn x\n");
        assert!(
            codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn typed_local_assigned_then_read_does_not_warn() {
        let h = infer_first_func("func f() -> int:\n\tvar x: int\n\tx = 5\n\treturn x\n");
        assert!(
            !codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn typed_local_with_initializer_is_not_unassigned() {
        let h = infer_first_func("func f() -> int:\n\tvar x: int = 0\n\treturn x\n");
        assert!(
            !codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn untyped_local_is_not_unassigned_checked() {
        // An untyped `var x` is not an UNASSIGNED_VARIABLE candidate (no declared slot type).
        let h = infer_first_func("func f():\n\tvar x\n\tvar y = x\n\treturn y\n");
        assert!(
            !codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn typed_local_assigned_in_all_branches_then_read_does_not_warn() {
        // Both branches assign before the merge ⇒ definitely assigned ⇒ no warning (the join).
        let h = infer_first_func(
            "func f(c) -> int:\n\tvar x: int\n\tif c:\n\t\tx = 1\n\telse:\n\t\tx = 2\n\treturn x\n",
        );
        assert!(
            !codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn typed_local_assigned_in_one_branch_then_read_warns() {
        // Assigned only in the `then` branch ⇒ may be unassigned at the read ⇒ warns (matches Godot).
        let h =
            infer_first_func("func f(c) -> int:\n\tvar x: int\n\tif c:\n\t\tx = 1\n\treturn x\n");
        assert!(
            codes(&h).contains(&"UNASSIGNED_VARIABLE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn arm_after_wildcard_is_unreachable_pattern() {
        let h =
            infer_first_func("func f(x):\n\tmatch x:\n\t\t_:\n\t\t\tpass\n\t\t1:\n\t\t\tpass\n");
        assert!(
            codes(&h).contains(&"UNREACHABLE_PATTERN"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn arm_after_var_bind_is_unreachable_pattern() {
        let h = infer_first_func(
            "func f(x):\n\tmatch x:\n\t\tvar y:\n\t\t\treturn y\n\t\t1:\n\t\t\tpass\n",
        );
        assert!(
            codes(&h).contains(&"UNREACHABLE_PATTERN"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn arm_before_wildcard_is_not_unreachable() {
        let h =
            infer_first_func("func f(x):\n\tmatch x:\n\t\t1:\n\t\t\tpass\n\t\t_:\n\t\t\tpass\n");
        assert!(
            !codes(&h).contains(&"UNREACHABLE_PATTERN"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn guarded_wildcard_is_not_a_catch_all() {
        // `_ when c:` is conditional — a following arm is NOT unreachable.
        let h = infer_first_func(
            "func f(x, c):\n\tmatch x:\n\t\t_ when c:\n\t\t\tpass\n\t\t1:\n\t\t\tpass\n",
        );
        assert!(
            !codes(&h).contains(&"UNREACHABLE_PATTERN"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn multi_pattern_with_wildcard_is_conservatively_not_catch_all() {
        // `1, _:` IS a catch-all in Godot, but we conservatively under-warn (no false positive).
        let h =
            infer_first_func("func f(x):\n\tmatch x:\n\t\t1, _:\n\t\t\tpass\n\t\t2:\n\t\t\tpass\n");
        assert!(
            !codes(&h).contains(&"UNREACHABLE_PATTERN"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn enum_local_without_default_warns() {
        let h = infer_first_func("func f():\n\tvar m: Tween.TweenProcessMode\n");
        assert!(
            codes(&h).contains(&"ENUM_VARIABLE_WITHOUT_DEFAULT"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn enum_member_without_default_warns() {
        let codes = file_codes("var err: Error\nfunc f():\n\tpass\n");
        assert!(
            codes.iter().any(|c| c == "ENUM_VARIABLE_WITHOUT_DEFAULT"),
            "{codes:?}"
        );
    }

    #[test]
    fn native_virtual_override_with_clashing_param_type_warns() {
        // `_input(event: InputEvent)` is a Node virtual; `event: int` is an incompatible override.
        let codes = file_codes("extends Node\nfunc _input(event: int):\n\tpass\n");
        assert!(
            codes.iter().any(|c| c == "NATIVE_METHOD_OVERRIDE"),
            "{codes:?}"
        );
    }

    #[test]
    fn native_virtual_override_with_correct_param_type_does_not_warn() {
        let codes = file_codes("extends Node\nfunc _input(event: InputEvent):\n\tpass\n");
        assert!(
            !codes.iter().any(|c| c == "NATIVE_METHOD_OVERRIDE"),
            "{codes:?}"
        );
    }

    #[test]
    fn native_virtual_override_with_untyped_param_does_not_warn() {
        let codes = file_codes("extends Node\nfunc _input(event):\n\tpass\n");
        assert!(
            !codes.iter().any(|c| c == "NATIVE_METHOD_OVERRIDE"),
            "{codes:?}"
        );
    }

    #[test]
    fn a_non_virtual_method_is_not_a_native_override() {
        let codes = file_codes("extends Node\nfunc my_helper(x: int):\n\treturn x\n");
        assert!(
            !codes.iter().any(|c| c == "NATIVE_METHOD_OVERRIDE"),
            "{codes:?}"
        );
    }

    #[test]
    fn dotted_enum_override_param_does_not_false_warn() {
        // A valid override whose param is a dotted engine enum must NOT clash (enums are int-backed
        // and resolve to different qualified names on the annotation vs model side). Bug-hunt repro.
        let codes = file_codes(
            "extends MultiplayerPeerExtension\nfunc _set_transfer_mode(p_mode: MultiplayerPeer.TransferMode):\n\tpass\n",
        );
        assert!(
            !codes.iter().any(|c| c == "NATIVE_METHOD_OVERRIDE"),
            "{codes:?}"
        );
    }

    #[test]
    fn unused_signal_warns() {
        let codes = file_codes("signal my_event\nfunc f():\n\tpass\n");
        assert!(codes.iter().any(|c| c == "UNUSED_SIGNAL"), "{codes:?}");
    }

    #[test]
    fn emitted_signal_is_not_unused() {
        let codes = file_codes("signal my_event\nfunc f():\n\tmy_event.emit()\n");
        assert!(!codes.iter().any(|c| c == "UNUSED_SIGNAL"), "{codes:?}");
    }

    #[test]
    fn signal_connected_by_string_is_not_unused() {
        let codes = file_codes("signal my_event\nfunc f():\n\tconnect(\"my_event\", Callable())\n");
        assert!(!codes.iter().any(|c| c == "UNUSED_SIGNAL"), "{codes:?}");
    }

    #[test]
    fn enum_local_with_default_does_not_warn() {
        let h = infer_first_func(
            "func f():\n\tvar m: Tween.TweenProcessMode = Tween.TWEEN_PROCESS_IDLE\n\treturn m\n",
        );
        assert!(
            !codes(&h).contains(&"ENUM_VARIABLE_WITHOUT_DEFAULT"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn static_method_on_instance_warns() {
        // `JSON.stringify` is static; calling it through a JSON *instance* warns.
        let h =
            infer_first_func("func f():\n\tvar j := JSON.new()\n\tj.stringify({})\n\treturn j\n");
        assert!(
            codes(&h).contains(&"STATIC_CALLED_ON_INSTANCE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn static_method_on_the_type_does_not_warn() {
        // `JSON.stringify(...)` (on the type) is the correct form — never flagged.
        let h = infer_first_func("func f():\n\tJSON.stringify({})\n");
        assert!(
            !codes(&h).contains(&"STATIC_CALLED_ON_INSTANCE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn static_method_through_a_type_aliased_local_does_not_warn() {
        // `var t := JSON` aliases the TYPE; `t.stringify()` is valid, not static-on-instance.
        let h = infer_first_func("func f():\n\tvar t := JSON\n\tt.stringify({})\n");
        assert!(
            !codes(&h).contains(&"STATIC_CALLED_ON_INSTANCE"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn property_called_as_function_warns() {
        // `n.name` is a Node property; calling it is PROPERTY_USED_AS_FUNCTION.
        let h = infer_first_func("func f(n: Node):\n\tn.name()\n");
        assert!(
            codes(&h).contains(&"PROPERTY_USED_AS_FUNCTION"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn constant_called_as_function_warns() {
        // `NOTIFICATION_READY` is a Node constant; calling it is CONSTANT_USED_AS_FUNCTION.
        let h = infer_first_func("func f(n: Node):\n\tn.NOTIFICATION_READY()\n");
        assert!(
            codes(&h).contains(&"CONSTANT_USED_AS_FUNCTION"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn calling_a_real_method_is_not_a_kind_misuse() {
        let h = infer_first_func("func f(n: Node):\n\tn.get_parent()\n");
        assert!(
            codes(&h).iter().all(|c| !c.ends_with("_USED_AS_FUNCTION")),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn reading_a_property_as_a_value_is_not_a_kind_misuse() {
        let h = infer_first_func("func f(n: Node):\n\tvar s = n.name\n\treturn s\n");
        assert!(
            codes(&h).iter().all(|c| !c.ends_with("_USED_AS_FUNCTION")),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn enum_member_into_its_own_enum_slot_is_not_int_as_enum() {
        // `var m: Tween.TweenProcessMode = Tween.TWEEN_PROCESS_IDLE` is valid GDScript with no
        // cast — the enum member must type as its enum (not bare `int`), so `check_assign` sees
        // `Enum → Enum` (Ok). A regression here would false-warn on extremely common engine code.
        let h = infer_first_func(
            "func f():\n\tvar m: Tween.TweenProcessMode = Tween.TWEEN_PROCESS_IDLE\n\treturn m\n",
        );
        assert!(
            !codes(&h).contains(&"INT_AS_ENUM_WITHOUT_CAST"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn bare_int_into_enum_slot_still_warns() {
        // The fix must not over-suppress: a genuine uncast `int` into an enum slot still warns.
        let h = infer_first_func("func f():\n\tvar m: Tween.TweenProcessMode = 0\n\treturn m\n");
        assert!(
            codes(&h).contains(&"INT_AS_ENUM_WITHOUT_CAST"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn member_access_resolves_engine_property() {
        // In a Node script, bare `get_node(...)` resolves via the inherited base to Object(Node);
        // `get_parent()` is a real Node method → no UNSAFE.
        let h = infer_first_func(
            "extends Node\nfunc f():\n\tvar n := get_node(\"x\")\n\tn.get_parent()\n",
        );
        assert!(
            codes(&h).iter().all(|c| !c.starts_with("UNSAFE")),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn unsafe_method_on_known_type() {
        let h = infer_first_func(
            "extends Node\nfunc f():\n\tvar n := get_node(\"x\")\n\tn.totally_bogus_method()\n",
        );
        assert!(
            codes(&h).contains(&UNSAFE_METHOD_ACCESS),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn is_narrowing_suppresses_unsafe() {
        // Without narrowing, `x.free()` on an untyped param would be unchecked anyway; with
        // `is Node` it is checked against Node and `free` IS a Node method → no UNSAFE.
        let h = infer_first_func("func f(x):\n\tif x is Node:\n\t\tx.queue_free()\n");
        assert!(
            codes(&h).iter().all(|c| !c.starts_with("UNSAFE")),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn is_narrowing_flags_real_missing_member() {
        // After `is Node`, x is Node; `.bogus()` is genuinely missing → UNSAFE.
        let h = infer_first_func("func f(x):\n\tif x is Node:\n\t\tx.bogus_method()\n");
        assert!(codes(&h).contains(&UNSAFE_METHOD_ACCESS));
    }

    #[test]
    fn early_return_is_guard_narrows_past_the_guard() {
        // `if not (x is Node): return` — the only non-returning path proves x is Node, so after the
        // guard a real Node method is safe and a missing one warns (Workstream 2, beats the engine).
        let safe =
            infer_first_func("func f(x):\n\tif not (x is Node):\n\t\treturn\n\tx.get_parent()\n");
        assert!(
            codes(&safe).iter().all(|c| !c.starts_with("UNSAFE")),
            "real Node method must not warn after the guard: {:?}",
            codes(&safe)
        );
        let bogus =
            infer_first_func("func f(x):\n\tif not (x is Node):\n\t\treturn\n\tx.bogus_method()\n");
        assert!(
            codes(&bogus).contains(&UNSAFE_METHOD_ACCESS),
            "missing method must warn after the guard: {:?}",
            codes(&bogus)
        );
    }

    #[test]
    fn and_short_circuit_narrows_the_rhs() {
        // `x is Node and x.<m>()` types the RHS under x: Node — a real method is safe, a missing
        // one warns. The engine does not narrow here (Workstream 2, beats the engine).
        let safe = infer_first_func("func f(x):\n\tif x is Node and x.get_parent():\n\t\tpass\n");
        assert!(
            codes(&safe).iter().all(|c| !c.starts_with("UNSAFE")),
            "real Node method in the and-rhs must not warn: {:?}",
            codes(&safe)
        );
        let bogus =
            infer_first_func("func f(x):\n\tif x is Node and x.bogus_method():\n\t\tpass\n");
        assert!(
            codes(&bogus).contains(&UNSAFE_METHOD_ACCESS),
            "missing method in the and-rhs must warn: {:?}",
            codes(&bogus)
        );
    }

    // ---- Workstream 1 M1: self-contained checks ----

    #[test]
    fn empty_file_warns() {
        assert!(file_codes("").iter().any(|c| c == "EMPTY_FILE"));
        assert!(
            file_codes("# just a comment\n")
                .iter()
                .any(|c| c == "EMPTY_FILE")
        );
        assert!(
            file_codes("extends Node\n")
                .iter()
                .all(|c| c != "EMPTY_FILE")
        );
    }

    #[test]
    fn unused_variable_and_parameter() {
        let h = infer_first_func("func f(unused_p):\n\tvar unused_v = 1\n");
        assert!(codes(&h).contains(&"UNUSED_PARAMETER"), "{:?}", codes(&h));
        assert!(codes(&h).contains(&"UNUSED_VARIABLE"), "{:?}", codes(&h));
        // A used binding does not warn; a `_`-prefixed one is intentionally ignored.
        let used = infer_first_func("func f(p):\n\tvar v = p\n\treturn v\n");
        assert!(codes(&used).iter().all(|c| !c.starts_with("UNUSED")));
        let underscored = infer_first_func("func f(_ignored):\n\tpass\n");
        assert!(!codes(&underscored).contains(&"UNUSED_PARAMETER"));
    }

    #[test]
    fn standalone_expression_and_ternary() {
        let expr = infer_first_func("func f(a, b):\n\ta + b\n");
        assert!(
            codes(&expr).contains(&"STANDALONE_EXPRESSION"),
            "{:?}",
            codes(&expr)
        );
        let tern = infer_first_func("func f(c):\n\t1 if c else 2\n");
        assert!(
            codes(&tern).contains(&"STANDALONE_TERNARY"),
            "{:?}",
            codes(&tern)
        );
        // A call statement has an effect — never flagged.
        let call = infer_first_func("func f(n):\n\tn.queue_free()\n");
        assert!(codes(&call).iter().all(|c| !c.starts_with("STANDALONE")));
    }

    #[test]
    fn unreachable_code_after_return() {
        let h = infer_first_func("func f():\n\treturn\n\tprint(\"dead\")\n");
        assert!(codes(&h).contains(&"UNREACHABLE_CODE"), "{:?}", codes(&h));
    }

    #[test]
    fn incompatible_ternary_warns() {
        // `"s" if c else 1` — String vs int, no common type.
        let h = infer_first_func("func f(c):\n\tvar x = \"s\" if c else 1\n\treturn x\n");
        assert!(
            codes(&h).contains(&"INCOMPATIBLE_TERNARY"),
            "{:?}",
            codes(&h)
        );
    }

    #[test]
    fn variant_receiver_never_unsafe() {
        // Untyped param → Variant receiver → unchecked, no diagnostic.
        let h = infer_first_func("func f(x):\n\tx.anything_at_all()\n");
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
    }

    #[test]
    fn unsafe_call_argument_on_variant_into_typed_param() {
        // Passing an untyped (Variant) value to a typed own-method parameter needs an unsafe cast.
        let h = infer_first_func("func f(p):\n\ttake(p)\nfunc take(n: Node2D):\n\tpass\n");
        assert!(
            codes(&h).contains(&UNSAFE_CALL_ARGUMENT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn unsafe_call_argument_silent_on_safe_and_untyped() {
        // A subtype arg (upcast) is safe; an untyped parameter accepts anything — neither warns.
        let upcast =
            infer_first_func("func f(n: Node2D):\n\ttake(n)\nfunc take(n: Node):\n\tpass\n");
        assert!(
            !codes(&upcast).contains(&UNSAFE_CALL_ARGUMENT),
            "upcast is safe: {:?}",
            upcast.result.diagnostics
        );
        let untyped = infer_first_func("func f(p):\n\ttake(p)\nfunc take(n):\n\tpass\n");
        assert!(
            !codes(&untyped).contains(&UNSAFE_CALL_ARGUMENT),
            "untyped param accepts anything: {:?}",
            untyped.result.diagnostics
        );
    }

    #[test]
    fn inference_on_variant() {
        // `:=` from an untyped (Variant) param.
        let h = infer_first_func("func f(x):\n\tvar y := x\n");
        assert!(codes(&h).contains(&INFERENCE_ON_VARIANT));
    }

    #[test]
    fn field_inferred_from_earlier_field_is_typed() {
        // W2-MEMBER-FIXPOINT: `b`'s initializer references the earlier field `a`. A single shallow
        // field pass would see `a` as `Variant` (seam) and fire INFERENCE_ON_VARIANT on `:= a`; the
        // bounded fixpoint seeds `a: int` so `a + 1` is `int` and `:=` is precise — no warning.
        let codes = file_codes("var a := 1\nvar b := a + 1\n");
        assert!(
            !codes.iter().any(|c| c == INFERENCE_ON_VARIANT),
            "field `b` from earlier field `a` should type as int, not Variant: {codes:?}"
        );
    }

    #[test]
    fn field_forward_reference_is_seamed_not_warned() {
        // A field referencing a *later* field still resolves through the fixpoint (both rounds
        // see each other's seeded type), and at worst lands on the conservative seam — never a
        // false INFERENCE_ON_VARIANT. (`b` precedes `a` lexically here.)
        let codes = file_codes("var b := a\nvar a := 1\n");
        assert!(
            !codes.iter().any(|c| c == INFERENCE_ON_VARIANT),
            "forward field reference must not false-warn: {codes:?}"
        );
    }

    #[test]
    fn standalone_inferred_field_unchanged() {
        // No-regression: a self-contained inferred field still types from its literal, no warning.
        let codes = file_codes("var n := 0\n");
        assert!(
            codes.is_empty(),
            "a literal-initialised field should produce no diagnostics: {codes:?}"
        );
    }

    #[test]
    fn lambda_var_is_callable_not_variant() {
        let h = infer_first_func("func f():\n\tvar cb := func():\n\t\tpass\n");
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn multiline_lambda_then_paren_line_no_false_warning() {
        // A multi-line lambda bound to a var, followed by a statement that begins with `(`.
        // The parser now ends the lambda at its dedent (the `(` line is its own statement), so
        // there is no spurious call-on-lambda and no false `INFERENCE_ON_VARIANT`.
        let src = "func f(state, i, loop):\n\tvar cb := func():\n\t\tif i >= state.size():\n\t\t\treturn\n\t(loop as SceneTree).process_frame.connect(cb, CONNECT_ONE_SHOT)\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn calling_a_callable_value_is_seam_not_variant() {
        // Invoking an arbitrary expression (here a parenthesized `Callable` value) reaches the
        // seam arm of `infer_call`: the return type isn't tracked, so the result is Unknown,
        // not `Variant`, and the inferred-on-Variant warning never fires.
        let src = "func f(cb: Callable):\n\tvar x := (cb)()\n\treturn x\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn calling_a_local_callable_param_is_a_use() {
        // Regression (BUG A2): a bare-name call `cb(0)` on a param never recorded a read — only
        // the value-read path (`resolve_name`) fed `used_locals` — so a param that was ONLY ever
        // called fired a false UNUSED_PARAMETER.
        let src = "func f(cb: Callable):\n\tcb(0)\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&"UNUSED_PARAMETER"),
            "calling a param is a use: {:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn calling_a_local_lambda_var_is_a_use() {
        // Regression (BUG A2), local-var flavor: `var lam = func(x): …` then `lam(5)` used to
        // fire UNUSED_VARIABLE on `lam` (the guitkx hook-stub symptom:
        // `var useState = Hooks.useState` + `useState(0)` flagged useState unused).
        let src = "func g():\n\tvar lam = func(x): return x + 1\n\tlam(5)\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&"UNUSED_VARIABLE"),
            "calling a local is a use: {:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn calling_a_local_callable_result_stays_the_seam() {
        // The call RESULT of a local Callable is the seam (Ty::Callable carries no signature) —
        // never `Variant`, so `var x := cb(0)` must not fire INFERENCE_ON_VARIANT.
        let src = "func f(cb: Callable):\n\tvar x := cb(0)\n\treturn x\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn local_callable_shadows_same_named_method_in_a_call() {
        // GDScript scoping: a local shadows an own/inherited method for a bare-name call, matching
        // resolve_name's canonical local-first order. So the call binds the LOCAL: it is a use (no
        // UNUSED_VARIABLE) and the shadowed method's signature must NOT arg-check the call (the
        // local Callable's params aren't modeled — no UNSAFE_CALL_ARGUMENT / TYPE_MISMATCH).
        let src = "func helper(x: int) -> int:\n\treturn x\nfunc f():\n\tvar helper = func(): return 1\n\thelper(\"not an int\")\n";
        let c = file_codes(src);
        assert!(
            !c.iter().any(|x| x == "UNUSED_VARIABLE"),
            "the shadowing local is used by the call: {c:?}"
        );
        assert!(
            !c.iter()
                .any(|x| x == "UNSAFE_CALL_ARGUMENT" || x == TYPE_MISMATCH),
            "the shadowed method's signature must not check the local's call: {c:?}"
        );
    }

    #[test]
    fn ternary_with_seam_branch_does_not_collapse_to_variant() {
        // A ternary whose else-branch is the seam (`await` is untracked → Unknown) must `join`
        // to Unknown, NOT Variant — otherwise `var x := …` fires a false INFERENCE_ON_VARIANT.
        // (Regression: `join` used to absorb any uninformative branch to Variant.)
        let src =
            "func f(c: bool):\n\tvar x := 5 if c else await get_tree().process_frame\n\treturn x\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "seam branch should keep the ternary on the seam: {:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn await_a_coroutine_call_recovers_its_return_type() {
        // `await f()` yields the call's value, so await is identity on a non-signal operand:
        // `await make()` for `func make() -> int` types `x` as int (was the seam before).
        let src = "func g() -> int:\n\tvar x := await make()\n\treturn x\nfunc make() -> int:\n\treturn 5\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "no false variant warning: {:?}",
            h.result.diagnostics
        );
        let api = gdscript_api::bundled();
        let x = &h.result.bindings[0];
        assert!(
            matches!(&x.ty, Ty::Builtin(b) if api.builtin(*b).name == "int"),
            "await make() should recover int, got {:?}",
            x.ty
        );
    }

    #[test]
    fn await_a_signal_stays_the_seam() {
        // `await sig` yields the signal's payload (needs the Phase-3+ sig table) — must stay the seam,
        // never the Signal type itself, and never a false INFERENCE_ON_VARIANT.
        let src = "func f():\n\tvar x := await get_tree().process_frame\n\treturn x\n";
        let h = infer_first_func(src);
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "awaiting a signal must not warn: {:?}",
            h.result.diagnostics
        );
        assert!(
            matches!(&h.result.bindings[0].ty, Ty::Unknown),
            "awaiting a signal stays the seam, got {:?}",
            h.result.bindings[0].ty
        );
    }

    #[test]
    fn for_var_over_packed_string_array_is_string() {
        // `for s in "a,b".split(",")` iterates a PackedStringArray → String, so `s.to_int()`
        // resolves and `var x := s` does not warn.
        let h = infer_first_func("func f():\n\tfor s in \"a,b\".split(\",\"):\n\t\tvar x := s\n");
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn class_new_is_object_not_variant() {
        let h = infer_first_func("func f():\n\tvar s := GDScript.new()\n");
        assert!(
            !codes(&h).contains(&INFERENCE_ON_VARIANT),
            "{:?}",
            h.result.diagnostics
        );
    }

    #[test]
    fn unknown_seam_never_warns() {
        // `preload(...)` is Unknown; `:=` from it does NOT warn, and member access is unchecked.
        let h = infer_first_func("func f():\n\tvar s := preload(\"res://x.gd\")\n\ts.whatever()\n");
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
    }

    #[test]
    fn expr_types_are_memoized_for_hover() {
        let h = infer_first_func("func f():\n\tvar n := 42\n");
        // The `42` literal expr should be typed int.
        let has_int = h
            .result
            .expr_ty
            .values()
            .any(|t| matches!(t, Ty::Builtin(_)));
        assert!(has_int);
        // sanity: the body lowered at least one expr
        assert!(!h.body.exprs.is_empty());
    }
}
