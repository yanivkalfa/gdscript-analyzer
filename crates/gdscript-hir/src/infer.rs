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
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use std::sync::Arc;

use crate::body::{self, BinOp, Body, Expr, ExprId, Literal, ParamBinding, Stmt, UnOp};
use crate::cst::{self, AstPtr};
use crate::flow::{self, FlowAnalysis, NarrowedTy, Place};
use crate::item_tree::{ItemTree, Member, item_tree};
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
#[must_use]
pub fn infer(
    db: &dyn Db,
    api: &EngineApi,
    root: &GdNode,
    class: &ClassScope,
    body: &Body,
    return_ty: Ty,
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
        narrowing: FxHashMap::default(),
        flow: flow::analyze(body),
    };
    // Parameters bind first (their defaults can reference earlier params).
    let params = body.params.clone();
    for p in &params {
        let ty = cx.param_ty(p);
        cx.bindings.push(Binding {
            name_range: p.name_range,
            ty: ty.clone(),
            init: None,
            annotated: p.type_ref.is_some(),
            inferred_colon_eq: false,
            kind: BindingKind::Param,
        });
        cx.locals.insert(p.name.clone(), ty);
    }
    if let Some(tail) = body.tail {
        cx.infer_expr(tail, &Expectation::None);
    }
    let block = body.block.clone();
    cx.infer_block(&block);
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
    infer(db, api, root, class, &body, return_ty)
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
            let result = infer(db, api, root, &class, &body, return_ty);
            diagnostics.extend(result.diagnostics.iter().cloned());
            raw_warnings.extend(result.raw_warnings.iter().cloned());
            units.push(Unit {
                range: f.range,
                body,
                result,
            });
        }
    }

    FileInference {
        tree,
        units,
        diagnostics,
        raw_warnings,
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
    let result = infer(db, api, root, class, &body, Ty::Variant);
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
    /// The active narrowing env for the current statement, keyed by a dotted access path. Rebuilt
    /// per statement from [`Cx::flow`] (Workstream 2) — not mutated ad-hoc anymore.
    narrowing: FxHashMap<String, Ty>,
    /// The precomputed per-body control-flow narrowing facts (Workstream 2). The checker consults
    /// `facts_before(stmt)` to build [`Cx::narrowing`]; it survives `else`/early-return/`and`-`or`.
    flow: FlowAnalysis,
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
            Assign::Ok | Assign::OkUnsafe | Assign::IntAsEnum => {}
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
        match self.body.stmt(id).clone() {
            Stmt::Expr(e) => {
                self.infer_expr(e, &Expectation::None);
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
                    name_range: f.var_range,
                    ty: var_ty.clone(),
                    init: None,
                    annotated: f.var_type.is_some(),
                    inferred_colon_eq: false,
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
                            name_range: b.range,
                            ty: Ty::Variant,
                            init: None,
                            annotated: false,
                            inferred_colon_eq: false,
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
                }
            }
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
        self.bindings.push(Binding {
            name_range: v.name_range,
            ty: binding_ty.clone(),
            init: v.init,
            annotated: v.type_ref.is_some(),
            inferred_colon_eq: v.is_inferred,
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
                    self.join(&a, &b)
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
                self.index_ty(&base_ty)
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
            Literal::Int => self.int_ty(),
            Literal::Float | Literal::MathConst => self.float_ty(),
            Literal::Bool => self.bool_ty(),
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
        let Some(ctx) = self.owning_scene() else {
            return fallback; // no scene attaches this script (dynamic UI / single-file)
        };
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
            R::Missing if !ctx.ambiguous => {
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
            // ambiguous miss / escape (`..`/absolute) → `Node`, never a false warning
            _ => fallback,
        }
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
                let ret = self.resolve_call_name(&name);
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
    fn resolve_call_name(&self, name: &str) -> Ty {
        if let Some(item) = self.class.lookup(name)
            && let Some(Member::Func(f)) = self.class.member(item)
        {
            return self.func_return_ty(f.return_type.as_deref());
        }
        // A bare call inside the class is `self.name(...)` — resolve against the inherited base.
        if let Ty::Object(base) = self.class.base
            && let Some(MemberRef::Method(sig)) = self.api.lookup_member(base, name)
        {
            return ty::resolve_tyref(self.api, &sig.return_ty);
        }
        if let Some(u) = self.api.utility(name) {
            return ty::resolve_tyref(self.api, &u.return_ty);
        }
        if let Some(f) = self.api.gdscript_builtin(name) {
            return resolve::layer_to_ty(self.api, f.ret);
        }
        // A builtin / class name used as a constructor: `Vector2(...)` / `Array(...)`.
        // Normalize via `resolve_tyref` so `Array`/`Dictionary`/`Callable`/`Signal` land on
        // their dedicated `Ty` variants rather than `Builtin(...)`.
        if let Some(b) = self.api.builtin_by_name(name) {
            return ty::resolve_tyref(self.api, &TyRef::Builtin(b));
        }
        // Otherwise unresolved — most likely a cross-file global / autoload / a method on a
        // `class_name` base we can't see. Treat as the seam so `var x := foo()` never warns.
        Ty::Unknown
    }

    fn func_return_ty(&self, annotation: Option<&str>) -> Ty {
        annotation.map_or(Ty::Variant, |t| {
            resolve::resolve_type_name(self.db, self.api, t)
        })
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
            Ty::Builtin(_) | Ty::Array(_) | Ty::Dict(..) | Ty::Callable | Ty::Signal(_) => {
                self.builtin_member_ty(&recv_ty, name, name_range, as_method)
            }
            // Enum value access (`MyEnum.VALUE`) is an `int`.
            Ty::Enum(_) => self.int_ty(),
            // A cross-file script reference: resolve the member against its (own) member table.
            Ty::ScriptRef(sref) => self.script_member_ty(*sref, name, as_method),
            _ => Ty::Variant,
        }
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
    /// values. Returns `int` when found.
    fn class_enum_value(&self, class: gdscript_api::ClassId, name: &str) -> Option<Ty> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.api.class(cid);
            if c.enums
                .iter()
                .any(|e| e.values.iter().any(|v| v.name == name))
            {
                return Some(self.int_ty());
            }
            cur = c.base;
        }
        None
    }

    /// The builtin id backing a builtin / `Array` / `Dictionary` receiver.
    fn builtin_id_of(&self, ty: &Ty) -> Option<gdscript_api::BuiltinId> {
        match ty {
            Ty::Builtin(b) => Some(*b),
            Ty::Array(_) => self.api.builtin_by_name("Array"),
            Ty::Dict(..) => self.api.builtin_by_name("Dictionary"),
            Ty::Callable => self.api.builtin_by_name("Callable"),
            Ty::Signal(_) => self.api.builtin_by_name("Signal"),
            _ => None,
        }
    }

    /// The element type of an indexing expression (Playbook §2 switch).
    fn index_ty(&self, base: &Ty) -> Ty {
        match base {
            Ty::Array(elem) => (**elem).clone(),
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
                name_range: p.name_range,
                ty: ty.clone(),
                init: None,
                annotated: p.type_ref.is_some(),
                inferred_colon_eq: false,
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

    fn resolve_name(&mut self, id: ExprId, name: &str) -> Ty {
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
        resolve::resolve_external(self.db, &resolve::ExternalRef::Autoload(SmolStr::new(name)))
    }

    fn own_member_ty(&self, item: ClassItem, as_method: bool) -> Ty {
        match item {
            ClassItem::EnumVariant => self.int_ty(),
            ClassItem::Member(_) => match self.class.member(item) {
                Some(Member::Var(v)) => self.field_ty(&v.name, v.ptr),
                Some(Member::Const(c)) => self.field_ty(&c.name, c.ptr),
                Some(Member::Func(f)) => {
                    if as_method {
                        self.func_return_ty(f.return_type.as_deref())
                    } else {
                        Ty::Callable
                    }
                }
                Some(Member::Signal(_)) => Ty::Signal(None),
                Some(Member::Class(_)) => Ty::Unknown,
                Some(Member::Enum(_)) | None => Ty::Variant,
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
        let Some(facts) = self.flow.facts_before(id) else {
            return out;
        };
        for (place, nt) in facts.iter() {
            let NarrowedTy::Is(ptr) = nt else {
                continue;
            };
            let narrowed = self.resolve_ptr_ty(*ptr);
            if narrowed.is_uninformative() {
                continue;
            }
            // Widen-only, gated against the place's declared type. We can look that up directly for
            // a local/param; for `self`-members / field chains the `is_uninformative` check above is
            // the soundness floor (we never assert a member the un-narrowed type couldn't justify).
            if let Place::Local(n) = place
                && let Some(cur) = self.locals.get(n)
                && !cur.is_uninformative()
                && !self.is_subtype(&narrowed, cur)
            {
                continue;
            }
            out.insert(place.dotted_key(), narrowed);
        }
        out
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
        let result = infer(&db, api, &root, &class, &body, return_ty);
        Harness { result, body }
    }

    /// Every code inference produced — the ungated `diagnostics` plus the severity-free
    /// `raw_warnings` (the gateable Godot codes, post-W1-M0). Infer-level tests assert what the
    /// checker *records*; the gate-level resolution is tested in `crate::warnings`.
    fn codes(h: &Harness) -> Vec<&str> {
        h.result
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .chain(h.result.raw_warnings.iter().map(|w| w.code.as_str()))
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
            .collect()
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
    fn narrowing_conversion_float_to_int() {
        let h = infer_first_func("func f():\n\tvar n: int = 1.5\n");
        assert!(codes(&h).contains(&NARROWING_CONVERSION));
    }

    #[test]
    fn int_to_float_is_silent() {
        let h = infer_first_func("func f():\n\tvar x: float = 3\n");
        assert!(codes(&h).is_empty(), "{:?}", codes(&h));
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
