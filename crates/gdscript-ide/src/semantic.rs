//! The Phase-2 semantic IDE features (Playbook §1.1/§4): type diagnostics, hover,
//! inlay hints, member completion, signature help, and the "add type annotation" code action.
//!
//! Each is a pure `(text[, offset]) -> POD` function that re-parses, runs
//! [`gdscript_hir::infer::analyze_file`] against the bundled engine model, and maps the result
//! back through the body source map. Re-inferring per query is well within the single-file
//! warm budget; the engine model is `Arc`-shared and excluded from per-file timing.
//!
//! The engine model is native-only (the blob is `include_bytes!`-embedded behind the default
//! `bundled-api` feature). On `wasm32` [`engine`] yields `None` and every semantic feature
//! degrades gracefully (empty / fall back to the Tier-0 by-name path) until the host wires the
//! fetched blob in via `EngineApi::from_bytes` (Phase 5).

use gdscript_api::{BuiltinId, ClassId, EngineApi, MemberRef};
use gdscript_base::{
    CodeAction, CompletionItem, CompletionKind, Diagnostic, HoverResult, InlayHint, InlayHintKind,
    ParamInfo, SignatureHelp, SignatureInfo, SourceChange, TextEdit, TextRange,
};
use gdscript_db::{Db, FileText, parse};
use gdscript_hir::infer::{BindingKind, FileInference};
use gdscript_hir::item_tree::{ItemTree, Member};
use gdscript_hir::queries;
use gdscript_hir::ty::{self, Ty};
use gdscript_syntax::ast;
use gdscript_syntax::{GdNode, SyntaxKind};

use cstree::util::NodeOrToken;

fn to_base_range(r: text_size::TextRange) -> TextRange {
    TextRange::new(u32::from(r.start()), u32::from(r.end()))
}

/// A display label for a type. Resolves a `ScriptRef` to its `class_name` via the project
/// registry (which [`Ty::label`] can't do — it has only the engine model). Other types defer to
/// [`Ty::label`]; `Unknown`/`Error` stay elided.
fn type_label(db: &dyn Db, api: &EngineApi, ty: &Ty) -> Option<String> {
    if let Ty::ScriptRef(sref) = ty {
        return queries::script_ref_name(db, *sref).map(|n| n.to_string());
    }
    ty.label(api)
}

// ---- diagnostics -------------------------------------------------------------------------

/// The §5 type diagnostics for a file (merged into [`crate::Analysis::diagnostics`]).
#[must_use]
pub fn type_diagnostics(db: &dyn Db, file: FileText) -> Vec<Diagnostic> {
    // `analyze_file` already yields an empty result with no engine model (wasm32), so the
    // diagnostics are naturally empty there — no separate guard needed.
    queries::analyze_file(db, file).diagnostics.clone()
}

// ---- hover -------------------------------------------------------------------------------

/// Hover: the inferred type of the expression / binding under the cursor. `Unknown` (the
/// Phase-3 seam) is elided — its label is `None`, so no placeholder is shown.
#[must_use]
pub fn hover(db: &dyn Db, file: FileText, offset: u32) -> Option<HoverResult> {
    let api = db.engine()?;
    let fi = queries::analyze_file(db, file);
    let unit = fi.unit_at(offset)?;

    // An expression under the cursor wins (most specific).
    if let Some(eid) = unit.body.source_map.expr_at_offset(offset)
        && let Some(ty) = unit.result.type_of(eid)
        && let Some(label) = type_label(db, api, ty)
    {
        return Some(HoverResult {
            ty_label: Some(label),
            doc: String::new(),
            range: unit.body.source_map.expr_range(eid),
        });
    }
    // Otherwise a binding name (the declaration site of a local / param / for-var).
    let b = unit.result.binding_at(offset)?;
    Some(HoverResult {
        ty_label: Some(type_label(db, api, &b.ty)?),
        doc: String::new(),
        range: b.name_range,
    })
}

// ---- inlay hints -------------------------------------------------------------------------

/// Inlay `: T` hints on `:=` declarations + unannotated params / `for`-vars — suppressed when
/// the type is uninformative (`Variant`/`Unknown`), the differentiator the engine LSP lacks.
#[must_use]
pub fn inlay_hints(db: &dyn Db, file: FileText) -> Vec<InlayHint> {
    let Some(api) = db.engine() else {
        return Vec::new();
    };
    let fi = queries::analyze_file(db, file);
    let mut hints = Vec::new();
    for unit in &fi.units {
        for b in &unit.result.bindings {
            if b.annotated || b.ty.is_uninformative() {
                continue;
            }
            let show = match b.kind {
                BindingKind::Var => b.inferred_colon_eq,
                BindingKind::Param | BindingKind::ForVar => true,
            };
            if show && let Some(label) = type_label(db, api, &b.ty) {
                hints.push(InlayHint {
                    offset: b.name_range.end,
                    label: format!(": {label}"),
                    kind: InlayHintKind::Type,
                });
            }
        }
    }
    hints
}

// ---- member completion -------------------------------------------------------------------

/// Member completion after `receiver.`: the inheritance-table member set filtered by the
/// inferred receiver type. Returns `None` when the cursor is **not** in a member context, or
/// the receiver is `Variant`/`Unknown` (the caller then falls back to the Tier-0 by-name path
/// so completion never regresses below Phase 1).
#[must_use]
pub fn member_completions(db: &dyn Db, file: FileText, offset: u32) -> Option<Vec<CompletionItem>> {
    let api = db.engine()?;
    let root = parse(db, file).syntax_node();
    let receiver = member_context(&root, offset)?;
    let fi = queries::analyze_file(db, file);

    let mut items = Vec::new();
    let self_recv = is_self_node(&receiver);
    if self_recv {
        items.extend(own_member_items(&fi.tree));
    }

    let recv_range = to_base_range(receiver.text_range());
    let recv_ty = fi.unit_at(offset).and_then(|u| {
        u.body
            .source_map
            .expr_for_range(recv_range)
            .and_then(|e| u.result.type_of(e))
    });

    match recv_ty {
        Some(t) if !t.is_uninformative() => items.extend(members_of_ty(api, t)),
        // `self` with an opaque base still offers this file's own members.
        _ if self_recv => {}
        // A non-`self`, uninformative receiver → defer to the Tier-0 by-name completion.
        _ => return None,
    }
    Some(items)
}

/// The receiver node of the tightest `FieldExpr` whose `.` precedes `offset`, or `None` if the
/// cursor is not in a member-access position.
fn member_context(root: &GdNode, offset: u32) -> Option<GdNode> {
    ast::descendants(root)
        .into_iter()
        .filter(|n| n.kind() == SyntaxKind::FieldExpr)
        .filter(|n| {
            let r = n.text_range();
            u32::from(r.start()) < offset && offset <= u32::from(r.end())
        })
        .min_by_key(|n| u32::from(n.text_range().len()))
        // The first child node of a `FieldExpr` is its receiver expression.
        .and_then(|field| field.children().next().cloned())
}

fn is_self_node(node: &GdNode) -> bool {
    node.kind() == SyntaxKind::NameRef
        && node
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .any(|t| t.kind() == SyntaxKind::SelfKw)
}

/// This file's own members (for `self.` completion).
fn own_member_items(tree: &ItemTree) -> Vec<CompletionItem> {
    tree.members
        .iter()
        .filter_map(|m| {
            let name = m.name()?;
            Some(CompletionItem {
                label: name.to_owned(),
                kind: own_member_kind(m),
                insert_text: None,
                detail: None,
            })
        })
        .collect()
}

fn own_member_kind(m: &Member) -> CompletionKind {
    match m {
        Member::Func(_) => CompletionKind::Function,
        Member::Var(_) => CompletionKind::Variable,
        Member::Const(_) => CompletionKind::Constant,
        Member::Signal(_) => CompletionKind::Signal,
        Member::Enum(_) => CompletionKind::Enum,
        Member::Class(_) => CompletionKind::Class,
    }
}

fn members_of_ty(api: &EngineApi, ty: &Ty) -> Vec<CompletionItem> {
    match ty {
        Ty::Object(c) => api
            .members_of(*c)
            .iter()
            .map(|m| member_ref_item(api, m))
            .collect(),
        Ty::Builtin(b) => builtin_member_items(api, *b),
        Ty::Array(_) => api
            .builtin_by_name("Array")
            .map(|b| builtin_member_items(api, b))
            .unwrap_or_default(),
        Ty::Dict(..) => api
            .builtin_by_name("Dictionary")
            .map(|b| builtin_member_items(api, b))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn member_ref_item(api: &EngineApi, m: &MemberRef) -> CompletionItem {
    let (kind, detail) = match m {
        MemberRef::Method(sig) => (
            CompletionKind::Function,
            ty::resolve_tyref(api, &sig.return_ty).label(api),
        ),
        MemberRef::Property(p) => (
            CompletionKind::Variable,
            ty::resolve_tyref(api, &p.ty).label(api),
        ),
        MemberRef::Const(c) => (
            CompletionKind::Constant,
            ty::resolve_tyref(api, &c.ty).label(api),
        ),
        MemberRef::Signal(_) => (CompletionKind::Signal, None),
        MemberRef::Enum(_) => (CompletionKind::Enum, None),
    };
    CompletionItem {
        label: m.name().to_owned(),
        kind,
        insert_text: None,
        detail,
    }
}

fn builtin_member_items(api: &EngineApi, b: BuiltinId) -> Vec<CompletionItem> {
    let data = api.builtin(b);
    let fields = data.members.iter().map(|f| CompletionItem {
        label: f.name.clone(),
        kind: CompletionKind::Variable,
        insert_text: None,
        detail: ty::resolve_tyref(api, &f.ty).label(api),
    });
    let methods = data.methods.iter().map(|m| CompletionItem {
        label: m.name.clone(),
        kind: CompletionKind::Function,
        insert_text: None,
        detail: ty::resolve_tyref(api, &m.return_ty).label(api),
    });
    let constants = data.constants.iter().map(|c| CompletionItem {
        label: c.name.clone(),
        kind: CompletionKind::Constant,
        insert_text: None,
        detail: None,
    });
    fields.chain(methods).chain(constants).collect()
}

// ---- signature help ----------------------------------------------------------------------

/// Signature help at a call site: the callee's parameter list with the active parameter
/// resolved by counting top-level commas before the cursor.
#[must_use]
pub fn signature_help(db: &dyn Db, file: FileText, offset: u32) -> Option<SignatureHelp> {
    let api = db.engine()?;
    let root = parse(db, file).syntax_node();
    let text = file.text(db);

    // The tightest `ArgList` whose parentheses enclose the cursor.
    let arglist = ast::descendants(&root)
        .into_iter()
        .filter(|n| n.kind() == SyntaxKind::ArgList)
        .filter(|n| {
            let r = n.text_range();
            u32::from(r.start()) < offset && offset <= u32::from(r.end())
        })
        .min_by_key(|n| u32::from(n.text_range().len()))?;
    let call = arglist.parent()?;
    if call.kind() != SyntaxKind::CallExpr {
        return None;
    }
    let callee = call.children().next()?;
    let fi = queries::analyze_file(db, file);
    let tree = queries::item_tree(db, file);
    let sig = resolve_signature(db, api, callee, &fi, &tree)?;

    let open = u32::from(arglist.text_range().start()) + 1; // just past `(`
    let active = count_top_level_commas(text, open, offset);
    Some(SignatureHelp {
        signatures: vec![sig],
        active_signature: 0,
        active_parameter: active,
    })
}

/// Build the signature for a call's callee node (`recv.method` or a bare name).
fn resolve_signature(
    db: &dyn Db,
    api: &EngineApi,
    callee: &GdNode,
    fi: &FileInference,
    tree: &ItemTree,
) -> Option<SignatureInfo> {
    match callee.kind() {
        SyntaxKind::FieldExpr => {
            let receiver = callee.children().next()?;
            let method = field_member_name(callee)?;
            let recv_class = receiver_class(fi, receiver)?;
            if let Some(MemberRef::Method(sig)) = api.lookup_member(recv_class, &method) {
                return Some(method_signature(api, &method, sig));
            }
            None
        }
        SyntaxKind::NameRef => {
            let name = node_first_ident(callee)?;
            if let Some(u) = api.utility(&name) {
                return Some(util_signature(api, &name, u));
            }
            // A bare call is `self.name(...)` — resolve it against the inherited base.
            if let Ty::Object(base) = gdscript_hir::resolve::resolve_base(db, api, tree)
                && let Some(MemberRef::Method(sig)) = api.lookup_member(base, &name)
            {
                return Some(method_signature(api, &name, sig));
            }
            None
        }
        _ => None,
    }
}

/// The class id a receiver expression resolves to, for signature lookup.
fn receiver_class(fi: &FileInference, receiver: &GdNode) -> Option<ClassId> {
    let recv_range = to_base_range(receiver.text_range());
    let offset = recv_range.start;
    let unit = fi.unit_at(offset)?;
    let eid = unit.body.source_map.expr_for_range(recv_range)?;
    match unit.result.type_of(eid)? {
        Ty::Object(c) => Some(*c),
        _ => None,
    }
}

fn method_signature(api: &EngineApi, name: &str, sig: &gdscript_api::MethodSig) -> SignatureInfo {
    let params: Vec<ParamInfo> = sig
        .params
        .iter()
        .map(|p| ParamInfo {
            label: param_label(api, &p.name, &p.ty),
            doc: String::new(),
        })
        .collect();
    let ret = ty::resolve_tyref(api, &sig.return_ty)
        .label(api)
        .unwrap_or_else(|| "void".to_owned());
    let inner = params
        .iter()
        .map(|p| p.label.clone())
        .collect::<Vec<_>>()
        .join(", ");
    SignatureInfo {
        label: format!("{name}({inner}) -> {ret}"),
        doc: String::new(),
        params,
    }
}

fn util_signature(api: &EngineApi, name: &str, u: &gdscript_api::UtilityFn) -> SignatureInfo {
    let params: Vec<ParamInfo> = u
        .params
        .iter()
        .map(|p| ParamInfo {
            label: param_label(api, &p.name, &p.ty),
            doc: String::new(),
        })
        .collect();
    let ret = ty::resolve_tyref(api, &u.return_ty)
        .label(api)
        .unwrap_or_else(|| "void".to_owned());
    let inner = params
        .iter()
        .map(|p| p.label.clone())
        .collect::<Vec<_>>()
        .join(", ");
    SignatureInfo {
        label: format!("{name}({inner}) -> {ret}"),
        doc: String::new(),
        params,
    }
}

fn param_label(api: &EngineApi, name: &str, tyref: &gdscript_api::TyRef) -> String {
    match ty::resolve_tyref(api, tyref).label(api) {
        Some(t) => format!("{name}: {t}"),
        None => name.to_owned(),
    }
}

/// Count comma separators at bracket-depth 0 in `text[start..offset]` — the active param index.
fn count_top_level_commas(text: &str, start: u32, offset: u32) -> u32 {
    let bytes = text.as_bytes();
    let end = (offset as usize).min(bytes.len());
    let begin = (start as usize).min(end);
    let mut depth = 0i32;
    let mut commas = 0u32;
    for &b in &bytes[begin..end] {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    commas
}

// ---- code actions ------------------------------------------------------------------------

/// The "add type annotation" code action: on an unannotated local `var` with a known inferred
/// type, insert `: T` after the name (Playbook §1.1.7).
#[must_use]
pub fn code_actions(db: &dyn Db, file: FileText, offset: u32) -> Vec<CodeAction> {
    let Some(api) = db.engine() else {
        return Vec::new();
    };
    let fi = queries::analyze_file(db, file);
    let Some(unit) = fi.unit_at(offset) else {
        return Vec::new();
    };
    let Some(b) = unit.result.binding_at(offset) else {
        return Vec::new();
    };
    if b.annotated || b.kind != BindingKind::Var {
        return Vec::new();
    }
    // The type to suggest is the binding's own type, or — for an untyped `var x = e` whose
    // binding is gradually `Variant` — the precise initializer type.
    let suggest = if b.ty.is_uninformative() {
        b.init.and_then(|e| unit.result.type_of(e))
    } else {
        Some(&b.ty)
    };
    let Some(label) = suggest.and_then(|t| t.label(api)) else {
        return Vec::new();
    };
    let at = b.name_range.end;
    vec![CodeAction {
        title: format!("Add type annotation `: {label}`"),
        kind: Some("refactor.rewrite".to_owned()),
        edit: SourceChange::single(
            file.file_id(db),
            vec![TextEdit {
                range: TextRange::new(at, at),
                new_text: format!(": {label}"),
            }],
        ),
    }]
}

// ---- small CST helpers -------------------------------------------------------------------

/// The member name of a `FieldExpr` (the last `NameRef`'s identifier).
fn field_member_name(field: &GdNode) -> Option<String> {
    field
        .children()
        .filter(|c| c.kind() == SyntaxKind::NameRef)
        .last()
        .and_then(node_first_ident)
}

/// The first meaningful identifier-ish token text of a node.
fn node_first_ident(node: &GdNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia() && !t.kind().is_synthetic_layout())
        .map(|t| t.text().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_base::FileId;
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    fn db_ft(src: &str) -> (RootDatabase, FileText) {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), src, Durability::LOW);
        db.sync_source_root(); // build the class_name registry, as apply_change does
        let ft = db.file_text(FileId(0)).unwrap();
        (db, ft)
    }

    #[test]
    fn hover_on_user_class_shows_class_name() {
        let src = "class_name Widget\nfunc f():\n\tvar w: Widget\n";
        let offset = u32::try_from(src.find("w: Widget").unwrap()).unwrap();
        let (db, ft) = db_ft(src);
        let h = hover(&db, ft, offset).expect("hover");
        assert_eq!(h.ty_label.as_deref(), Some("Widget"));
    }

    #[test]
    fn hover_reports_inferred_type() {
        let offset = u32::try_from("func f():\n\tvar n := ".len()).unwrap() + 1;
        let (db, ft) = db_ft("func f():\n\tvar n := 42\n");
        let h = hover(&db, ft, offset).expect("hover");
        assert_eq!(h.ty_label.as_deref(), Some("int"));
    }

    #[test]
    fn inlay_hint_on_inferred_var() {
        let (db, ft) = db_ft("func f():\n\tvar n := 42\n");
        let hints = inlay_hints(&db, ft);
        assert!(hints.iter().any(|h| h.label == ": int"));
    }

    #[test]
    fn inlay_suppressed_on_variant() {
        // Untyped param → Variant; `:=` from it is Variant → no inlay (and it would warn).
        let (db, ft) = db_ft("func f(x):\n\tvar y := x\n");
        let hints = inlay_hints(&db, ft);
        assert!(hints.iter().all(|h| h.label != ": Variant"));
    }

    #[test]
    fn member_completion_lists_engine_members() {
        // `extends Node` + `self.` → Node members present, plus this file's own `f`.
        let src = "extends Node\nfunc f():\n\tself.\n";
        let offset = u32::try_from(src.find("self.").unwrap() + "self.".len()).unwrap();
        let (db, ft) = db_ft(src);
        let items = member_completions(&db, ft, offset).expect("member context");
        assert!(items.iter().any(|i| i.label == "add_child"));
        assert!(items.iter().any(|i| i.label == "f"));
    }

    #[test]
    fn member_completion_falls_back_on_variant() {
        // Untyped receiver → None so the caller uses Tier-0.
        let src = "func f(x):\n\tx.\n";
        let offset = u32::try_from(src.find("x.").unwrap() + "x.".len()).unwrap();
        let (db, ft) = db_ft(src);
        assert!(member_completions(&db, ft, offset).is_none());
    }

    #[test]
    fn code_action_adds_annotation() {
        let src = "func f():\n\tvar n = 42\n";
        let offset = u32::try_from(src.find("n =").unwrap()).unwrap();
        let (db, ft) = db_ft(src);
        let actions = code_actions(&db, ft, offset);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].edit.edits[0].edits[0].new_text, ": int");
    }

    #[test]
    fn signature_help_resolves_engine_method() {
        let src = "extends Node\nfunc f():\n\tadd_child()\n";
        let offset = u32::try_from(src.find("add_child(").unwrap() + "add_child(".len()).unwrap();
        let (db, ft) = db_ft(src);
        let help = signature_help(&db, ft, offset).expect("signature");
        assert!(help.signatures[0].label.starts_with("add_child("));
        assert!(!help.signatures[0].params.is_empty());
    }
}
