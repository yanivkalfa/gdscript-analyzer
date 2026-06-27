//! WS4 — the typed AST.
//!
//! A thin, zero-cost typed *view* over the lossless CST (the rust-analyzer model). An
//! AST node just wraps a red [`GdNode`] of the matching [`SyntaxKind`]; accessors are
//! filtered child lookups. No data is copied. Because [`GdNode`] is a *resolved* node
//! (it carries the interner), text accessors are clean — `Name::text()` needs no extra
//! resolver argument.

use cstree::util::NodeOrToken;

use crate::SyntaxKind;
use crate::syntax_kind::{GdNode, GdToken};

/// A typed node: a checked view over a red node of one [`SyntaxKind`].
pub trait AstNode: Sized {
    /// Whether a node of `kind` can be viewed as `Self`.
    fn can_cast(kind: SyntaxKind) -> bool;
    /// View `node` as `Self`, if its kind matches.
    fn cast(node: GdNode) -> Option<Self>;
    /// The underlying red node.
    fn syntax(&self) -> &GdNode;
}

/// Generate a single-kind AST wrapper struct + its [`AstNode`] impl.
macro_rules! ast_node {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name(GdNode);

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$name
            }
            fn cast(node: GdNode) -> Option<Self> {
                if node.kind() == SyntaxKind::$name {
                    Some(Self(node))
                } else {
                    None
                }
            }
            fn syntax(&self) -> &GdNode {
                &self.0
            }
        }
    };
}

ast_node!(
    /// The whole file.
    SourceFile
);
ast_node!(ClassNameDecl);
ast_node!(ExtendsClause);
ast_node!(Annotation);
ast_node!(FuncDecl);
ast_node!(VarDecl);
ast_node!(ConstDecl);
ast_node!(EnumDecl);
ast_node!(EnumVariant);
ast_node!(SignalDecl);
ast_node!(InnerClassDecl);
ast_node!(ClassBody);
ast_node!(ParamList);
ast_node!(Param);
ast_node!(Block);
ast_node!(TypeRef);
ast_node!(
    /// A declaration's name (wraps the declared identifier).
    Name
);

// ---- generic navigation helpers -------------------------------------------------

/// The first child node castable to `N`.
fn child<N: AstNode>(node: &GdNode) -> Option<N> {
    node.children().find_map(|c| N::cast(c.clone()))
}

/// All child nodes castable to `N`.
fn children<N: AstNode>(node: &GdNode) -> impl Iterator<Item = N> + '_ {
    node.children().filter_map(|c| N::cast(c.clone()))
}

/// Whether `node` has a direct child token of `kind`.
fn has_token(node: &GdNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .any(|t| t.kind() == kind)
}

/// The text of the first direct child token of `kind`.
fn token_text(node: &GdNode, kind: SyntaxKind) -> Option<String> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
        .map(|t| t.text().to_owned())
}

/// The text of the first direct child token usable as a *name* — the grammar's `at_name`
/// whitelist: an `Ident`, or one of the soft keywords `match`/`when` (Godot's `is_identifier()`
/// permits both as identifiers). Without this, a symbol named `match`/`when` is silently dropped at
/// the AST layer because its token is `MatchKw`/`WhenKw`, not `Ident`. See `TECH_DEBT.md`.
fn name_token_text(node: &GdNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::Ident | SyntaxKind::MatchKw | SyntaxKind::WhenKw
            )
        })
        .map(|t| t.text().to_owned())
}

// ---- accessors ------------------------------------------------------------------

impl Name {
    /// The identifier text (incl. the soft-keyword names `match`/`when`).
    #[must_use]
    pub fn text(&self) -> Option<String> {
        name_token_text(&self.0)
    }
}

impl SourceFile {
    /// The top-level declarations, in source order.
    pub fn decls(&self) -> impl Iterator<Item = Decl> + '_ {
        self.0.children().filter_map(|c| Decl::cast(c.clone()))
    }
}

impl FuncDecl {
    /// The function name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The parameter list.
    #[must_use]
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    /// The body block.
    #[must_use]
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
    /// The declared return type, if any (the `TypeRef` after `->`).
    #[must_use]
    pub fn return_type(&self) -> Option<TypeRef> {
        child(&self.0)
    }
    /// Whether this is a `static func`.
    #[must_use]
    pub fn is_static(&self) -> bool {
        has_token(&self.0, SyntaxKind::StaticKw)
    }
}

impl ParamList {
    /// The parameters (excludes vararg rest params).
    pub fn params(&self) -> impl Iterator<Item = Param> + '_ {
        children(&self.0)
    }
}

impl Param {
    /// The parameter name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The declared type, if any.
    #[must_use]
    pub fn type_ref(&self) -> Option<TypeRef> {
        child(&self.0)
    }
}

impl VarDecl {
    /// The variable name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The declared type, if any.
    #[must_use]
    pub fn type_ref(&self) -> Option<TypeRef> {
        child(&self.0)
    }
    /// Whether this is a `static var`.
    #[must_use]
    pub fn is_static(&self) -> bool {
        has_token(&self.0, SyntaxKind::StaticKw)
    }
}

impl ConstDecl {
    /// The constant name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
}

impl EnumDecl {
    /// The enum's name, if it is a named enum.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The enum variants.
    pub fn variants(&self) -> impl Iterator<Item = EnumVariant> + '_ {
        children(&self.0)
    }
}

impl EnumVariant {
    /// The variant name (incl. the soft-keyword names `match`/`when`).
    #[must_use]
    pub fn text(&self) -> Option<String> {
        name_token_text(&self.0)
    }
}

impl SignalDecl {
    /// The signal name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The typed parameter list, if any.
    #[must_use]
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
}

impl ClassNameDecl {
    /// The registered global class name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
}

impl InnerClassDecl {
    /// The inner class name.
    #[must_use]
    pub fn name(&self) -> Option<Name> {
        child(&self.0)
    }
    /// The class body (its members), if present.
    #[must_use]
    pub fn body(&self) -> Option<ClassBody> {
        child(&self.0)
    }
}

impl ClassBody {
    /// The member declarations.
    pub fn decls(&self) -> impl Iterator<Item = Decl> + '_ {
        self.0.children().filter_map(|c| Decl::cast(c.clone()))
    }
}

impl Annotation {
    /// The annotation name (the identifier after `@`).
    #[must_use]
    pub fn name(&self) -> Option<String> {
        token_text(&self.0, SyntaxKind::Ident)
    }
}

impl TypeRef {
    /// The leading type identifier (e.g. `int`, `Array`).
    #[must_use]
    pub fn text(&self) -> Option<String> {
        self.0
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::VoidKw))
            .map(|t| t.text().to_owned())
    }
}

/// Any top-level or class-body declaration — the unit `document_symbols` iterates.
#[derive(Debug, Clone)]
pub enum Decl {
    /// `class_name X`
    ClassName(ClassNameDecl),
    /// `func f(...)`
    Func(FuncDecl),
    /// `var x`
    Var(VarDecl),
    /// `const X`
    Const(ConstDecl),
    /// `enum E { ... }`
    Enum(EnumDecl),
    /// `signal s`
    Signal(SignalDecl),
    /// `class Inner: ...`
    Class(InnerClassDecl),
}

impl Decl {
    /// View a node as a declaration, if it is one.
    #[must_use]
    pub fn cast(node: GdNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::ClassNameDecl => ClassNameDecl::cast(node).map(Self::ClassName),
            SyntaxKind::FuncDecl => FuncDecl::cast(node).map(Self::Func),
            SyntaxKind::VarDecl => VarDecl::cast(node).map(Self::Var),
            SyntaxKind::ConstDecl => ConstDecl::cast(node).map(Self::Const),
            SyntaxKind::EnumDecl => EnumDecl::cast(node).map(Self::Enum),
            SyntaxKind::SignalDecl => SignalDecl::cast(node).map(Self::Signal),
            SyntaxKind::InnerClassDecl => InnerClassDecl::cast(node).map(Self::Class),
            _ => None,
        }
    }

    /// The declaration's underlying node.
    #[must_use]
    pub fn syntax(&self) -> &GdNode {
        match self {
            Self::ClassName(d) => d.syntax(),
            Self::Func(d) => d.syntax(),
            Self::Var(d) => d.syntax(),
            Self::Const(d) => d.syntax(),
            Self::Enum(d) => d.syntax(),
            Self::Signal(d) => d.syntax(),
            Self::Class(d) => d.syntax(),
        }
    }

    /// The declaration's name, if it has one.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        let name = match self {
            Self::ClassName(d) => d.name(),
            Self::Func(d) => d.name(),
            Self::Var(d) => d.name(),
            Self::Const(d) => d.name(),
            Self::Enum(d) => d.name(),
            Self::Signal(d) => d.name(),
            Self::Class(d) => d.name(),
        };
        name.and_then(|n| n.text())
    }
}

/// A pre-order walk over every node in the tree (depth-first), for visitors that need
/// to inspect all declarations/blocks (e.g. folding ranges).
#[must_use]
pub fn descendants(root: &GdNode) -> Vec<GdNode> {
    let mut out = vec![root.clone()];
    for child in root.children() {
        out.extend(descendants(child));
    }
    out
}

/// The token (if any) at `offset`, right-biased — the completion-context probe.
#[must_use]
pub fn token_at(root: &GdNode, offset: text_size::TextSize) -> Option<GdToken> {
    root.token_at_offset(offset).right_biased()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn func_accessors() {
        let parse = parse("static func add(a: int, b := 1) -> int:\n\treturn a + b\n");
        let file = SourceFile::cast(parse.syntax_node()).unwrap();
        let func = file
            .decls()
            .find_map(|d| match d {
                Decl::Func(f) => Some(f),
                _ => None,
            })
            .unwrap();
        assert!(func.is_static());
        assert_eq!(func.name().and_then(|n| n.text()).as_deref(), Some("add"));
        assert_eq!(
            func.return_type().and_then(|t| t.text()).as_deref(),
            Some("int")
        );
        let params: Vec<_> = func
            .param_list()
            .unwrap()
            .params()
            .filter_map(|p| p.name().and_then(|n| n.text()))
            .collect();
        assert_eq!(params, vec!["a", "b"]);
        assert!(func.body().is_some());
    }

    #[test]
    fn declarations_are_enumerated() {
        let parse = parse(
            "class_name Foo\nconst K = 1\nvar x: int\nsignal s\nenum E { A, B }\nfunc f():\n\tpass\nclass Inner:\n\tvar y = 2\n",
        );
        let file = SourceFile::cast(parse.syntax_node()).unwrap();
        let names: Vec<_> = file.decls().map(|d| d.name().unwrap_or_default()).collect();
        assert_eq!(names, vec!["Foo", "K", "x", "s", "E", "f", "Inner"]);
    }

    #[test]
    fn enum_variants_and_inner_class_members() {
        let parse =
            parse("enum E { A, B = 5, C }\nclass Inner:\n\tvar a = 1\n\tfunc m():\n\t\tpass\n");
        let file = SourceFile::cast(parse.syntax_node()).unwrap();

        let en = file
            .decls()
            .find_map(|d| match d {
                Decl::Enum(e) => Some(e),
                _ => None,
            })
            .unwrap();
        let variants: Vec<_> = en.variants().filter_map(|v| v.text()).collect();
        assert_eq!(variants, vec!["A", "B", "C"]);

        let inner = file
            .decls()
            .find_map(|d| match d {
                Decl::Class(c) => Some(c),
                _ => None,
            })
            .unwrap();
        let member_names: Vec<_> = inner
            .body()
            .unwrap()
            .decls()
            .map(|d| d.name().unwrap_or_default())
            .collect();
        assert_eq!(member_names, vec!["a", "m"]);
    }

    #[test]
    fn token_at_offset_finds_identifier() {
        let src = "var hello = 1\n";
        let parse = parse(src);
        let node = parse.syntax_node();
        // offset 5 is inside "hello" (chars 4..9).
        let tok = node
            .token_at_offset(text_size::TextSize::new(5))
            .right_biased()
            .unwrap();
        assert_eq!(tok.kind(), SyntaxKind::Ident);
        assert_eq!(tok.text(), "hello");
    }
}
