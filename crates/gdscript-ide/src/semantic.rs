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
    CodeAction, CompletionItem, CompletionKind, Diagnostic, FileId, HoverResult, InlayHint,
    InlayHintKind, ParamInfo, SignatureHelp, SignatureInfo, SourceChange, TextEdit, TextRange,
};
use gdscript_db::{Db, FileText, parse};
use gdscript_hir::infer::{BindingKind, FileInference};
use gdscript_hir::item_tree::{ItemTree, Member};
use gdscript_hir::queries;
use gdscript_hir::ty::{self, Ty};
use gdscript_hir::warnings::{self, WarningSettings};
use gdscript_syntax::ast;
use gdscript_syntax::{GdNode, SyntaxKind};
use std::sync::Arc;

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
///
/// Runs the warning **gate** (Workstream 1) *downstream* of the cached `analyze_file` query: the
/// ungated analyzer-native diagnostics pass through, and each severity-free [`RawWarning`] is
/// resolved against the project's [`WarningSettings`] + the per-file suppression map. Because the
/// settings query is keyed on `ProjectConfig` (not a body), a warning-level edit never re-runs
/// inference — only this gate re-runs.
#[must_use]
pub fn type_diagnostics(db: &dyn Db, file: FileText) -> Vec<Diagnostic> {
    // `analyze_file` already yields an empty result with no engine model (wasm32), so the
    // diagnostics are naturally empty there — no separate guard needed.
    let inf = queries::analyze_file(db, file);
    let base = db.project_config().map_or_else(
        || Arc::new(WarningSettings::analyzer_default()),
        |c| queries::warning_settings(db, c),
    );
    // A host-level `--strict`/`--engine-defaults` override flips only the opt-in-group promotion on
    // the resolved settings (preserving the project's explicit per-code levels). Read from a plain,
    // non-salsa `Db` field, so it never enters the tracked query graph — the W1 firewall holds.
    let settings = match db.warning_override() {
        gdscript_db::WarningOverride::None => base,
        gdscript_db::WarningOverride::Strict => Arc::new((*base).clone().with_strict_opt_in(true)),
        gdscript_db::WarningOverride::EngineDefaults => {
            Arc::new((*base).clone().with_strict_opt_in(false))
        }
    };
    let ignores = queries::suppression_map(db, file);
    let path = file.res_path(db);
    let mut out: Vec<Diagnostic> = inf.diagnostics.clone();
    out.extend(
        inf.raw_warnings
            .iter()
            .filter_map(|rw| warnings::gate(rw, &settings, &ignores, path.as_deref())),
    );
    out
}

// ---- hover -------------------------------------------------------------------------------

/// Hover: the inferred type of the expression / binding under the cursor, plus engine
/// documentation (Markdown) when the cursor resolves to a documented engine symbol. `Unknown`
/// (the Phase-3 seam) is elided — its label is `None`, so no placeholder is shown.
#[must_use]
pub fn hover(db: &dyn Db, file: FileText, offset: u32) -> Option<HoverResult> {
    let api = db.engine()?;
    let fi = queries::analyze_file(db, file);

    // A documented engine symbol (member of a typed receiver, or a class / builtin / utility /
    // singleton) wins: it carries the type label *and* the hover doc. Resolved scope-aware (member
    // via the receiver type; bare names via `classify`), so a shadowing local never false-resolves.
    if let Some(h) = engine_symbol_hover(db, api, file, &fi, offset) {
        return Some(h);
    }

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

/// Resolve the identifier under the cursor to a documented engine symbol, returning its type label
/// together with its Markdown doc. `None` when the cursor isn't on an engine symbol (the caller
/// then falls back to the plain type/binding hover).
fn engine_symbol_hover(
    db: &dyn Db,
    api: &EngineApi,
    file: FileText,
    fi: &FileInference,
    offset: u32,
) -> Option<HoverResult> {
    let root = parse(db, file).syntax_node();
    // The identifier under the cursor (try the offset, then one byte back for an end-of-word cursor).
    let tok = ast::token_at(&root, offset.into())
        .filter(|t| t.kind() == SyntaxKind::Ident)
        .or_else(|| {
            ast::token_at(&root, offset.saturating_sub(1).into())
                .filter(|t| t.kind() == SyntaxKind::Ident)
        })?;
    let range = to_base_range(tok.text_range());

    // Member access `recv.member`: resolve the member against the receiver's inferred type.
    if let Some(field) = field_for_member_token(&tok) {
        let receiver = field.children().next()?;
        let recv_ty = receiver_ty(fi, receiver)?;
        return member_doc_hover(db, api, recv_ty, tok.text(), range);
    }

    // A bare name: confirm — scope-aware — that it resolves to an engine symbol, then look it up by
    // name across the engine tables. `classify` handles local/member shadowing for us.
    let pos = gdscript_base::FilePosition {
        file: file.file_id(db),
        offset,
    };
    if let Some(gdscript_hir::def::GodotDef::Engine { name }) = gdscript_hir::def::classify(db, pos)
    {
        return engine_name_hover(api, &name, range);
    }
    None
}

/// If `tok` is the **member** name of a `FieldExpr` (`recv.member`, the `NameRef` after the `.`),
/// the enclosing `FieldExpr`. `None` for the receiver side or a non-member token.
fn field_for_member_token(tok: &gdscript_syntax::GdToken) -> Option<GdNode> {
    let nr = tok.parent(); // a token always has a parent node
    if nr.kind() != SyntaxKind::NameRef {
        return None;
    }
    let field = nr.parent()?;
    if field.kind() != SyntaxKind::FieldExpr {
        return None;
    }
    // The member is the LAST `NameRef` child; the first is the receiver (for `a.b`, both are
    // `NameRef`s). Compare by range — distinct nodes have distinct ranges.
    let member = field
        .children()
        .filter(|c| c.kind() == SyntaxKind::NameRef)
        .last()?;
    (member.text_range() == nr.text_range()).then(|| field.clone())
}

/// The inferred type of a receiver expression (for member-doc lookup).
fn receiver_ty<'a>(fi: &'a FileInference, receiver: &GdNode) -> Option<&'a Ty> {
    let recv_range = to_base_range(receiver.text_range());
    let unit = fi.unit_at(recv_range.start)?;
    let eid = unit.body.source_map.expr_for_range(recv_range)?;
    unit.result.type_of(eid)
}

/// Build a hover for `recv.member` given the receiver's type: the member's signature/type label +
/// its engine doc.
fn member_doc_hover(
    db: &dyn Db,
    api: &EngineApi,
    recv_ty: &Ty,
    name: &str,
    range: TextRange,
) -> Option<HoverResult> {
    match recv_ty {
        Ty::Object(c) => {
            let m = api.lookup_member(*c, name)?;
            Some(HoverResult {
                ty_label: member_ref_label(api, &m),
                doc: member_ref_doc(api, &m).unwrap_or_default(),
                range,
            })
        }
        // `self` / an aliased self / any script value: a member reached through the `extends`
        // chain. If a *user* script in the chain declares the name it's a user member (no engine
        // doc — the user may have overridden an engine method); otherwise resolve against the
        // engine base and show that doc.
        Ty::ScriptRef(sref) => script_ref_member_doc(db, api, *sref, name, range),
        Ty::Builtin(b) => builtin_member_doc_hover(api, *b, name, range),
        Ty::Array(_) => api
            .builtin_by_name("Array")
            .and_then(|b| builtin_member_doc_hover(api, b, name, range)),
        Ty::Dict(..) => api
            .builtin_by_name("Dictionary")
            .and_then(|b| builtin_member_doc_hover(api, b, name, range)),
        _ => None,
    }
}

/// A hover for a builtin-type member (`Vector2.length`, `String.to_int`, …).
fn builtin_member_doc_hover(
    api: &EngineApi,
    b: BuiltinId,
    name: &str,
    range: TextRange,
) -> Option<HoverResult> {
    let data = api.builtin(b);
    if let Some(m) = data.methods.iter().find(|m| m.name == name) {
        return Some(HoverResult {
            ty_label: Some(method_signature(api, name, m).label),
            doc: m
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    if let Some(c) = data.constants.iter().find(|c| c.name == name) {
        return Some(HoverResult {
            ty_label: ty::resolve_tyref(api, &c.ty).label(api),
            doc: c
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    None
}

/// A hover for a bare engine symbol — an engine class, a builtin type, a `@GlobalScope` utility
/// function, or a singleton.
fn engine_name_hover(api: &EngineApi, name: &str, range: TextRange) -> Option<HoverResult> {
    if let Some(cid) = api.class_by_name(name) {
        let c = api.class(cid);
        return Some(HoverResult {
            ty_label: Some(name.to_owned()),
            doc: c
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    if let Some(bid) = api.builtin_by_name(name) {
        let b = api.builtin(bid);
        return Some(HoverResult {
            ty_label: Some(name.to_owned()),
            doc: b
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    if let Some(u) = api.utility(name) {
        return Some(HoverResult {
            ty_label: Some(util_signature(api, name, u).label),
            doc: u
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    if let Some(cid) = api.singleton(name) {
        let c = api.class(cid);
        return Some(HoverResult {
            ty_label: Some(c.name.clone()),
            doc: c
                .doc
                .and_then(|id| api.doc(id))
                .unwrap_or_default()
                .to_owned(),
            range,
        });
    }
    None
}

/// The display label for a resolved engine member (a method signature, or a property/const type).
fn member_ref_label(api: &EngineApi, m: &MemberRef) -> Option<String> {
    match m {
        MemberRef::Method(sig) => Some(method_signature(api, &sig.name, sig).label),
        MemberRef::Property(p) => ty::resolve_tyref(api, &p.ty).label(api),
        MemberRef::Const(c) => ty::resolve_tyref(api, &c.ty).label(api),
        MemberRef::Signal(s) => Some(format!("signal {}", s.name)),
        MemberRef::Enum(e) => Some(format!("enum {}", e.name)),
        MemberRef::EnumValue { class, decl, value } => Some(format!(
            "{}.{}.{} = {}",
            class, decl.name, value.name, value.value
        )),
    }
}

/// Resolve `name` reached through a script's `extends` chain to an engine-base member's doc.
/// Returns `None` when a *user* script in the chain declares the name (a user member has no engine
/// doc, and may shadow/override an engine one), or the name isn't an engine member. Depth-bounded
/// against a cyclic `extends`.
fn script_ref_member_doc(
    db: &dyn Db,
    api: &EngineApi,
    sref: ty::ScriptRefId,
    name: &str,
    range: TextRange,
) -> Option<HoverResult> {
    let mut cur = Some(sref);
    for _ in 0..=32 {
        let s = cur?;
        let ft = db.file_text(FileId(s.0))?;
        if queries::item_tree(db, ft)
            .members
            .iter()
            .any(|m| m.name() == Some(name))
        {
            return None; // user-declared (possibly an override) — no engine doc
        }
        match queries::script_class(db, ft).base() {
            Ty::ScriptRef(base) => cur = Some(*base),
            Ty::Object(class) => {
                let m = api.lookup_member(*class, name)?;
                return Some(HoverResult {
                    ty_label: member_ref_label(api, &m),
                    doc: member_ref_doc(api, &m).unwrap_or_default(),
                    range,
                });
            }
            _ => return None,
        }
    }
    None
}

/// The engine doc Markdown for a resolved member, if any.
fn member_ref_doc(api: &EngineApi, m: &MemberRef) -> Option<String> {
    let id = match m {
        MemberRef::Method(s) => s.doc,
        MemberRef::Property(p) => p.doc,
        MemberRef::Signal(s) => s.doc,
        MemberRef::Const(c) => c.doc,
        MemberRef::Enum(e) => e.doc,
        // Enum values carry no per-value doc handle in the model; hover shows the label only.
        MemberRef::EnumValue { .. } => None,
    }?;
    api.doc(id).map(str::to_owned)
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
                // A `match` capture is always `Variant` (suppressed above anyway) — no inlay.
                BindingKind::MatchBind => false,
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
        // A script reference (`self`, an aliased self, or any `ScriptRef` value): own members +
        // the whole `extends` chain (user bases, then the engine base's members). For `self` the
        // own members were already added above, so don't repeat them.
        Some(Ty::ScriptRef(sref)) => items.extend(script_ref_items(db, api, *sref, !self_recv, 0)),
        Some(t) if !t.is_uninformative() => items.extend(members_of_ty(api, t)),
        // `self` with an opaque base still offers this file's own members.
        _ if self_recv => {}
        // A non-`self`, uninformative receiver → defer to the Tier-0 by-name completion.
        _ => return None,
    }
    Some(items)
}

/// Node-path completion (M2): when the cursor is inside a `$` get-node path (`$`, `$Panel/`,
/// `$Panel/Bt…`), offer the **child node names** of the resolved prefix from the owning scene, each
/// detailed with its `type=`. `$` is unambiguous (always `get_node`), so this never hijacks ordinary
/// completion; in a file with no owning scene it returns `None` and the normal path runs. `%Unique`
/// completion is deferred (the `%`/modulo ambiguity needs token context — see `TECH_DEBT.md`).
pub fn node_path_completions(
    db: &dyn Db,
    file: FileText,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    let text = file.text(db);
    // The backward byte scan has no lexer awareness, so a `$name/` / `%name/` *inside a string
    // literal or comment* (e.g. `var s = "$x/y"`) would otherwise hijack completion. Bail when the
    // cursor sits in a `String`/comment token. (The real `$Panel/` form is `Dollar`/`Ident`/`Slash`
    // tokens, never `String`; the quoted `$"…"` form isn't byte-scannable anyway.)
    let root = parse(db, file).syntax_node();
    if let Some(tok) = ast::token_at(&root, offset.saturating_sub(1).into())
        && (tok.kind() == SyntaxKind::String || tok.kind().is_trivia())
    {
        return None;
    }
    let ctx = queries::scene_context(db, file)?;
    let to_items = |parent| -> Vec<CompletionItem> {
        ctx.model
            .children_of(Some(parent))
            .map(|(_, n)| CompletionItem {
                label: n.name.to_string(),
                kind: CompletionKind::Variable,
                insert_text: None,
                detail: Some(n.decl_type.as_deref().unwrap_or("Node").to_owned()),
            })
            .collect()
    };
    // `$path` → the child nodes of the resolved prefix (the attach node for a bare `$`).
    if let Some(prefix) = dollar_path_prefix(text, offset) {
        let parent = if prefix.is_empty() {
            ctx.attach
        } else {
            ctx.model.resolve_path_from(ctx.attach, &prefix)?
        };
        return Some(to_items(parent));
    }
    // `%Unique` → scene-wide unique nodes. A bare `%` offers every unique node; `%Box/` offers the
    // children of the unique node `Box`. The `%` must be a unique-name SIGIL, not a modulo operator
    // (`a % b`): the parsed `%` token's parent is `UniqueNodeExpr` for the sigil but `BinExpr` for
    // modulo (the grammar's prefix-`%` vs infix-`%`), so we bail on a `BinExpr` parent.
    if let Some((prefix, pct_pos)) = unique_path_prefix(text, offset) {
        let pct = u32::try_from(pct_pos).unwrap_or(0);
        let is_modulo = ast::token_at(&root, pct.into()).is_some_and(|t| {
            t.kind() == SyntaxKind::Percent && t.parent().kind() == SyntaxKind::BinExpr
        });
        if is_modulo {
            return None;
        }
        if prefix.is_empty() {
            let mut items: Vec<CompletionItem> = ctx
                .model
                .unique_nodes
                .iter()
                .filter_map(|(name, idx)| {
                    let n = ctx.model.node(*idx)?;
                    Some(CompletionItem {
                        label: name.to_string(),
                        kind: CompletionKind::Variable,
                        insert_text: None,
                        detail: Some(n.decl_type.as_deref().unwrap_or("Node").to_owned()),
                    })
                })
                .collect();
            items.sort_by(|a, b| a.label.cmp(&b.label)); // FxHashMap order is non-deterministic
            return Some(items);
        }
        let parent = ctx.model.resolve_unique(&prefix)?;
        return Some(to_items(parent));
    }
    None
}

/// If `offset` sits inside a `$`-path, the already-typed **parent** path — everything before the
/// segment under the cursor (`$Panel/Box/Bt|` → `"Panel/Box"`, `$Panel/|` → `"Panel"`, `$|` → `""`).
/// `None` if the cursor is not inside a `$`-path. A pure backward byte scan, robust to the partial
/// (unparseable) text a node path has mid-edit.
fn dollar_path_prefix(text: &str, offset: u32) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = (offset as usize).min(bytes.len());
    // skip the current (partial) segment under the cursor.
    while i > 0 && is_path_ident_byte(bytes[i - 1]) {
        i -= 1;
    }
    let mut segs_rev: Vec<&str> = Vec::new();
    loop {
        if i == 0 {
            return None;
        }
        match bytes[i - 1] {
            b'$' => {
                let mut segs = segs_rev;
                segs.reverse();
                return Some(segs.join("/"));
            }
            b'/' => {
                let seg_end = i - 1;
                let mut s = seg_end;
                while s > 0 && is_path_ident_byte(bytes[s - 1]) {
                    s -= 1;
                }
                segs_rev.push(&text[s..seg_end]);
                i = s;
            }
            _ => return None,
        }
    }
}

/// Like [`dollar_path_prefix`] but for a `%Unique` path: the already-typed **parent** path before
/// the segment under the cursor (`%Box/Bt|` → `("Box", pos)`, `%Box/|` → `("Box", pos)`, `%|` →
/// `("", pos)`), plus the byte offset of the leading `%` (so the caller can confirm via the parsed
/// token that this `%` is a unique-name sigil, not a modulo operator). `None` if not in a `%`-path.
fn unique_path_prefix(text: &str, offset: u32) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut i = (offset as usize).min(bytes.len());
    // skip the current (partial) segment under the cursor.
    while i > 0 && is_path_ident_byte(bytes[i - 1]) {
        i -= 1;
    }
    let mut segs_rev: Vec<&str> = Vec::new();
    loop {
        if i == 0 {
            return None;
        }
        match bytes[i - 1] {
            b'%' => {
                let mut segs = segs_rev;
                segs.reverse();
                return Some((segs.join("/"), i - 1));
            }
            b'/' => {
                let seg_end = i - 1;
                let mut s = seg_end;
                while s > 0 && is_path_ident_byte(bytes[s - 1]) {
                    s -= 1;
                }
                segs_rev.push(&text[s..seg_end]);
                i = s;
            }
            _ => return None,
        }
    }
}

fn is_path_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Completion items for the members reachable through a script [`Ty::ScriptRef`]: optionally its
/// own members, then everything up its `extends` chain (user bases recursively, then the engine
/// base's members). Depth-bounded against a cyclic `extends`.
fn script_ref_items(
    db: &dyn Db,
    api: &EngineApi,
    sref: ty::ScriptRefId,
    include_own: bool,
    depth: u32,
) -> Vec<CompletionItem> {
    if depth > 32 {
        return Vec::new();
    }
    let Some(ft) = db.file_text(FileId(sref.0)) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    if include_own {
        items.extend(own_member_items(&queries::item_tree(db, ft)));
    }
    match queries::script_class(db, ft).base() {
        Ty::ScriptRef(base) => items.extend(script_ref_items(db, api, *base, true, depth + 1)),
        Ty::Object(class) => {
            items.extend(
                api.members_of(*class)
                    .iter()
                    .map(|m| member_ref_item(api, m)),
            );
        }
        _ => {}
    }
    items
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
        MemberRef::EnumValue { class, decl, .. } => (
            CompletionKind::Constant,
            Some(format!("{}.{}", class, decl.name)),
        ),
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
            // A bare call is `self.name(...)` — resolve it against the inherited base. Only an
            // engine `Object` base is consulted here, so the relative-path anchor is irrelevant.
            if let Ty::Object(base) = gdscript_hir::resolve::resolve_base(db, api, tree, None)
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
    fn hover_on_self_engine_method_shows_doc() {
        // `self.add_child` resolves through `extends Node` to the engine member + its hover doc.
        let src = "extends Node\nfunc f():\n\tself.add_child(null)\n";
        let offset = u32::try_from(src.find("add_child").unwrap()).unwrap() + 1;
        let (db, ft) = db_ft(src);
        let h = hover(&db, ft, offset).expect("hover");
        assert!(
            h.ty_label
                .as_deref()
                .is_some_and(|l| l.starts_with("add_child(")),
            "method signature label: {:?}",
            h.ty_label
        );
        assert!(!h.doc.is_empty(), "engine method should carry a hover doc");
        assert!(
            !h.doc.contains("[/"),
            "no unconverted BBCode leaks: {:?}",
            h.doc
        );
    }

    #[test]
    fn hover_on_engine_class_shows_doc() {
        // The class name in a type annotation shows the class's hover doc.
        let src = "func f():\n\tvar n: Node\n";
        let offset = u32::try_from(src.find("Node").unwrap()).unwrap() + 1;
        let (db, ft) = db_ft(src);
        let h = hover(&db, ft, offset).expect("hover");
        assert_eq!(h.ty_label.as_deref(), Some("Node"));
        assert!(!h.doc.is_empty(), "engine class should carry a hover doc");
    }

    #[test]
    fn hover_on_overridden_method_prefers_no_engine_doc() {
        // A user method overriding an engine one is a user member — no engine doc is shown.
        let src = "extends Node\nfunc add_child(x):\n\tpass\nfunc g():\n\tself.add_child(null)\n";
        let off = src.find("self.add_child").unwrap() + "self.".len() + 1;
        let (db, ft) = db_ft(src);
        let h = hover(&db, ft, u32::try_from(off).unwrap());
        // Either no engine-symbol hover (falls back) or an empty doc — never Node.add_child's doc.
        assert!(
            h.as_ref().is_none_or(|h| h.doc.is_empty()),
            "an overriding user method must not show the engine doc: {:?}",
            h.map(|h| h.doc)
        );
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

    #[test]
    fn gating_drops_warnings_when_disabled_in_project_config() {
        let src = "func f():\n\tvar x = 5 / 2\n";
        // Standalone (no `project.godot`): the gateable INTEGER_DIVISION warning surfaces.
        let (db, ft) = db_ft(src);
        assert!(
            type_diagnostics(&db, ft)
                .iter()
                .any(|d| d.code == "INTEGER_DIVISION"),
            "standalone default should surface the warning",
        );
        // `debug/gdscript/warnings/enable=false` → the gate drops every gateable warning.
        let mut db2 = RootDatabase::default();
        db2.set_file_text(FileId(0), src, Durability::LOW);
        db2.set_project_config("[debug]\ngdscript/warnings/enable=false\n");
        db2.sync_source_root();
        let ft2 = db2.file_text(FileId(0)).unwrap();
        assert!(
            type_diagnostics(&db2, ft2)
                .iter()
                .all(|d| d.code != "INTEGER_DIVISION"),
            "enable=false must suppress gateable warnings",
        );
    }

    #[test]
    fn warning_override_engine_defaults_silences_the_opt_in_group() {
        let src = "extends Node\nfunc f(n: Node):\n\tn.not_a_real_method()\n";
        // Standalone (no project.godot) defaults to strict → the opt-in UNSAFE_* group fires.
        let (db, ft) = db_ft(src);
        assert!(
            type_diagnostics(&db, ft)
                .iter()
                .any(|d| d.code == "UNSAFE_METHOD_ACCESS")
        );
        // `--engine-defaults` forces the opt-in group back to IGNORE even in standalone mode.
        let (mut db2, ft2) = db_ft(src);
        db2.set_warning_override(crate::WarningOverride::EngineDefaults);
        assert!(
            type_diagnostics(&db2, ft2)
                .iter()
                .all(|d| d.code != "UNSAFE_METHOD_ACCESS"),
            "--engine-defaults must silence the opt-in group",
        );
    }

    #[test]
    fn warning_override_strict_respects_an_explicit_project_ignore() {
        let src = "extends Node\nfunc f(n: Node):\n\tn.not_a_real_method()\n";
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), src, Durability::LOW);
        // The project explicitly ignores unsafe_method_access.
        db.set_project_config("[debug]\ngdscript/warnings/unsafe_method_access=0\n");
        db.sync_source_root();
        db.set_warning_override(crate::WarningOverride::Strict);
        let ft = db.file_text(FileId(0)).unwrap();
        // `--strict` promotes the opt-in group, but an explicit per-code Ignore still wins.
        assert!(
            type_diagnostics(&db, ft)
                .iter()
                .all(|d| d.code != "UNSAFE_METHOD_ACCESS"),
            "an explicit per-code ignore must beat --strict",
        );
    }

    #[test]
    fn warning_ignore_annotation_suppresses_through_diagnostics() {
        let ignored =
            "func f():\n\t@warning_ignore(\"integer_division\")\n\tvar x = 5 / 2\n\treturn x\n";
        let (db, ft) = db_ft(ignored);
        assert!(
            type_diagnostics(&db, ft)
                .iter()
                .all(|d| d.code != "INTEGER_DIVISION"),
            "@warning_ignore must suppress the decorated statement's warning",
        );
        // Without the annotation, the same code fires.
        let (db2, ft2) = db_ft("func f():\n\tvar x = 5 / 2\n\treturn x\n");
        assert!(
            type_diagnostics(&db2, ft2)
                .iter()
                .any(|d| d.code == "INTEGER_DIVISION"),
        );
    }
}
