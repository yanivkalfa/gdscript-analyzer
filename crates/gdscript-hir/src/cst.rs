//! Low-level helpers over the Phase-1 cstree CST, shared by the HIR lowering passes
//! ([`crate::item_tree`], [`crate::body`], [`crate::infer`]).
//!
//! The typed AST ([`gdscript_syntax::ast`]) only models *declarations*; expressions and
//! statements are walked here at the raw [`GdNode`]/[`SyntaxKind`] level. Everything in
//! this module is a pure function of the tree — no interning, no engine API.

use cstree::util::NodeOrToken;
use gdscript_base::TextRange;
use gdscript_syntax::ast;
use gdscript_syntax::{GdNode, SyntaxKind};

/// A reparse-stable pointer to a syntax node — its [`SyntaxKind`] plus byte [`TextRange`]
/// (rust-analyzer's `SyntaxNodePtr`). Because it is plain `Copy` data keyed on text
/// position, identical source re-parses to the identical pointer, which is what lets the
/// [`crate::item_tree::ItemTree`] stay `Eq` while still being able to recover the CST node
/// for deferred body lowering / initializer inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AstPtr {
    /// The pointed-to node's kind.
    pub kind: SyntaxKind,
    /// The pointed-to node's byte range.
    pub range: TextRange,
}

impl AstPtr {
    /// The pointer to `node`.
    #[must_use]
    pub fn of(node: &GdNode) -> Self {
        Self {
            kind: node.kind(),
            range: text_range_of(node),
        }
    }

    /// Recover the node this pointer refers to, searching from `root`. `None` if the tree
    /// no longer contains a node of the matching kind + range (e.g. recovered from a stale
    /// pointer against edited text).
    #[must_use]
    pub fn to_node(self, root: &GdNode) -> Option<GdNode> {
        ast::descendants(root).into_iter().find(|n| {
            n.kind() == self.kind && text_range_of(n) == self.range
        })
    }
}

/// The [`gdscript_base::TextRange`] of `node` (converted from the `text_size` range the CST
/// carries).
#[must_use]
pub fn text_range_of(node: &GdNode) -> TextRange {
    let r = node.text_range();
    TextRange::new(u32::from(r.start()), u32::from(r.end()))
}

/// Whether `node` has a direct child token of `kind`.
#[must_use]
pub fn has_token(node: &GdNode, kind: SyntaxKind) -> bool {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .any(|t| t.kind() == kind)
}

/// The text of the first direct child token of `kind`.
#[must_use]
pub fn child_token_text(node: &GdNode, kind: SyntaxKind) -> Option<String> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
        .map(|t| t.text().to_owned())
}

/// Whether `kind` names an expression node (the unit the body lowerer turns into an
/// `Expr`). Excludes `ArgList`/`DictEntry`/`ErrorNode` (structural, not values).
#[must_use]
pub fn is_expr_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::BinExpr
            | SyntaxKind::UnaryExpr
            | SyntaxKind::TernaryExpr
            | SyntaxKind::CastExpr
            | SyntaxKind::IsExpr
            | SyntaxKind::InExpr
            | SyntaxKind::CallExpr
            | SyntaxKind::IndexExpr
            | SyntaxKind::FieldExpr
            | SyntaxKind::AwaitExpr
            | SyntaxKind::Literal
            | SyntaxKind::NameRef
            | SyntaxKind::ArrayLit
            | SyntaxKind::DictLit
            | SyntaxKind::LambdaExpr
            | SyntaxKind::ParenExpr
            | SyntaxKind::PreloadExpr
            | SyntaxKind::GetNodeExpr
            | SyntaxKind::UniqueNodeExpr
    )
}

/// The first direct child node satisfying `pred`.
pub fn first_child(node: &GdNode, pred: impl Fn(SyntaxKind) -> bool) -> Option<GdNode> {
    node.children().find_map(|c| pred(c.kind()).then(|| c.clone()))
}

/// The first direct child node that is an expression.
#[must_use]
pub fn first_child_expr(node: &GdNode) -> Option<GdNode> {
    first_child(node, is_expr_kind)
}
