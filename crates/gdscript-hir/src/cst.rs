//! Low-level helpers over the Phase-1 cstree CST, shared by the HIR lowering passes
//! ([`crate::item_tree`], [`crate::body`], [`crate::infer`]).
//!
//! The typed AST ([`gdscript_syntax::ast`]) only models *declarations*; expressions and
//! statements are walked here at the raw [`GdNode`]/[`SyntaxKind`] level. Everything in
//! this module is a pure function of the tree — no interning, no engine API.

use cstree::util::NodeOrToken;
use gdscript_base::TextRange;
use gdscript_syntax::{GdNode, GdToken, SyntaxKind};

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
    ///
    /// Prunes by range — only descends into the one child subtree that contains the target — so
    /// recovery is ~O(tree depth), not O(nodes). This is on the hot path (called per function /
    /// field / `TypeRef` during inference), so the pruning is what keeps single-file analysis
    /// within the warm budget.
    #[must_use]
    pub fn to_node(self, root: &GdNode) -> Option<GdNode> {
        find_node(root, self)
    }
}

fn find_node(node: &GdNode, ptr: AstPtr) -> Option<GdNode> {
    let range = text_range_of(node);
    if node.kind() == ptr.kind && range == ptr.range {
        return Some(node.clone());
    }
    // Only descend into the subtree that fully contains the target's range.
    if range.start <= ptr.range.start && range.end >= ptr.range.end {
        for child in node.children() {
            if let Some(found) = find_node(child, ptr) {
                return Some(found);
            }
        }
    }
    None
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
    node.children()
        .find_map(|c| pred(c.kind()).then(|| c.clone()))
}

/// The first direct child node that is an expression.
#[must_use]
pub fn first_child_expr(node: &GdNode) -> Option<GdNode> {
    first_child(node, is_expr_kind)
}

/// All direct child nodes that are expressions, in source order.
#[must_use]
pub fn child_exprs(node: &GdNode) -> Vec<GdNode> {
    node.children()
        .filter(|c| is_expr_kind(c.kind()))
        .cloned()
        .collect()
}

/// All direct child nodes of the given `kind`, in source order.
#[must_use]
pub fn children_of(node: &GdNode, kind: SyntaxKind) -> Vec<GdNode> {
    node.children()
        .filter(|c| c.kind() == kind)
        .cloned()
        .collect()
}

/// The head `Ident` token of `node`'s `extends Base[.Inner]` target — the bare identifier directly
/// after the `extends` keyword (in an `ExtendsClause`, a `ClassNameDecl`, or an inner-class decl
/// that inlines its `extends`). `None` when there is no `extends`, or the target is a string path
/// (`extends "res://x.gd"`). The head names the base *class*; trailing `.Inner` segments are
/// excluded (only the first `Ident` after `extends` is returned).
#[must_use]
pub fn extends_head_token(node: &GdNode) -> Option<GdToken> {
    let mut after_extends = false;
    for t in node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
    {
        if t.kind() == SyntaxKind::ExtendsKw {
            after_extends = true;
        } else if after_extends && t.kind() == SyntaxKind::Ident {
            return Some(t.clone());
        }
    }
    None
}

/// The first meaningful (non-trivia, non-layout) token of `node`.
#[must_use]
pub fn first_token(node: &GdNode) -> Option<GdToken> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia() && !t.kind().is_synthetic_layout())
        .cloned()
}

/// The [`TextRange`] of a token.
#[must_use]
pub fn token_range(token: &GdToken) -> TextRange {
    let r = token.text_range();
    TextRange::new(u32::from(r.start()), u32::from(r.end()))
}
