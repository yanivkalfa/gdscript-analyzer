//! The item tree (Playbook §3.1): a signature-level view of one `.gd` file — its
//! `class_name`, `extends` target, and class members (funcs/vars/consts/signals/enums/inner
//! classes) — lowered from the CST **without reading any function body**.
//!
//! This "no bodies" rule is the Phase-3 cache invariant: editing a function body must not
//! change the item tree, so signature-derived data (and everything keyed on it) can be
//! reused across body edits once salsa lands. To keep that promise the tree holds only plain
//! owned data plus reparse-stable [`AstPtr`]s — never live CST nodes — so it is `Eq` and a
//! body edit that doesn't move a declaration produces an identical tree.

use std::sync::Arc;

use gdscript_base::TextRange;
use gdscript_syntax::ast::{self, AstNode};
use gdscript_syntax::{GdNode, SyntaxKind};
use smol_str::SmolStr;

use crate::cst::{self, AstPtr};

/// The signature-level model of one file (or one inner class).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemTree {
    /// The registered global class name (`class_name X`), if any. Always `None` for an
    /// inner class.
    pub class_name: Option<SmolStr>,
    /// The `extends` target, if written.
    pub extends: Option<ExtendsRef>,
    /// The class members, in source order.
    pub members: Vec<Member>,
}

impl ItemTree {
    /// The first member named `name` (linear scan — member lists are small).
    #[must_use]
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name() == Some(name))
    }
}

/// An `extends` target. Phase 2 only resolves a bare engine-class [`ExtendsRef::Name`]; the
/// dotted and script-path forms funnel through the Phase-3 seam to `Ty::Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendsRef {
    /// `extends Node` — a bare identifier, resolved against the engine table (else `Unknown`).
    Name(SmolStr),
    /// `extends A.B` — a dotted path (namespaced / inner class); `Unknown` in Phase 2.
    Path(SmolStr),
    /// `extends "res://x.gd"` — a script path literal; `Unknown` in Phase 2.
    ScriptPath(SmolStr),
    /// `extends "res://x.gd".Inner` — a script path **selecting an inner class**. We can't model the
    /// inner class yet (see `TECH_DEBT`), so this is the seam (`Unknown`) — never the outer script, which
    /// would wrongly accept the outer class's members. The path is carried for a future inner-class
    /// resolver. (`SmolStr` is the path part, sans the trailing `.Inner` selectors.)
    ScriptPathInner(SmolStr),
}

/// One class member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Member {
    /// `func f(...)`.
    Func(FuncItem),
    /// `var x`.
    Var(VarItem),
    /// `const X`.
    Const(ConstItem),
    /// `signal s`.
    Signal(SignalItem),
    /// `enum E { ... }` (or an anonymous `enum { ... }`).
    Enum(EnumItem),
    /// `class Inner: ...`.
    Class(InnerClassItem),
}

impl Member {
    /// The member's declared name, or `None` for an anonymous enum.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Func(f) => Some(&f.name),
            Self::Var(v) => Some(&v.name),
            Self::Const(c) => Some(&c.name),
            Self::Signal(s) => Some(&s.name),
            Self::Enum(e) => e.name.as_deref(),
            Self::Class(c) => Some(&c.name),
        }
    }
}

/// A parameter of a function or signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamItem {
    /// The parameter name.
    pub name: SmolStr,
    /// The written type annotation (unresolved text, e.g. `"int"`, `"Array[int]"`), if any.
    pub type_ref: Option<SmolStr>,
    /// Whether the parameter has a default value (`p := expr` / `p: T = expr`).
    pub has_default: bool,
}

/// A `func` member (signature only — the body is lowered lazily by [`crate::body`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncItem {
    /// The function name.
    pub name: SmolStr,
    /// The parameters, in order.
    pub params: Vec<ParamItem>,
    /// The written return-type annotation (unresolved text), if any.
    pub return_type: Option<SmolStr>,
    /// Whether this is a `static func`.
    pub is_static: bool,
    /// Pointer to the `FuncDecl` node, for body lowering.
    pub ptr: AstPtr,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range (the navigation focus).
    pub name_range: TextRange,
}

/// A `var` member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarItem {
    /// The variable name.
    pub name: SmolStr,
    /// The written type annotation (unresolved text), if any.
    pub type_ref: Option<SmolStr>,
    /// Whether this is a `static var`.
    pub is_static: bool,
    /// Whether it has an initializer expression.
    pub has_init: bool,
    /// Whether the type was inferred with `:=`.
    pub is_inferred: bool,
    /// Pointer to the `VarDecl` node, for initializer inference.
    pub ptr: AstPtr,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range.
    pub name_range: TextRange,
}

/// A `const` member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstItem {
    /// The constant name.
    pub name: SmolStr,
    /// The written type annotation (unresolved text), if any.
    pub type_ref: Option<SmolStr>,
    /// The `res://` (or relative) path of a `const X = preload("…")` initializer — read at the
    /// **signature** level (the initializer is directly a `preload` of a string literal). Lets a
    /// cross-file reference (`other.X`) resolve the const to the preloaded script's `ScriptRef`, which
    /// the offset-free `script_class` projection otherwise can't (it drops initializers). Firewall-safe:
    /// a `const` declaration is not a function body, so a body edit leaves it unchanged.
    pub preload_path: Option<SmolStr>,
    /// Pointer to the `ConstDecl` node, for value inference.
    pub ptr: AstPtr,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range.
    pub name_range: TextRange,
}

/// A `signal` member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalItem {
    /// The signal name.
    pub name: SmolStr,
    /// The typed parameters, in order.
    pub params: Vec<ParamItem>,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range.
    pub name_range: TextRange,
}

/// An `enum` member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumItem {
    /// The enum name, or `None` for an anonymous `enum { ... }` (whose variants become
    /// class-level `int` constants).
    pub name: Option<SmolStr>,
    /// The variant names, in order.
    pub variants: Vec<SmolStr>,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range (the whole `enum` keyword range for an anonymous enum).
    pub name_range: TextRange,
}

/// An inner `class` member: its name plus its own (recursively lowered) item tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InnerClassItem {
    /// The inner class name.
    pub name: SmolStr,
    /// The inner class's members + `extends`.
    pub tree: ItemTree,
    /// The whole declaration's range.
    pub range: TextRange,
    /// The name token's range.
    pub name_range: TextRange,
}

/// Lower a parsed file to its [`ItemTree`] (Playbook §3.1). Pure; reads no bodies.
#[must_use]
pub fn item_tree(root: &GdNode) -> Arc<ItemTree> {
    let Some(file) = ast::SourceFile::cast(root.clone()) else {
        return Arc::new(ItemTree::default());
    };
    Arc::new(lower_class(root, file.decls()))
}

/// Lower a sequence of declarations (a file body or an inner-class body) plus the `extends`
/// clause found among `container`'s structure into an [`ItemTree`].
fn lower_class(container: &GdNode, decls: impl Iterator<Item = ast::Decl>) -> ItemTree {
    let mut tree = ItemTree {
        extends: find_extends(container),
        ..ItemTree::default()
    };
    for decl in decls {
        match decl {
            ast::Decl::ClassName(d) => {
                if let Some(name) = decl_name(d.name()) {
                    tree.class_name = Some(name);
                }
            }
            ast::Decl::Func(d) => tree.members.push(Member::Func(lower_func(&d))),
            ast::Decl::Var(d) => tree.members.push(Member::Var(lower_var(&d))),
            ast::Decl::Const(d) => tree.members.push(Member::Const(lower_const(&d))),
            ast::Decl::Signal(d) => tree.members.push(Member::Signal(lower_signal(&d))),
            ast::Decl::Enum(d) => tree.members.push(Member::Enum(lower_enum(&d))),
            ast::Decl::Class(d) => {
                if let Some(item) = lower_inner_class(&d) {
                    tree.members.push(Member::Class(item));
                }
            }
        }
    }
    tree
}

fn lower_func(d: &ast::FuncDecl) -> FuncItem {
    let node = d.syntax();
    FuncItem {
        name: decl_name(d.name()).unwrap_or_default(),
        params: d
            .param_list()
            .map(|pl| lower_params(&pl))
            .unwrap_or_default(),
        return_type: d.return_type().and_then(|t| t.text()).map(SmolStr::new),
        is_static: d.is_static(),
        ptr: AstPtr::of(node),
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    }
}

fn lower_var(d: &ast::VarDecl) -> VarItem {
    let node = d.syntax();
    VarItem {
        name: decl_name(d.name()).unwrap_or_default(),
        type_ref: d.type_ref().and_then(|t| t.text()).map(SmolStr::new),
        is_static: d.is_static(),
        has_init: cst::first_child_expr(node).is_some(),
        is_inferred: cst::has_token(node, SyntaxKind::ColonEq),
        ptr: AstPtr::of(node),
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    }
}

fn lower_const(d: &ast::ConstDecl) -> ConstItem {
    let node = d.syntax();
    // The annotation, if any, is the `TypeRef` child (the AST exposes no accessor on
    // `ConstDecl`, so read it directly).
    let type_ref = cst::first_child(node, |k| k == SyntaxKind::TypeRef)
        .and_then(ast::TypeRef::cast)
        .and_then(|t| t.text())
        .map(SmolStr::new);
    ConstItem {
        name: decl_name(d.name()).unwrap_or_default(),
        type_ref,
        preload_path: const_preload_path(node),
        ptr: AstPtr::of(node),
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    }
}

/// The `res://` (or relative) path a `const X = preload("…")` aliases, read at the signature level.
/// The initializer must be **directly** a `preload` of a string literal (so the const aliases exactly
/// one preloaded script — not a `preload` nested in an array/expression). Mirrors the body lowering's
/// `PreloadExpr` extraction.
fn const_preload_path(const_decl: &GdNode) -> Option<SmolStr> {
    let preload = cst::first_child(const_decl, |k| k == SyntaxKind::PreloadExpr)?;
    let arg = cst::first_child(&preload, |k| k == SyntaxKind::ArgList)
        .and_then(|al| cst::first_child_expr(&al))?;
    if arg.kind() != SyntaxKind::Literal {
        return None;
    }
    cst::child_token_text(&arg, SyntaxKind::String)
        .map(|s| SmolStr::new(s.trim_matches(['"', '\''])))
}

fn lower_signal(d: &ast::SignalDecl) -> SignalItem {
    let node = d.syntax();
    SignalItem {
        name: decl_name(d.name()).unwrap_or_default(),
        params: d
            .param_list()
            .map(|pl| lower_params(&pl))
            .unwrap_or_default(),
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    }
}

fn lower_enum(d: &ast::EnumDecl) -> EnumItem {
    let node = d.syntax();
    EnumItem {
        name: decl_name(d.name()),
        variants: d
            .variants()
            .filter_map(|v| v.text())
            .map(SmolStr::new)
            .collect(),
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    }
}

fn lower_inner_class(d: &ast::InnerClassDecl) -> Option<InnerClassItem> {
    let node = d.syntax();
    let name = decl_name(d.name())?;
    let mut tree = d
        .body()
        .map(|b| lower_class(b.syntax(), b.decls()))
        .unwrap_or_default();
    // An inner class inlines its `extends` directly on the decl (no `ExtendsClause` wrapper),
    // so resolve it from the decl node rather than the (empty) body result.
    tree.extends = find_extends(node);
    Some(InnerClassItem {
        name,
        tree,
        range: cst::text_range_of(node),
        name_range: name_range(d.name(), node),
    })
}

fn lower_params(pl: &ast::ParamList) -> Vec<ParamItem> {
    pl.params()
        .map(|p| ParamItem {
            name: decl_name(p.name()).unwrap_or_default(),
            type_ref: p.type_ref().and_then(|t| t.text()).map(SmolStr::new),
            has_default: cst::has_token(p.syntax(), SyntaxKind::ColonEq)
                || cst::has_token(p.syntax(), SyntaxKind::Eq)
                || cst::first_child_expr(p.syntax()).is_some(),
        })
        .collect()
}

/// Find the `extends` target of `container`, in either of the two CST shapes the parser
/// produces: the top-level form wraps it in an `ExtendsClause` child node, while an inner
/// class inlines the `extends` keyword + target tokens directly on the `InnerClassDecl`. In
/// both shapes the target tokens (a `String`, or `Ident` (`.` `Ident`)*) are *direct* tokens
/// of the node we parse — the class name is wrapped in a `Name` node, never a bare token.
fn find_extends(container: &GdNode) -> Option<ExtendsRef> {
    if let Some(clause) = cst::first_child(container, |k| k == SyntaxKind::ExtendsClause) {
        return parse_extends_tokens(&clause);
    }
    if cst::has_token(container, SyntaxKind::ExtendsKw) {
        return parse_extends_tokens(container);
    }
    None
}

/// Parse the `extends` target from a node's direct tokens.
fn parse_extends_tokens(node: &GdNode) -> Option<ExtendsRef> {
    // Identifier tokens after the `extends` keyword: the dotted selectors (`A.B`, or the `.Inner`
    // trailing a string path).
    let idents: Vec<String> = node
        .children_with_tokens()
        .filter_map(cstree::util::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::Ident)
        .map(|t| t.text().to_owned())
        .collect();
    // A string literal path: `extends "res://x.gd"` — or `extends "res://x.gd".Inner`, which selects an
    // inner class we can't model yet → the seam (NOT the outer script, which would wrongly accept the
    // outer class's members).
    if let Some(s) = cst::child_token_text(node, SyntaxKind::String) {
        let path = SmolStr::new(s.trim_matches(['"', '\'']));
        return Some(if idents.is_empty() {
            ExtendsRef::ScriptPath(path)
        } else {
            ExtendsRef::ScriptPathInner(path)
        });
    }
    // Otherwise one or more dotted identifiers: `extends Node` / `extends A.B`.
    match idents.len() {
        0 => None,
        1 => Some(ExtendsRef::Name(SmolStr::new(&idents[0]))),
        _ => Some(ExtendsRef::Path(SmolStr::new(idents.join(".")))),
    }
}

fn decl_name(name: Option<ast::Name>) -> Option<SmolStr> {
    name.and_then(|n| n.text()).map(SmolStr::new)
}

/// The focus range: the name token's range, or the whole declaration's range as a fallback
/// (anonymous enums, recovered declarations).
///
/// The lossless tree flushes the inter-token whitespace *before* the identifier into the `Name`
/// node (the `Name` marker opens before the `Ident`'s advance), so `Name`'s own range carries a
/// leading-space. Trim it to the bare identifier — navigation uses this as a symbol's focus range
/// and to tag its own declaration in find-references, both of which must be the exact identifier.
fn name_range(name: Option<ast::Name>, decl: &GdNode) -> TextRange {
    name.map_or_else(
        || cst::text_range_of(decl),
        |n| trimmed_name_range(n.syntax()),
    )
}

/// `Name`'s range with the leading whitespace trivia stripped (see [`name_range`]). A `Name` is
/// `[leading-trivia][Ident]` — no trailing trivia — so trimming the front yields the identifier.
fn trimmed_name_range(name_node: &GdNode) -> TextRange {
    let r = cst::text_range_of(name_node);
    let text = name_node.text().to_string();
    let lead = u32::try_from(text.len() - text.trim_start().len()).unwrap_or(0);
    let len = u32::try_from(text.trim().len()).unwrap_or(0);
    TextRange::new(r.start + lead, r.start + lead + len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_syntax::parse;

    fn tree_of(src: &str) -> Arc<ItemTree> {
        item_tree(&parse(src).syntax_node())
    }

    #[test]
    fn class_header_and_members() {
        let tree = tree_of(
            "class_name Foo\nextends Node2D\nconst K = 1\nvar x: int\nstatic var s := 2\nsignal hit(dmg: int)\nenum E { A, B }\nfunc f(a: int, b := 1) -> void:\n\tpass\n",
        );
        assert_eq!(tree.class_name.as_deref(), Some("Foo"));
        assert_eq!(tree.extends, Some(ExtendsRef::Name(SmolStr::new("Node2D"))));
        let names: Vec<_> = tree.members.iter().filter_map(Member::name).collect();
        assert_eq!(names, vec!["K", "x", "s", "hit", "E", "f"]);
    }

    #[test]
    fn func_signature() {
        let tree = tree_of("func add(a: int, b := 1) -> int:\n\treturn a + b\n");
        let Member::Func(f) = &tree.members[0] else {
            panic!("expected func")
        };
        assert_eq!(f.name, "add");
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].type_ref.as_deref(), Some("int"));
        assert!(!f.params[0].has_default);
        assert!(f.params[1].has_default);
    }

    #[test]
    fn var_init_and_inference_flags() {
        let tree = tree_of("var a: int = 1\nvar b := 2\nvar c\nvar d = 3\n");
        let vars: Vec<&VarItem> = tree
            .members
            .iter()
            .filter_map(|m| match m {
                Member::Var(v) => Some(v),
                _ => None,
            })
            .collect();
        // a: explicit type, has init, not inferred
        assert_eq!(vars[0].type_ref.as_deref(), Some("int"));
        assert!(vars[0].has_init && !vars[0].is_inferred);
        // b: `:=` inferred, has init, no annotation
        assert!(vars[1].type_ref.is_none() && vars[1].has_init && vars[1].is_inferred);
        // c: no init, no annotation
        assert!(!vars[2].has_init && vars[2].type_ref.is_none());
        // d: untyped with init
        assert!(vars[3].has_init && !vars[3].is_inferred && vars[3].type_ref.is_none());
    }

    #[test]
    fn extends_script_path() {
        let tree = tree_of("extends \"res://player.gd\"\n");
        assert_eq!(
            tree.extends,
            Some(ExtendsRef::ScriptPath(SmolStr::new("res://player.gd")))
        );
    }

    #[test]
    fn extends_script_path_with_inner_class_is_distinguished() {
        // `extends "res://base.gd".Inner` must NOT collapse to the outer script (which would wrongly
        // accept the outer class's members); it parses to ScriptPathInner → the seam.
        let tree = tree_of("extends \"res://base.gd\".Inner\n");
        assert_eq!(
            tree.extends,
            Some(ExtendsRef::ScriptPathInner(SmolStr::new("res://base.gd"))),
            "the trailing .Inner must be detected, not dropped"
        );
    }

    #[test]
    fn anonymous_enum_has_no_name_but_variants() {
        let tree = tree_of("enum { RED, GREEN, BLUE }\n");
        let Member::Enum(e) = &tree.members[0] else {
            panic!("expected enum")
        };
        assert!(e.name.is_none());
        assert_eq!(
            e.variants,
            vec![
                SmolStr::new("RED"),
                SmolStr::new("GREEN"),
                SmolStr::new("BLUE")
            ]
        );
    }

    #[test]
    fn inner_class_members_and_extends() {
        let tree = tree_of("class Inner extends RefCounted:\n\tvar y = 2\n\tfunc m():\n\t\tpass\n");
        let Member::Class(inner) = &tree.members[0] else {
            panic!("expected inner class")
        };
        assert_eq!(inner.name, "Inner");
        let names: Vec<_> = inner.tree.members.iter().filter_map(Member::name).collect();
        assert_eq!(names, vec!["y", "m"]);
        assert_eq!(
            inner.tree.extends,
            Some(ExtendsRef::Name(SmolStr::new("RefCounted")))
        );
    }

    #[test]
    fn ptr_round_trips_to_node() {
        let parse = parse("func f():\n\tpass\n");
        let root = parse.syntax_node();
        let tree = item_tree(&root);
        let Member::Func(f) = &tree.members[0] else {
            panic!()
        };
        let node = f.ptr.to_node(&root).expect("func node recovered");
        assert_eq!(node.kind(), SyntaxKind::FuncDecl);
    }
}
