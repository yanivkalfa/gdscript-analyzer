//! Canonical symbol identity + cursor classification (Playbook §3.M5) — the basis of cross-file
//! navigation (find-references, rename, goto-definition).
//!
//! [`GodotDef`] is the analyzer's analogue of rust-analyzer's `Definition`: a **stable identity**
//! for a renameable/findable symbol, keyed on declaration site (file + name / body location),
//! **never on the name string alone**. [`classify`] is the inverse of inference — it does the same
//! local → member → inherited → global → autoload → engine lookup [`crate::infer`] does, but
//! returns the *declaration identity* instead of the type. Find-references resolves the cursor to
//! a `GodotDef`, then keeps only other tokens that classify to the **same** `GodotDef` (resolve,
//! don't string-match), so two unrelated `i`s, `A.update` vs `B.update`, or a local shadowing a
//! member are distinct by construction.
//!
//! GDScript forbids two same-named members in one class, so a [`GodotDef::Member`] is identified by
//! `(owner_file, name)` alone — no member *kind* in the identity (which keeps decl-site and
//! reference-site classification consistent; the kind is recovered from the item tree for display).

use gdscript_base::{FileId, FilePosition, TextRange};
use gdscript_db::{Db, FileText, parse};
use gdscript_syntax::{GdNode, GdToken, SyntaxKind, ast};
use smol_str::SmolStr;

use crate::cst;
use crate::ty::Ty;

/// The canonical identity of a findable / renameable symbol. Equality is on **identity**, not the
/// name string (rust-analyzer's `Definition`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GodotDef {
    /// A `class_name` global. Identity = the one file that declares it.
    Global {
        /// The declaring file.
        decl_file: FileId,
        /// The class name.
        name: SmolStr,
    },
    /// A script member (func / var / const / signal / enum / inner class). Identity = the script
    /// file that *declares* it (for an inherited member, the base file where it is found) + name.
    Member {
        /// The file declaring the member.
        owner_file: FileId,
        /// The member name.
        name: SmolStr,
    },
    /// A local binding (var / param / `for`-var). Identity = the owning function body + the
    /// binding's declaration-site name range. Two `i`s in different functions, or a local
    /// shadowing a member, are distinct by construction.
    Local {
        /// The file the body lives in.
        body_file: FileId,
        /// The enclosing function/initializer unit's range.
        body_range: TextRange,
        /// The binding's declaration name-token range.
        decl_name_range: TextRange,
    },
    /// An autoload **singleton** (the `*`-flagged `[autoload]` name; project-unique).
    Autoload {
        /// The autoload name.
        name: SmolStr,
        /// The `.gd` it points to, if resolvable (`None` for a `.tscn`/non-`.gd` target).
        target_file: Option<FileId>,
    },
    /// An engine / builtin symbol (`Node`, `Vector2`, a builtin func, …) — resolved, but **not**
    /// ours to rename, and find-references over it is out of scope. Distinguishes "resolved, it's
    /// engine" from "unresolved" (the latter is `None`).
    Engine {
        /// The engine symbol name.
        name: SmolStr,
    },
}

impl GodotDef {
    /// The symbol's name — the cheap text pre-filter key for find-references.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Global { name, .. }
            | Self::Member { name, .. }
            | Self::Autoload { name, .. }
            | Self::Engine { name } => name,
            Self::Local { .. } => "", // filled by the caller from the decl range
        }
    }

    /// Whether this symbol can be renamed at all (engine/builtin symbols cannot).
    #[must_use]
    pub fn is_renameable(&self) -> bool {
        !matches!(self, Self::Engine { .. })
    }
}

/// Classify the symbol the cursor (`pos`) sits on — the single entry point find-references and
/// goto-definition share. `None` for a non-identifier token, or a reference whose target cannot be
/// resolved (the seam — we never guess an identity).
#[must_use]
pub fn classify(db: &dyn Db, pos: FilePosition) -> Option<GodotDef> {
    let ft = db.file_text(pos.file)?;
    let root = parse(db, ft).syntax_node();
    let tok = ast::token_at(&root, pos.offset.into())?;
    let parent = tok.parent();
    // Identifiers are symbols. The soft keywords `match`/`when` are too — but ONLY in a name
    // position (a `Name` decl, or a `NameRef`/`FieldExpr` member). A bare `match` *statement* keyword
    // (parent = `MatchStmt`) must stay a non-symbol, else the cursor on it would falsely resolve to a
    // same-named declaration. (Mirrors the grammar's `at_name` whitelist; see `TECH_DEBT.md`.)
    let is_name_tok = tok.kind() == SyntaxKind::Ident
        || (matches!(tok.kind(), SyntaxKind::MatchKw | SyntaxKind::WhenKw)
            && matches!(parent.kind(), SyntaxKind::Name | SyntaxKind::NameRef));
    if !is_name_tok {
        return None; // keywords / punctuation are not symbols
    }
    let name = SmolStr::new(tok.text());
    let tok_range = cst::token_range(&tok);

    // (A) Declaration sites: the cursor is on the `Name` token of a declaration.
    if parent.kind() == SyntaxKind::Name
        && let Some(def) = classify_decl(db, ft, pos.file, parent, &name, tok_range)
    {
        return Some(def);
    }
    // (A2) The head name of an `extends Base` clause. Its `Ident` is a *bare* child of the
    //      `ExtendsClause` / `ClassNameDecl` / inner-class decl — not wrapped in a `Name` or
    //      `TypeRef` node — so neither (A) nor (B) catches it. Resolve it as a type name so a
    //      `class_name`'s find-references / rename includes every `extends ThatClass` reference
    //      (else a rename silently leaves the `extends` stale — an incomplete, corrupting edit).
    if let Some(head) = cst::extends_head_token(parent)
        && cst::token_range(&head) == tok_range
    {
        return classify_type_name(db, &name);
    }
    // (A3) An anonymous-enum variant declaration (`enum { FIRE }`): a bare `Ident` under an
    //      `EnumVariant` whose enum has no name. Such a variant is a class-level `int` constant, so
    //      it shares the Member identity space — resolve to Member{file, name} so find-refs / goto
    //      reach it. (A *named* enum's variants are accessed as `Enum.NAME`, not as bare class-level
    //      names, so they are out of scope here.)
    if parent.kind() == SyntaxKind::EnumVariant && in_anon_enum(parent) {
        return Some(GodotDef::Member {
            owner_file: pos.file,
            name,
        });
    }
    // (B) A type reference (`var x: Foo`, `is Foo`, `as Foo`): the token is inside a `TypeRef`.
    //     Resolve the type name to a class_name global or an engine class.
    if has_ancestor(&tok, SyntaxKind::TypeRef) {
        return classify_type_name(db, &name);
    }
    // (C) A reference inside a function body / field initializer (a `NameRef`, or the member token
    //     of a `FieldExpr`). Resolve through the inference units.
    classify_body_ref(db, ft, pos.file, pos.offset, &name)
}

/// Classify a declaration-site name (`parent` is the `Name` node; its parent is the decl).
fn classify_decl(
    db: &dyn Db,
    ft: FileText,
    file: FileId,
    name_node: &GdNode,
    name: &SmolStr,
    tok_range: TextRange,
) -> Option<GodotDef> {
    let decl = name_node.parent()?;
    // A `var`/`const` nested inside a function, a property accessor (`get`/`set`), or a lambda body
    // is a LOCAL — not a class member. (A `FuncDecl` ancestor alone misses an accessor-body or a
    // class-level-lambda-body local, which would otherwise be mis-typed as a `Member`.)
    let in_body = node_has_ancestor(decl, SyntaxKind::FuncDecl)
        || node_has_ancestor(decl, SyntaxKind::Getter)
        || node_has_ancestor(decl, SyntaxKind::Setter)
        || node_has_ancestor(decl, SyntaxKind::LambdaExpr);
    // A declaration nested inside a `class Inner:` body is an inner-class member. Its `(file, name)`
    // identity would collide with a same-named TOP-LEVEL member, letting find-refs / rename cross
    // between two unrelated classes (a silent corrupting edit). Inner-class member identity isn't
    // modeled yet (item_tree stores inner members separately), so treat them as out of scope —
    // navigation refuses rather than mis-resolves. (The inner class's own *name* is unaffected: its
    // decl node IS the `InnerClassDecl`, whose ancestor walk starts *above* it.)
    let in_inner_class = node_has_ancestor(decl, SyntaxKind::InnerClassDecl);
    match decl.kind() {
        SyntaxKind::ClassNameDecl => Some(GodotDef::Global {
            decl_file: file,
            name: name.clone(),
        }),
        // A parameter, `for`-loop variable, or `match`-pattern `var` capture is always a local.
        SyntaxKind::Param | SyntaxKind::ForStmt | SyntaxKind::PatternBind => {
            local_def(db, ft, file, tok_range)
        }
        // A `var`/`const` inside a body is a local.
        SyntaxKind::VarDecl | SyntaxKind::ConstDecl if in_body => {
            local_def(db, ft, file, tok_range)
        }
        // Otherwise a class-level member — but only of the top-level class (inner-class members are
        // out of scope, see above).
        SyntaxKind::FuncDecl
        | SyntaxKind::SignalDecl
        | SyntaxKind::EnumDecl
        | SyntaxKind::InnerClassDecl
        | SyntaxKind::VarDecl
        | SyntaxKind::ConstDecl
            if !in_inner_class =>
        {
            Some(GodotDef::Member {
                owner_file: file,
                name: name.clone(),
            })
        }
        _ => None,
    }
}

/// Build a [`GodotDef::Local`] for the binding whose decl-name is at `tok_range`. The identity uses
/// the **binding's** `name_range` (via `binding_at`), so a declaration cursor and a reference (which
/// resolves to the same binding) produce the *same* `Local` — even if the raw token range and the
/// lowered binding range differ.
fn local_def(db: &dyn Db, ft: FileText, file: FileId, tok_range: TextRange) -> Option<GodotDef> {
    let fi = crate::queries::analyze_file(db, ft);
    let unit = fi.unit_at(tok_range.start)?;
    let binding = unit.result.binding_at(tok_range.start)?;
    Some(GodotDef::Local {
        body_file: file,
        body_range: unit.range,
        decl_name_range: trim_range(ft.text(db), binding.name_range),
    })
}

/// A binding's `name_range` can include leading whitespace (a body-lowering quirk); trim it to the
/// bare identifier so a `Local`'s identity and a rename's edit range are both exact.
fn trim_range(text: &str, nr: TextRange) -> TextRange {
    match text.get(nr.start as usize..nr.end as usize) {
        Some(s) => {
            let lead = u32::try_from(s.len() - s.trim_start().len()).unwrap_or(0);
            let len = u32::try_from(s.trim().len()).unwrap_or(0);
            TextRange::new(nr.start + lead, nr.start + lead + len)
        }
        None => nr,
    }
}

/// Resolve a bare type name (in a `TypeRef`) to a `class_name` global or an engine class.
fn classify_type_name(db: &dyn Db, name: &SmolStr) -> Option<GodotDef> {
    let api = db.engine()?;
    match crate::resolve::resolve_type_name(db, api, name) {
        Ty::ScriptRef(sref) => Some(GodotDef::Global {
            decl_file: FileId(sref.0),
            name: name.clone(),
        }),
        Ty::Object(_) | Ty::Builtin(_) => Some(GodotDef::Engine { name: name.clone() }),
        _ => None,
    }
}

/// Classify a reference inside a function/initializer body (a `NameRef`, or a `FieldExpr` member).
fn classify_body_ref(
    db: &dyn Db,
    ft: FileText,
    file: FileId,
    offset: u32,
    name: &SmolStr,
) -> Option<GodotDef> {
    let fi = crate::queries::analyze_file(db, ft);
    let unit = fi.unit_at(offset)?;
    let eid = unit.body.source_map.expr_at_offset(offset)?;
    match unit.body.expr(eid) {
        crate::body::Expr::Name(n) if n == name => {
            resolve_name_to_def(db, ft, file, offset, unit, name)
        }
        crate::body::Expr::Field {
            receiver,
            name: fname,
            name_range,
        } if fname == name && name_range.start <= offset && offset < name_range.end => {
            // `self.member` consults this file's own/inherited members (self's static type is the
            // *base*, so we must resolve it as an own member, like `infer_field` does).
            if matches!(unit.body.expr(*receiver), crate::body::Expr::SelfExpr) {
                return member_owner(db, crate::ty::ScriptRefId(file.0), name, 0).map(|owner| {
                    GodotDef::Member {
                        owner_file: owner,
                        name: name.clone(),
                    }
                });
            }
            let recv_ty = unit.result.type_of(*receiver)?;
            match recv_ty {
                Ty::ScriptRef(sref) => {
                    member_owner(db, *sref, name, 0).map(|owner| GodotDef::Member {
                        owner_file: owner,
                        name: name.clone(),
                    })
                }
                Ty::Object(_) | Ty::Builtin(_) => Some(GodotDef::Engine { name: name.clone() }),
                _ => None, // uninformative receiver — cannot prove identity
            }
        }
        _ => None,
    }
}

/// Replicate [`crate::infer`]'s bare-name lookup order, returning the *declaration identity*:
/// local → own/inherited member → engine global → `class_name` global → autoload. `offset` is the
/// reference site, used to pick the correct binding when a name is shadowed (lexical scoping).
fn resolve_name_to_def(
    db: &dyn Db,
    ft: FileText,
    file: FileId,
    offset: u32,
    unit: &crate::infer::Unit,
    name: &SmolStr,
) -> Option<GodotDef> {
    // 1. A local binding in this unit (var / param / for-var / match-capture). A name can be
    //    shadowed (a param and a same-named local, or a re-declared `var`), so pick the
    //    nearest-PRECEDING declaration — the binding with the greatest start `<=` the reference
    //    offset — mirroring GDScript's lexical shadowing. (First-by-iteration would pick the
    //    outermost, conflating two distinct locals and corrupting a rename.) The binding
    //    `name_range` may carry leading whitespace, so trim before comparing / recording.
    let text = ft.text(db);
    let mut best: Option<TextRange> = None;
    for b in &unit.result.bindings {
        if !matches!(
            b.kind,
            crate::infer::BindingKind::Var
                | crate::infer::BindingKind::Param
                | crate::infer::BindingKind::ForVar
                | crate::infer::BindingKind::MatchBind
        ) {
            continue;
        }
        let nr = trim_range(text, b.name_range);
        if text.get(nr.start as usize..nr.end as usize) != Some(name.as_str()) {
            continue;
        }
        if nr.start <= offset && best.is_none_or(|cur| nr.start >= cur.start) {
            best = Some(nr);
        }
    }
    if let Some(nr) = best {
        return Some(GodotDef::Local {
            body_file: file,
            body_range: unit.range,
            decl_name_range: nr,
        });
    }
    // 2/3. Own or inherited member (walk this script's extends chain).
    if let Some(owner) = member_owner(db, crate::ty::ScriptRefId(file.0), name, 0) {
        return Some(GodotDef::Member {
            owner_file: owner,
            name: name.clone(),
        });
    }
    // 4. An engine global (builtin / native class / singleton / utility / enum) — before
    //    `class_name`, matching `resolve_name`'s precedence.
    if let Some(api) = db.engine()
        && crate::resolve::resolve_global(api, name).is_some()
    {
        return Some(GodotDef::Engine { name: name.clone() });
    }
    // 5. A `class_name` global.
    if let Some(root) = db.source_root()
        && let Some(decl) = crate::queries::global_registry(db, root).resolve(name)
    {
        return Some(GodotDef::Global {
            decl_file: decl.file_id(db),
            name: name.clone(),
        });
    }
    // 6. An autoload singleton.
    if let Some(config) = db.project_config()
        && let Some(path) = crate::queries::autoload_registry(db, config)
            .resolve_path(name)
            .cloned()
    {
        let target = db.source_root().and_then(|root| {
            crate::queries::res_path_registry(db, root)
                .get(path.as_str())
                .copied()
        });
        return Some(GodotDef::Autoload {
            name: name.clone(),
            target_file: target,
        });
    }
    None
}

/// The file that *declares* member `name` for the script in `sref`, walking the `extends` chain
/// (own members first, then user bases). Depth-bounded like the inference member walk.
fn member_owner(
    db: &dyn Db,
    sref: crate::ty::ScriptRefId,
    name: &str,
    depth: u32,
) -> Option<FileId> {
    if depth > 32 {
        return None;
    }
    let file = db.file_text(FileId(sref.0))?;
    let tree = crate::queries::item_tree(db, file);
    // An own member, OR an anonymous-enum variant (a class-level `int` constant that the member
    // table doesn't expose — its enum has no name and its variants aren't `Member`s).
    if tree.member(name).is_some() || anon_enum_has_variant(&tree, name) {
        return Some(file.file_id(db));
    }
    match crate::queries::script_class(db, file).base() {
        Ty::ScriptRef(base) => member_owner(db, *base, name, depth + 1),
        _ => None, // engine base member, or none — not a user-declared member
    }
}

/// Whether `tree` declares `name` as a variant of an **anonymous** `enum { … }` (a flattened
/// class-level `int` constant). Named-enum variants are excluded — they are accessed as `Enum.NAME`,
/// not as bare class-level names.
fn anon_enum_has_variant(tree: &crate::item_tree::ItemTree, name: &str) -> bool {
    tree.members.iter().any(|m| {
        matches!(m, crate::item_tree::Member::Enum(e)
            if e.name.is_none() && e.variants.iter().any(|v| v == name))
    })
}

/// Whether `enum_variant` (an `EnumVariant` node) belongs to an anonymous `enum { … }` (no name).
fn in_anon_enum(enum_variant: &GdNode) -> bool {
    enum_variant.parent().is_some_and(|enum_decl| {
        enum_decl.kind() == SyntaxKind::EnumDecl
            && !enum_decl.children().any(|c| c.kind() == SyntaxKind::Name)
    })
}

/// Whether `tok` has an ancestor node of `kind`.
fn has_ancestor(tok: &GdToken, kind: SyntaxKind) -> bool {
    node_has_ancestor_or_self(tok.parent(), kind)
}

/// Whether `node` itself or any ancestor is of `kind`.
fn node_has_ancestor(node: &GdNode, kind: SyntaxKind) -> bool {
    node.parent()
        .is_some_and(|p| node_has_ancestor_or_self(p, kind))
}

fn node_has_ancestor_or_self(node: &GdNode, kind: SyntaxKind) -> bool {
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if n.kind() == kind {
            return true;
        }
        cur = n.parent().cloned();
    }
    false
}

// ---- M2: node-path navigation (go-to-definition into the `.tscn`) -------------------------

/// A `$Path`/`%Unique`/`get_node("…")` resolved to its scene-node declaration — for go-to-definition
/// **into the owning `.tscn`** (the `[node …]` line). The inverse of M1's node-path typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePathTarget {
    /// The owning scene's file.
    pub scene: FileId,
    /// The resolved node's name.
    pub node_name: SmolStr,
    /// Byte span of the whole `[node …]` header line.
    pub header_span: TextRange,
    /// Byte span of the `name="…"` value (the finer focus).
    pub name_span: TextRange,
}

/// If the cursor sits on a node-path expression (`$Path`/`%Unique`/`get_node("…")`) that resolves
/// against the owning scene, the target node's declaration in the `.tscn`. `None` otherwise.
#[must_use]
pub fn node_path_target(db: &dyn Db, pos: FilePosition) -> Option<NodePathTarget> {
    let ft = db.file_text(pos.file)?;
    let fi = crate::queries::analyze_file(db, ft);
    let unit = fi.unit_at(pos.offset)?;
    let eid = unit.body.source_map.expr_at_offset(pos.offset)?;
    let crate::body::Expr::GetNode {
        path: Some(path),
        unique,
    } = unit.body.expr(eid)
    else {
        return None;
    };
    let ctx = crate::queries::scene_context(db, ft)?;
    let idx = if *unique {
        ctx.model.resolve_unique(path)
    } else {
        ctx.model.resolve_path_from(ctx.attach, path)
    }?;
    let node = ctx.model.node(idx)?;
    Some(NodePathTarget {
        scene: ctx.scene,
        node_name: node.name.clone(),
        header_span: node.header_span,
        name_span: node.name_span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    fn db_with(files: &[(u32, &str)]) -> RootDatabase {
        let mut db = RootDatabase::default();
        for (id, src) in files {
            db.set_file_text(FileId(*id), src, Durability::LOW);
        }
        db.sync_source_root();
        db
    }

    fn at(db: &RootDatabase, file: u32, needle: &str, src: &str) -> Option<GodotDef> {
        let offset = u32::try_from(src.find(needle).expect("needle")).unwrap();
        classify(
            db,
            FilePosition {
                file: FileId(file),
                offset,
            },
        )
    }

    /// classify at the byte offset of the `nth` (0-based) occurrence of `needle`.
    fn at_nth(db: &RootDatabase, file: u32, needle: &str, n: usize, src: &str) -> Option<GodotDef> {
        let off = src.match_indices(needle).nth(n).expect("nth needle").0;
        classify(
            db,
            FilePosition {
                file: FileId(file),
                offset: u32::try_from(off).unwrap(),
            },
        )
    }

    #[test]
    fn two_unrelated_locals_are_distinct() {
        let src =
            "func a():\n\tvar i := 1\n\tvar ra := i\nfunc b():\n\tvar i := 2\n\tvar rb := i\n";
        let db = db_with(&[(0, src)]);
        // The `i` reference in a() (`ra := i`) vs in b() (`rb := i`) — the two `:= i` sites.
        let off_a = u32::try_from(src.match_indices(":= i").next().unwrap().0 + 3).unwrap();
        let off_b = u32::try_from(src.match_indices(":= i").nth(1).unwrap().0 + 3).unwrap();
        let da = classify(
            &db,
            FilePosition {
                file: FileId(0),
                offset: off_a,
            },
        )
        .unwrap();
        let dbf = classify(
            &db,
            FilePosition {
                file: FileId(0),
                offset: off_b,
            },
        )
        .unwrap();
        assert!(matches!(da, GodotDef::Local { .. }), "{da:?}");
        assert!(matches!(dbf, GodotDef::Local { .. }), "{dbf:?}");
        assert_ne!(da, dbf, "two unrelated `i`s must be distinct locals");
    }

    #[test]
    fn local_shadowing_a_member_is_distinct() {
        let src = "var pos := 1\nfunc f():\n\tvar pos := 2\n\tprint(pos)\n";
        let db = db_with(&[(0, src)]);
        // The member decl `var pos` (1st "pos") vs the local `var pos` (2nd "pos").
        let member = at_nth(&db, 0, "pos", 0, src).unwrap();
        let local = at_nth(&db, 0, "pos", 1, src).unwrap();
        assert!(matches!(member, GodotDef::Member { .. }), "{member:?}");
        assert!(matches!(local, GodotDef::Local { .. }), "{local:?}");
        assert_ne!(member, local);
        // The reference `pos` in `print(pos)` (3rd "pos") resolves to the LOCAL (scope wins).
        let r = at_nth(&db, 0, "pos", 2, src).unwrap();
        assert_eq!(r, local);
    }

    #[test]
    fn same_named_members_of_different_classes_are_distinct() {
        let a = "class_name A\nfunc update():\n\tpass\n";
        let b = "class_name B\nfunc update():\n\tpass\n";
        let db = db_with(&[(0, a), (1, b)]);
        let ua = at(&db, 0, "update", a).unwrap();
        let ub = at(&db, 1, "update", b).unwrap();
        assert!(matches!(ua, GodotDef::Member { .. }));
        assert!(matches!(ub, GodotDef::Member { .. }));
        assert_ne!(ua, ub, "A.update and B.update must be distinct");
    }

    #[test]
    fn class_name_decl_and_reference_classify_to_the_same_global() {
        let widget = "class_name Widget\nfunc make() -> int:\n\treturn 1\n";
        let user = "func f():\n\tvar w: Widget\n\tvar x := Widget.new()\n";
        let db = db_with(&[(0, widget), (1, user)]);
        let decl = at(&db, 0, "Widget", widget).unwrap();
        let ann = at(&db, 1, "Widget\n", user).unwrap(); // the annotation `: Widget`
        let ctor = at(&db, 1, "Widget.new", user).unwrap();
        assert!(matches!(
            decl,
            GodotDef::Global {
                decl_file: FileId(0),
                ..
            }
        ));
        assert_eq!(decl, ann, "annotation must resolve to the class_name def");
        assert_eq!(
            decl, ctor,
            "`Widget.new()` must resolve to the class_name def"
        );
    }

    #[test]
    fn extends_user_class_classifies_to_the_global() {
        // The `Base` in `extends Base` is a bare Ident (not a Name/TypeRef node); it must still
        // classify to Base's `class_name` global — else find-refs/rename of Base would miss the
        // `extends` and leave it stale.
        let base = "class_name Base\nfunc m():\n\tpass\n";
        let derived = "class_name Derived\nextends Base\n";
        let db = db_with(&[(0, base), (1, derived)]);
        let decl = at(&db, 0, "Base", base).unwrap();
        let ext = at(&db, 1, "Base", derived).unwrap(); // the `Base` in `extends Base`
        assert!(matches!(
            decl,
            GodotDef::Global {
                decl_file: FileId(0),
                ..
            }
        ));
        assert_eq!(
            decl, ext,
            "`extends Base` must classify to Base's class_name def"
        );
    }

    #[test]
    fn inherited_member_resolves_to_the_declaring_base() {
        let base = "class_name Base\nfunc base_m() -> int:\n\treturn 1\n";
        let derived = "class_name Derived\nextends Base\nfunc use_it():\n\tself.base_m()\n";
        let db = db_with(&[(0, base), (1, derived)]);
        let decl = at(&db, 0, "base_m", base).unwrap();
        let call = at(&db, 1, "base_m()", derived).unwrap();
        assert!(matches!(
            decl,
            GodotDef::Member {
                owner_file: FileId(0),
                ..
            }
        ));
        assert_eq!(
            decl, call,
            "inherited call must resolve to the base's member def"
        );
    }

    #[test]
    fn inner_class_member_is_out_of_scope() {
        // A method inside `class Inner:` must NOT share identity with a same-named top-level method
        // (that would let rename cross between two unrelated classes). It is out of scope → None.
        let src =
            "class_name A\nfunc update():\n\tpass\nclass Inner:\n\tfunc update():\n\t\tpass\n";
        let db = db_with(&[(0, src)]);
        let top = at_nth(&db, 0, "update", 0, src).unwrap();
        let inner = at_nth(&db, 0, "update", 1, src);
        assert!(matches!(top, GodotDef::Member { .. }), "{top:?}");
        assert_eq!(
            inner, None,
            "an inner-class member must not classify (out of scope), got {inner:?}"
        );
    }

    #[test]
    fn match_capture_classifies_as_local_distinct_from_member() {
        // A `match`-captured `var cap` is a local that shadows a same-named member; a reference to
        // it must resolve to the Local, not the member (else rename of the member would corrupt it).
        let src = "var cap := 0\nfunc f(v):\n\tmatch v:\n\t\tvar cap:\n\t\t\tprint(cap)\n";
        let db = db_with(&[(0, src)]);
        let member = at_nth(&db, 0, "cap", 0, src).unwrap();
        let capture = at_nth(&db, 0, "cap", 1, src).unwrap();
        let usage = at_nth(&db, 0, "cap", 2, src).unwrap();
        assert!(matches!(member, GodotDef::Member { .. }), "{member:?}");
        assert!(matches!(capture, GodotDef::Local { .. }), "{capture:?}");
        assert_eq!(
            usage, capture,
            "`print(cap)` must resolve to the match capture"
        );
        assert_ne!(usage, member);
    }

    #[test]
    fn accessor_body_local_is_not_a_member() {
        // A `var` inside a property `get`/`set` accessor is a local, never a class member.
        let src = "var hp: int:\n\tget:\n\t\tvar tmp = 2\n\t\treturn tmp\n";
        let db = db_with(&[(0, src)]);
        let tmp = at_nth(&db, 0, "tmp", 0, src);
        assert!(
            !matches!(tmp, Some(GodotDef::Member { .. })),
            "a local in a get/set body must not be a Member, got {tmp:?}"
        );
    }

    #[test]
    fn anon_enum_variant_classifies_as_member() {
        // An anonymous-enum variant is a class-level constant; its declaration and a bare reference
        // must classify to the same identity (so find-refs / goto reach it).
        let src = "enum { FIRE, ICE }\nfunc f():\n\tprint(FIRE)\n";
        let db = db_with(&[(0, src)]);
        let decl = at_nth(&db, 0, "FIRE", 0, src).unwrap(); // enum { FIRE }
        let usage = at_nth(&db, 0, "FIRE", 1, src).unwrap(); // print(FIRE)
        assert!(matches!(decl, GodotDef::Member { .. }), "{decl:?}");
        assert_eq!(
            decl, usage,
            "an anon-enum variant decl and use share identity"
        );
    }

    #[test]
    fn shadowed_local_reference_resolves_to_the_nearest_declaration() {
        // A param `x` and a local `var x`: the `print(x)` reference must resolve to the LOCAL
        // (nearest-preceding decl), not the param — else find-refs/rename conflates two locals.
        let src = "func f(x):\n\tvar x := 2\n\tprint(x)\n";
        let db = db_with(&[(0, src)]);
        let param = at_nth(&db, 0, "x", 0, src).unwrap();
        let local = at_nth(&db, 0, "x", 1, src).unwrap();
        let usage = at_nth(&db, 0, "x", 2, src).unwrap();
        assert!(matches!(param, GodotDef::Local { .. }), "{param:?}");
        assert!(matches!(local, GodotDef::Local { .. }), "{local:?}");
        assert_ne!(param, local, "param x and local x are distinct");
        assert_eq!(
            usage, local,
            "the reference resolves to the nearest (local) declaration"
        );
    }
}
