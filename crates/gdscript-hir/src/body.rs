//! Body lowering (Playbook §3.1/§3.4): a function body (or a class-level initializer
//! expression) lowered from the CST into a flat arena of [`Expr`]/[`Stmt`] addressed by
//! [`ExprId`]/[`StmtId`], plus a [`BodySourceMap`] mapping every [`ExprId`] back to its byte
//! range. Every IDE feature (hover, inlay, completion) maps a cursor offset → `ExprId` through
//! this source map, then reads the inferred type from [`crate::infer`].
//!
//! Lowering is a pure function of the CST: no engine API, no name resolution, no types. Type
//! annotations (`is`/`as`/`var`/param/`for`) are kept as [`AstPtr`]s to their `TypeRef` nodes
//! and resolved later, so this stage never depends on the model.

use gdscript_base::TextRange;
use gdscript_syntax::ast::{self, AstNode};
use gdscript_syntax::{GdNode, SyntaxKind};
use smol_str::SmolStr;

use crate::cst::{self, AstPtr};

/// An index into [`Body::exprs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub u32);

/// An index into [`Body::stmts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StmtId(pub u32);

/// A lowered block: its statements, in order.
pub type Block = Vec<StmtId>;

/// A literal's kind (the value text lives in the CST; only the kind drives typing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Literal {
    /// An integer literal.
    Int,
    /// A float literal.
    Float,
    /// `true` / `false` (carries the value, for constant checks like `ASSERT_ALWAYS_*`).
    Bool(bool),
    /// A `String` literal.
    Str,
    /// A `&"…"` `StringName` literal.
    StringName,
    /// A `^"…"` `NodePath` literal.
    NodePath,
    /// `null`.
    Null,
    /// `PI` / `TAU` / `INF` / `NAN` (a `float`).
    MathConst,
}

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `**`
    Pow,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    Le,
    /// `>=`
    Ge,
    /// `and` / `&&`
    And,
    /// `or` / `||`
    Or,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `=` (assignment) or any compound assignment (`+=`, `<<=`, …).
    Assign,
}

impl BinOp {
    /// Map an operator token kind to a [`BinOp`]. Compound assignments collapse to
    /// [`BinOp::Assign`] (the typing rule is the same: check the RHS against the LHS slot).
    #[must_use]
    pub fn from_token(kind: SyntaxKind) -> Option<Self> {
        use SyntaxKind as K;
        Some(match kind {
            K::Plus => Self::Add,
            K::Minus => Self::Sub,
            K::Star => Self::Mul,
            K::Slash => Self::Div,
            K::Percent => Self::Mod,
            K::StarStar => Self::Pow,
            K::EqEq => Self::Eq,
            K::Neq => Self::Ne,
            K::Lt => Self::Lt,
            K::Gt => Self::Gt,
            K::Le => Self::Le,
            K::Ge => Self::Ge,
            K::AndKw | K::AmpAmp => Self::And,
            K::OrKw | K::PipePipe => Self::Or,
            K::Amp => Self::BitAnd,
            K::Pipe => Self::BitOr,
            K::Caret => Self::BitXor,
            K::Shl => Self::Shl,
            K::Shr => Self::Shr,
            K::Eq
            | K::PlusEq
            | K::MinusEq
            | K::StarEq
            | K::SlashEq
            | K::PercentEq
            | K::StarStarEq
            | K::AmpEq
            | K::PipeEq
            | K::CaretEq
            | K::ShlEq
            | K::ShrEq => Self::Assign,
            _ => return None,
        })
    }

    /// Whether this is an arithmetic operator (`+ - * / % **`).
    #[must_use]
    pub fn is_arithmetic(self) -> bool {
        matches!(
            self,
            Self::Add | Self::Sub | Self::Mul | Self::Div | Self::Mod | Self::Pow
        )
    }

    /// Whether this is a comparison / logical operator (result is `bool`).
    #[must_use]
    pub fn is_boolean(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Gt | Self::Le | Self::Ge | Self::And | Self::Or
        )
    }
}

/// A prefix unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    /// `+`
    Pos,
    /// `not` / `!`
    Not,
    /// `~`
    BitNot,
}

impl UnOp {
    /// Map a prefix operator token kind to a [`UnOp`].
    #[must_use]
    pub fn from_token(kind: SyntaxKind) -> Option<Self> {
        Some(match kind {
            SyntaxKind::Minus => Self::Neg,
            SyntaxKind::Plus => Self::Pos,
            SyntaxKind::NotKw | SyntaxKind::Bang => Self::Not,
            SyntaxKind::Tilde => Self::BitNot,
            _ => return None,
        })
    }
}

/// A lowered expression. Children are referenced by [`ExprId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// An unlowerable / recovered expression (typed `Error`, suppresses cascade).
    Missing,
    /// A literal.
    Literal(Literal),
    /// A bare identifier reference.
    Name(SmolStr),
    /// `self`.
    SelfExpr,
    /// `super`.
    Super,
    /// A binary expression.
    Bin {
        /// The operator.
        op: BinOp,
        /// Left operand.
        lhs: ExprId,
        /// Right operand.
        rhs: ExprId,
    },
    /// A prefix unary expression.
    Unary {
        /// The operator.
        op: UnOp,
        /// The operand.
        operand: ExprId,
    },
    /// `a if c else b`.
    Ternary {
        /// The condition.
        cond: ExprId,
        /// Value when the condition holds.
        then_branch: ExprId,
        /// Value otherwise.
        else_branch: ExprId,
    },
    /// `callee(args…)`.
    Call {
        /// The callee.
        callee: ExprId,
        /// The argument expressions.
        args: Vec<ExprId>,
    },
    /// `receiver.name`.
    Field {
        /// The receiver.
        receiver: ExprId,
        /// The member name.
        name: SmolStr,
        /// The member-name token range (for hover / member-completion context).
        name_range: TextRange,
    },
    /// `base[index]`.
    Index {
        /// The indexed value.
        base: ExprId,
        /// The index expression.
        index: ExprId,
    },
    /// `operand is [not] T` — always `bool`; narrows on the true branch.
    Is {
        /// The tested operand.
        operand: ExprId,
        /// The `TypeRef` node tested against.
        ty: Option<AstPtr>,
        /// Whether it was `is not`.
        negated: bool,
    },
    /// `operand as T` — optimistic downcast to `T`.
    Cast {
        /// The operand.
        operand: ExprId,
        /// The target `TypeRef` node.
        ty: Option<AstPtr>,
    },
    /// `lhs [not] in rhs` — always `bool`.
    In {
        /// The needle.
        lhs: ExprId,
        /// The haystack.
        rhs: ExprId,
        /// Whether it was `not in`.
        negated: bool,
    },
    /// `await operand`.
    Await(ExprId),
    /// `[a, b, …]`.
    Array(Vec<ExprId>),
    /// `{ k: v, … }` (value is `None` only on recovery).
    Dict(Vec<(ExprId, Option<ExprId>)>),
    /// `func(...): …` — an anonymous function (typed `Callable`).
    Lambda {
        /// The lambda parameters.
        params: Vec<ParamBinding>,
        /// The lambda body.
        body: Block,
    },
    /// `preload(path)` — a compile-time resource reference. When `path` is a constant string
    /// literal (the only form Godot accepts), it is captured here so inference can resolve it to
    /// the declaring file's `ScriptRef` (M3); a non-literal argument leaves `path` `None` (the
    /// seam).
    Preload {
        /// The lowered path argument expression, if present (kept so it is still type-walked).
        arg: Option<ExprId>,
        /// The constant-folded path string (unquoted), when the argument is a string literal.
        path: Option<SmolStr>,
    },
    /// `$Path` / `%Unique` / `get_node("…")` — a node-path access. In Phase 2 this was always
    /// `Object(Node)`; Phase-4 M1 resolves the literal path against the owning scene to the node's
    /// concrete type. A computed `get_node(var)` keeps `path: None` (stays `Node`, never warns).
    GetNode {
        /// The literal node path (`"Panel/VBox/Button"`, or a `%Unique` name), or `None` if computed.
        path: Option<SmolStr>,
        /// `true` for the `%Unique` form (resolve via `unique_name_in_owner`); `false` for `$Path`.
        unique: bool,
    },
    /// `(inner)`.
    Paren(ExprId),
}

/// A function / lambda parameter binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamBinding {
    /// The parameter name.
    pub name: SmolStr,
    /// The `TypeRef` annotation node, if written.
    pub type_ref: Option<AstPtr>,
    /// The default-value expression, if written.
    pub default: Option<ExprId>,
    /// The name token range.
    pub name_range: TextRange,
}

/// A local `var` / `const` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVar {
    /// The binding name.
    pub name: SmolStr,
    /// The `TypeRef` annotation node, if written.
    pub type_ref: Option<AstPtr>,
    /// The initializer expression, if written.
    pub init: Option<ExprId>,
    /// Whether the type was inferred with `:=`.
    pub is_inferred: bool,
    /// Whether this is a `const`.
    pub is_const: bool,
    /// The name token range.
    pub name_range: TextRange,
}

/// A `for var [: T] in iter:` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForLoop {
    /// The loop variable name.
    pub var: SmolStr,
    /// The loop variable's `TypeRef` annotation node (4.2+ `for x: T in …`), if written.
    pub var_type: Option<AstPtr>,
    /// The loop variable's name token range.
    pub var_range: TextRange,
    /// The iterated expression.
    pub iter: ExprId,
    /// The loop body.
    pub body: Block,
}

/// One `var x` capture in a `match` pattern — a local binding (so navigation can find/rename it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchBind {
    /// The captured name.
    pub name: SmolStr,
    /// The capture's name-token range (may carry leading whitespace, like other body bindings).
    pub range: TextRange,
}

/// One `match` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    /// Names bound by `var x` patterns in this arm (typed `Variant` in Phase 2).
    pub binds: Vec<MatchBind>,
    /// The `when` guard expression, if any.
    pub guard: Option<ExprId>,
    /// The arm body.
    pub body: Block,
    /// The arm's byte range (the `UNREACHABLE_PATTERN` anchor).
    pub range: TextRange,
    /// Whether this arm is an **unconditional catch-all** — its sole top-level pattern is `_` or a
    /// `var x` bind, with no `when` guard. Every arm *after* a catch-all is `UNREACHABLE_PATTERN`.
    pub is_catch_all: bool,
}

/// Whether a `match` arm is an UNCONDITIONAL catch-all — its **sole top-level** pattern is `_` (a
/// `PatternLiteral`/`PatternWildcard` whose only token is `_`) or a `var x` bind (`PatternBind`),
/// and it has no `when` guard. Conservative: a multi-pattern arm (`1, _:`), a `_`/`var` nested in an
/// array/dict pattern, or a guarded arm is NOT a catch-all — we under-warn `UNREACHABLE_PATTERN`
/// rather than risk flagging a reachable arm (a false positive on valid code).
fn arm_is_unconditional_catch_all(arm: &GdNode) -> bool {
    use SyntaxKind as K;
    if cst::first_child(arm, |k| k == K::PatternGuard).is_some() {
        return false;
    }
    let patterns: Vec<&GdNode> = arm
        .children()
        .filter(|c| {
            matches!(
                c.kind(),
                K::PatternBind
                    | K::PatternLiteral
                    | K::PatternWildcard
                    | K::PatternArray
                    | K::PatternDict
                    | K::PatternRest
            )
        })
        .collect();
    let [only] = patterns.as_slice() else {
        return false;
    };
    match only.kind() {
        K::PatternBind | K::PatternWildcard => true,
        // `_` parses as a `PatternLiteral` wrapping the identifier expr `_` (a `NameRef` node), so
        // the `_` token is nested one level down — check the inner expr's first token.
        K::PatternLiteral => cst::first_child_expr(only)
            .and_then(|e| cst::first_token(&e))
            .is_some_and(|t| t.text() == "_"),
        _ => false,
    }
}

/// A lowered statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// An expression statement.
    Expr(ExprId),
    /// A local `var` / `const`.
    Var(LocalVar),
    /// `return [expr]`.
    Return(Option<ExprId>),
    /// `if … elif … else …`.
    If {
        /// The `if` condition.
        cond: ExprId,
        /// The `if` branch.
        then_branch: Block,
        /// The `elif` branches.
        elifs: Vec<(ExprId, Block)>,
        /// The `else` branch.
        else_branch: Option<Block>,
    },
    /// `while cond:`.
    While {
        /// The loop condition.
        cond: ExprId,
        /// The loop body.
        body: Block,
    },
    /// `for …:`.
    For(ForLoop),
    /// `match …:`.
    Match {
        /// The matched value.
        scrutinee: ExprId,
        /// The arms.
        arms: Vec<MatchArm>,
    },
    /// `break`.
    Break,
    /// `continue`.
    Continue,
    /// `pass` / `breakpoint`.
    Pass,
    /// `assert(cond[, msg])`.
    Assert(Option<ExprId>),
}

/// Maps every [`ExprId`]/[`StmtId`] back to its source byte range. The reverse direction (offset →
/// `ExprId`) is the tightest containing expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodySourceMap {
    expr_ranges: Vec<TextRange>,
    stmt_ranges: Vec<TextRange>,
}

impl BodySourceMap {
    /// The source range of an expression.
    #[must_use]
    pub fn expr_range(&self, id: ExprId) -> TextRange {
        self.expr_ranges[id.0 as usize]
    }

    /// The source range of a statement (the whole statement node — the `UNREACHABLE_CODE` anchor).
    #[must_use]
    pub fn stmt_range(&self, id: StmtId) -> TextRange {
        self.stmt_ranges[id.0 as usize]
    }

    /// The innermost (tightest) expression whose range contains `offset`.
    #[must_use]
    pub fn expr_at_offset(&self, offset: u32) -> Option<ExprId> {
        self.expr_ranges
            .iter()
            .enumerate()
            .filter(|(_, r)| r.start <= offset && offset < r.end)
            .min_by_key(|(_, r)| r.end - r.start)
            .map(|(i, _)| ExprId(u32::try_from(i).unwrap_or(u32::MAX)))
    }

    /// The expression whose range exactly equals `range` (for mapping a CST node back to its
    /// `ExprId` — e.g. a member-completion receiver).
    #[must_use]
    pub fn expr_for_range(&self, range: TextRange) -> Option<ExprId> {
        self.expr_ranges
            .iter()
            .position(|r| *r == range)
            .map(|i| ExprId(u32::try_from(i).unwrap_or(u32::MAX)))
    }
}

/// A lowered function body, or a single class-level initializer expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body {
    /// The expression arena.
    pub exprs: Vec<Expr>,
    /// The statement arena.
    pub stmts: Vec<Stmt>,
    /// The function parameters (empty for an initializer body).
    pub params: Vec<ParamBinding>,
    /// The top-level statements (empty for an initializer body).
    pub block: Block,
    /// A bare initializer expression (class-level `var`/`const`); `None` for a function body.
    pub tail: Option<ExprId>,
    /// The expr → range map.
    pub source_map: BodySourceMap,
}

impl Body {
    /// The expression behind an id.
    #[must_use]
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }

    /// The statement behind an id.
    #[must_use]
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id.0 as usize]
    }
}

/// Lower a `FuncDecl` node into a [`Body`].
#[must_use]
pub fn body_of_func(func: &GdNode) -> Body {
    let mut low = Lowerer::default();
    let decl = ast::FuncDecl::cast(func.clone());
    let params = decl
        .as_ref()
        .and_then(ast::FuncDecl::param_list)
        .map(|pl| low.lower_params(pl.syntax()))
        .unwrap_or_default();
    let block = decl
        .as_ref()
        .and_then(ast::FuncDecl::body)
        .map(|b| low.lower_block(b.syntax()))
        .unwrap_or_default();
    low.finish(params, block, None)
}

/// Lower a single expression node into a [`Body`] (a class-level `var`/`const` initializer).
#[must_use]
pub fn body_of_expr(expr: &GdNode) -> Body {
    let mut low = Lowerer::default();
    let tail = low.lower_expr(expr);
    low.finish(Vec::new(), Vec::new(), Some(tail))
}

/// Lower a class-level `VarDecl`/`ConstDecl` node into a [`Body`] holding one local-var
/// statement — so [`crate::infer`] runs the full annotation/inference checks (and records the
/// member's binding type) on a class field the same way it does for a local.
#[must_use]
pub fn body_of_decl_stmt(decl: &GdNode) -> Body {
    let mut low = Lowerer::default();
    let block = low.lower_stmt(decl).into_iter().collect();
    low.finish(Vec::new(), block, None)
}

/// Recover the function node for `ptr` from `root` and lower its body.
#[must_use]
pub fn body(root: &GdNode, ptr: AstPtr) -> Option<Body> {
    let node = ptr.to_node(root)?;
    Some(body_of_func(&node))
}

#[derive(Default)]
struct Lowerer {
    exprs: Vec<Expr>,
    stmts: Vec<Stmt>,
    expr_ranges: Vec<TextRange>,
    stmt_ranges: Vec<TextRange>,
}

impl Lowerer {
    fn finish(self, params: Vec<ParamBinding>, block: Block, tail: Option<ExprId>) -> Body {
        Body {
            exprs: self.exprs,
            stmts: self.stmts,
            params,
            block,
            tail,
            source_map: BodySourceMap {
                expr_ranges: self.expr_ranges,
                stmt_ranges: self.stmt_ranges,
            },
        }
    }

    fn alloc_expr(&mut self, expr: Expr, range: TextRange) -> ExprId {
        let id = ExprId(u32::try_from(self.exprs.len()).unwrap_or(u32::MAX));
        self.exprs.push(expr);
        self.expr_ranges.push(range);
        id
    }

    fn alloc_stmt(&mut self, stmt: Stmt, range: TextRange) -> StmtId {
        let id = StmtId(u32::try_from(self.stmts.len()).unwrap_or(u32::MAX));
        self.stmts.push(stmt);
        self.stmt_ranges.push(range);
        id
    }

    fn missing(&mut self, range: TextRange) -> ExprId {
        self.alloc_expr(Expr::Missing, range)
    }

    /// Lower the first child expression, or a `Missing` placeholder spanning `node`.
    fn lower_first_expr(&mut self, node: &GdNode) -> ExprId {
        match cst::first_child_expr(node) {
            Some(c) => self.lower_expr(&c),
            None => self.missing(cst::text_range_of(node)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn lower_expr(&mut self, node: &GdNode) -> ExprId {
        use SyntaxKind as K;
        let range = cst::text_range_of(node);
        let expr = match node.kind() {
            K::Literal => Expr::Literal(literal_kind(node)),
            K::NameRef => return self.lower_name_ref(node),
            K::ParenExpr => Expr::Paren(self.lower_first_expr(node)),
            K::BinExpr => {
                let exprs = cst::child_exprs(node);
                let op = bin_op(node).unwrap_or(BinOp::Add);
                let lhs = self.lower_or_missing(exprs.first(), range);
                let rhs = self.lower_or_missing(exprs.get(1), range);
                Expr::Bin { op, lhs, rhs }
            }
            K::UnaryExpr => {
                let op = un_op(node).unwrap_or(UnOp::Pos);
                let operand = self.lower_first_expr(node);
                Expr::Unary { op, operand }
            }
            K::AwaitExpr => Expr::Await(self.lower_first_expr(node)),
            K::TernaryExpr => {
                let exprs = cst::child_exprs(node);
                let then_branch = self.lower_or_missing(exprs.first(), range);
                let cond = self.lower_or_missing(exprs.get(1), range);
                let else_branch = self.lower_or_missing(exprs.get(2), range);
                Expr::Ternary {
                    cond,
                    then_branch,
                    else_branch,
                }
            }
            K::CallExpr => {
                // `get_node("literal")` / `get_node_or_null("literal")` types like `$literal`.
                if let Some(path) = get_node_call_path(node) {
                    Expr::GetNode {
                        path: Some(path),
                        unique: false,
                    }
                } else {
                    let callee = self.lower_first_expr(node);
                    let args = cst::first_child(node, |k| k == K::ArgList)
                        .map(|al| self.lower_exprs(&al))
                        .unwrap_or_default();
                    Expr::Call { callee, args }
                }
            }
            K::IndexExpr => {
                let exprs = cst::child_exprs(node);
                let base = self.lower_or_missing(exprs.first(), range);
                let index = self.lower_or_missing(exprs.get(1), range);
                Expr::Index { base, index }
            }
            K::FieldExpr => {
                let receiver = self.lower_first_expr(node);
                let (name, name_range) = field_member(node).unwrap_or((SmolStr::default(), range));
                Expr::Field {
                    receiver,
                    name,
                    name_range,
                }
            }
            K::IsExpr => {
                let operand = self.lower_first_expr(node);
                Expr::Is {
                    operand,
                    ty: type_ref_ptr(node),
                    negated: cst::has_token(node, K::NotKw),
                }
            }
            K::CastExpr => {
                let operand = self.lower_first_expr(node);
                Expr::Cast {
                    operand,
                    ty: type_ref_ptr(node),
                }
            }
            K::InExpr => {
                let exprs = cst::child_exprs(node);
                let lhs = self.lower_or_missing(exprs.first(), range);
                let rhs = self.lower_or_missing(exprs.get(1), range);
                Expr::In {
                    lhs,
                    rhs,
                    negated: cst::has_token(node, K::NotKw),
                }
            }
            K::ArrayLit => Expr::Array(self.lower_exprs(node)),
            K::DictLit => {
                let entries = cst::children_of(node, K::DictEntry)
                    .iter()
                    .map(|e| {
                        let kv = cst::child_exprs(e);
                        let key = self.lower_or_missing(kv.first(), cst::text_range_of(e));
                        let value = kv.get(1).map(|v| self.lower_expr(v));
                        (key, value)
                    })
                    .collect();
                Expr::Dict(entries)
            }
            K::LambdaExpr => {
                let params = cst::first_child(node, |k| k == K::ParamList)
                    .map(|pl| self.lower_params(&pl))
                    .unwrap_or_default();
                let body = cst::first_child(node, |k| k == K::Block)
                    .map(|b| self.lower_block(&b))
                    .unwrap_or_default();
                Expr::Lambda { params, body }
            }
            K::PreloadExpr => {
                let arg_node = cst::first_child(node, |k| k == K::ArgList)
                    .and_then(|al| cst::first_child_expr(&al));
                // Constant-fold a string-literal path (`preload("res://x.gd")`) so inference can
                // resolve it. Trim matching quotes, as the `extends "…"` path lowering does.
                let path = arg_node
                    .as_ref()
                    .filter(|n| n.kind() == K::Literal)
                    .and_then(|n| cst::child_token_text(n, K::String))
                    .map(|s| SmolStr::new(s.trim_matches(['"', '\''])));
                let arg = arg_node.map(|e| self.lower_expr(&e));
                Expr::Preload { arg, path }
            }
            K::GetNodeExpr | K::UniqueNodeExpr => Expr::GetNode {
                path: node_path_text(node),
                unique: node.kind() == K::UniqueNodeExpr,
            },
            _ => Expr::Missing,
        };
        self.alloc_expr(expr, range)
    }

    fn lower_name_ref(&mut self, node: &GdNode) -> ExprId {
        let range = cst::text_range_of(node);
        let expr = match cst::first_token(node) {
            Some(t) if t.kind() == SyntaxKind::SelfKw => Expr::SelfExpr,
            Some(t) if t.kind() == SyntaxKind::SuperKw => Expr::Super,
            Some(t) => Expr::Name(SmolStr::new(t.text())),
            None => Expr::Missing,
        };
        self.alloc_expr(expr, range)
    }

    fn lower_or_missing(&mut self, node: Option<&GdNode>, fallback: TextRange) -> ExprId {
        match node {
            Some(n) => self.lower_expr(n),
            None => self.missing(fallback),
        }
    }

    fn lower_exprs(&mut self, node: &GdNode) -> Vec<ExprId> {
        cst::child_exprs(node)
            .iter()
            .map(|c| self.lower_expr(c))
            .collect()
    }

    fn lower_params(&mut self, param_list: &GdNode) -> Vec<ParamBinding> {
        cst::children_of(param_list, SyntaxKind::Param)
            .iter()
            .filter_map(|p| {
                let name_tok = ast::Param::cast(p.clone())?.name()?;
                let name_node = name_tok.syntax();
                Some(ParamBinding {
                    name: SmolStr::new(name_tok.text()?),
                    type_ref: type_ref_ptr(p),
                    default: cst::first_child_expr(p).map(|e| self.lower_expr(&e)),
                    name_range: cst::text_range_of(name_node),
                })
            })
            .collect()
    }

    fn lower_block(&mut self, block: &GdNode) -> Block {
        block
            .children()
            .filter_map(|c| self.lower_stmt(c))
            .collect()
    }

    fn lower_stmt(&mut self, node: &GdNode) -> Option<StmtId> {
        use SyntaxKind as K;
        let range = cst::text_range_of(node);
        let stmt = match node.kind() {
            K::ExprStmt => Stmt::Expr(self.lower_first_expr(node)),
            K::VarDecl | K::ConstDecl => Stmt::Var(self.lower_local_var(node)),
            K::ReturnStmt => Stmt::Return(cst::first_child_expr(node).map(|e| self.lower_expr(&e))),
            K::IfStmt => self.lower_if(node),
            K::WhileStmt => Stmt::While {
                cond: self.lower_first_expr(node),
                body: self.lower_child_block(node),
            },
            K::ForStmt => Stmt::For(self.lower_for(node)),
            K::MatchStmt => self.lower_match(node),
            K::BreakStmt => Stmt::Break,
            K::ContinueStmt => Stmt::Continue,
            K::PassStmt | K::BreakpointStmt => Stmt::Pass,
            K::AssertStmt => Stmt::Assert(
                cst::first_child(node, |k| k == K::ArgList)
                    .and_then(|al| cst::first_child_expr(&al))
                    .map(|e| self.lower_expr(&e)),
            ),
            // A nested local `func` is a declaration, not a statement we type in Phase 2.
            _ => return None,
        };
        Some(self.alloc_stmt(stmt, range))
    }

    fn lower_local_var(&mut self, node: &GdNode) -> LocalVar {
        let name_node = cst::first_child(node, |k| k == SyntaxKind::Name);
        let name = name_node
            .as_ref()
            .and_then(|n| ast::Name::cast(n.clone()))
            .and_then(|n| n.text())
            .map(SmolStr::new)
            .unwrap_or_default();
        LocalVar {
            name,
            type_ref: type_ref_ptr(node),
            init: cst::first_child_expr(node).map(|e| self.lower_expr(&e)),
            is_inferred: cst::has_token(node, SyntaxKind::ColonEq),
            is_const: node.kind() == SyntaxKind::ConstDecl,
            name_range: name_node
                .as_ref()
                .map_or_else(|| cst::text_range_of(node), cst::text_range_of),
        }
    }

    fn lower_if(&mut self, node: &GdNode) -> Stmt {
        let cond = self.lower_first_expr(node);
        let then_branch = self.lower_child_block(node);
        let elifs = cst::children_of(node, SyntaxKind::ElifClause)
            .iter()
            .map(|c| (self.lower_first_expr(c), self.lower_child_block(c)))
            .collect();
        let else_branch = cst::first_child(node, |k| k == SyntaxKind::ElseClause)
            .map(|c| self.lower_child_block(&c));
        Stmt::If {
            cond,
            then_branch,
            elifs,
            else_branch,
        }
    }

    fn lower_for(&mut self, node: &GdNode) -> ForLoop {
        let name = cst::first_child(node, |k| k == SyntaxKind::Name);
        let var = name
            .as_ref()
            .and_then(|n| ast::Name::cast(n.clone()))
            .and_then(|n| n.text())
            .map(SmolStr::new)
            .unwrap_or_default();
        ForLoop {
            var,
            var_type: type_ref_ptr(node),
            var_range: name
                .as_ref()
                .map_or_else(|| cst::text_range_of(node), cst::text_range_of),
            iter: self.lower_first_expr(node),
            body: self.lower_child_block(node),
        }
    }

    fn lower_match(&mut self, node: &GdNode) -> Stmt {
        let scrutinee = self.lower_first_expr(node);
        let arms = cst::children_of(node, SyntaxKind::MatchArm)
            .iter()
            .map(|arm| {
                let binds = cst::children_of(arm, SyntaxKind::PatternBind)
                    .iter()
                    .filter_map(|b| {
                        let name_node = cst::first_child(b, |k| k == SyntaxKind::Name)?;
                        let name = ast::Name::cast(name_node.clone())?
                            .text()
                            .map(SmolStr::new)?;
                        Some(MatchBind {
                            name,
                            range: cst::text_range_of(&name_node),
                        })
                    })
                    .collect();
                let guard = cst::first_child(arm, |k| k == SyntaxKind::PatternGuard)
                    .and_then(|g| cst::first_child_expr(&g))
                    .map(|e| self.lower_expr(&e));
                let body = self.lower_child_block(arm);
                MatchArm {
                    binds,
                    guard,
                    body,
                    range: cst::text_range_of(arm),
                    is_catch_all: arm_is_unconditional_catch_all(arm),
                }
            })
            .collect();
        Stmt::Match { scrutinee, arms }
    }

    /// The (first) `Block` child of `node`, lowered.
    fn lower_child_block(&mut self, node: &GdNode) -> Block {
        cst::first_child(node, |k| k == SyntaxKind::Block)
            .map(|b| self.lower_block(&b))
            .unwrap_or_default()
    }
}

/// The `AstPtr` of a node's (first) direct `TypeRef` child.
fn type_ref_ptr(node: &GdNode) -> Option<AstPtr> {
    cst::first_child(node, |k| k == SyntaxKind::TypeRef).map(|t| AstPtr::of(&t))
}

/// Classify a `Literal` node by its token.
fn literal_kind(node: &GdNode) -> Literal {
    use SyntaxKind as K;
    match cst::first_token(node).map(|t| t.kind()) {
        Some(K::Int) => Literal::Int,
        Some(K::Float) => Literal::Float,
        Some(K::String) => Literal::Str,
        Some(K::StringName) => Literal::StringName,
        Some(K::NodePath) => Literal::NodePath,
        Some(K::True) => Literal::Bool(true),
        Some(K::False) => Literal::Bool(false),
        Some(K::ConstPi | K::ConstTau | K::ConstInf | K::ConstNan) => Literal::MathConst,
        _ => Literal::Null,
    }
}

/// The binary operator token of a `BinExpr`.
fn bin_op(node: &GdNode) -> Option<BinOp> {
    node.children_with_tokens()
        .filter_map(cstree::util::NodeOrToken::into_token)
        .find_map(|t| BinOp::from_token(t.kind()))
}

/// The prefix operator token of a `UnaryExpr`.
fn un_op(node: &GdNode) -> Option<UnOp> {
    node.children_with_tokens()
        .filter_map(cstree::util::NodeOrToken::into_token)
        .find_map(|t| UnOp::from_token(t.kind()))
}

/// The member name + its range from a `FieldExpr` (the `NameRef` after the `.`).
fn field_member(node: &GdNode) -> Option<(SmolStr, TextRange)> {
    let nameref = cst::children_of(node, SyntaxKind::NameRef).pop()?;
    let tok = cst::first_token(&nameref)?;
    Some((SmolStr::new(tok.text()), cst::token_range(&tok)))
}

/// The literal path of a `get_node("…")` / `get_node_or_null("…")` call (a **bare** call = implicit
/// `self.get_node`), or `None` if it isn't such a call or the argument is computed (the latter stays
/// a normal call → `Node`). Lets the call lower to a [`Expr::GetNode`] so it types like `$path`.
fn get_node_call_path(node: &GdNode) -> Option<SmolStr> {
    let callee = cst::first_child_expr(node)?;
    // The callee must be `get_node`/`get_node_or_null`, either **bare** (implicit `self`) or
    // **`self.<m>`** (explicit self = the same attach node). A *foreign* receiver
    // (`obj.get_node(...)`) is left as a normal call — its path is relative to another node we can't
    // resolve here.
    let is_get_node = match callee.kind() {
        SyntaxKind::NameRef => {
            cst::first_token(&callee).is_some_and(|t| is_get_node_name(t.text()))
        }
        SyntaxKind::FieldExpr => {
            is_self_receiver(&callee)
                && field_member(&callee).is_some_and(|(name, _)| is_get_node_name(&name))
        }
        _ => false,
    };
    if !is_get_node {
        return None;
    }
    let arg = cst::first_child(node, |k| k == SyntaxKind::ArgList)
        .and_then(|al| cst::first_child_expr(&al))?;
    if arg.kind() != SyntaxKind::Literal {
        return None; // computed `get_node(var)` — stays a normal call (→ Node)
    }
    let s = cst::child_token_text(&arg, SyntaxKind::String)?;
    Some(SmolStr::new(s.trim_matches(['"', '\''])))
}

fn is_get_node_name(name: &str) -> bool {
    matches!(name, "get_node" | "get_node_or_null")
}

/// Whether a `FieldExpr`'s receiver is `self` (a `NameRef` carrying a `SelfKw` token).
fn is_self_receiver(field_expr: &GdNode) -> bool {
    cst::first_child_expr(field_expr).is_some_and(|recv| {
        recv.kind() == SyntaxKind::NameRef
            && recv
                .children_with_tokens()
                .filter_map(cstree::util::NodeOrToken::into_token)
                .any(|t| t.kind() == SyntaxKind::SelfKw)
    })
}

/// The literal node path from a `$Path`/`%Unique` (`GetNodeExpr`/`UniqueNodeExpr`) node: a dequoted
/// `$"a/b"` string, or the `/`-joined `Ident` segments of `$a/b`. `None` if it carries no path.
fn node_path_text(node: &GdNode) -> Option<SmolStr> {
    if let Some(s) = cst::child_token_text(node, SyntaxKind::String) {
        return Some(SmolStr::new(s.trim_matches(['"', '\''])));
    }
    let segs: Vec<String> = node
        .children_with_tokens()
        .filter_map(cstree::util::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::Ident)
        .map(|t| t.text().to_owned())
        .collect();
    (!segs.is_empty()).then(|| SmolStr::new(segs.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_syntax::parse;

    fn func_body(src: &str) -> Body {
        let root = parse(src).syntax_node();
        let func = gdscript_syntax::ast::descendants(&root)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::FuncDecl)
            .expect("a FuncDecl");
        body_of_func(&func)
    }

    #[test]
    fn lowers_params_and_return() {
        let body = func_body("func add(a: int, b := 1) -> int:\n\treturn a + b\n");
        assert_eq!(body.params.len(), 2);
        assert_eq!(body.params[0].name, "a");
        assert!(body.params[0].type_ref.is_some());
        assert!(body.params[1].default.is_some());
        assert_eq!(body.block.len(), 1);
        let Stmt::Return(Some(ret)) = body.stmt(body.block[0]) else {
            panic!("expected return")
        };
        assert!(matches!(body.expr(*ret), Expr::Bin { op: BinOp::Add, .. }));
    }

    #[test]
    fn lowers_local_var_and_field_and_call() {
        let body = func_body("func f():\n\tvar n := get_node(\"x\")\n\tn.show()\n");
        // local var
        let Stmt::Var(v) = body.stmt(body.block[0]) else {
            panic!("expected var")
        };
        assert_eq!(v.name, "n");
        assert!(v.is_inferred && v.init.is_some());
        // n.show() — a call on a field
        let Stmt::Expr(e) = body.stmt(body.block[1]) else {
            panic!("expected expr stmt")
        };
        let Expr::Call { callee, .. } = body.expr(*e) else {
            panic!("expected call")
        };
        assert!(matches!(body.expr(*callee), Expr::Field { name, .. } if name == "show"));
    }

    #[test]
    fn lowers_if_with_is_narrowing() {
        let body = func_body("func f(x):\n\tif x is Node:\n\t\tx.free()\n\telse:\n\t\tpass\n");
        let Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } = body.stmt(body.block[0])
        else {
            panic!("expected if")
        };
        assert!(matches!(body.expr(*cond), Expr::Is { negated: false, .. }));
        assert_eq!(then_branch.len(), 1);
        assert!(else_branch.is_some());
    }

    #[test]
    fn source_map_finds_tightest_expr() {
        // `a + b` — offset on `b` should resolve to the Name(b) expr, not the whole BinExpr.
        let body = func_body("func f(a, b):\n\treturn a + b\n");
        let b_offset = u32::try_from("func f(a, b):\n\treturn a + ".len()).unwrap();
        let id = body
            .source_map
            .expr_at_offset(b_offset)
            .expect("an expr at b");
        assert!(matches!(body.expr(id), Expr::Name(n) if n == "b"));
    }

    #[test]
    fn initializer_body_has_tail() {
        let root = parse("var x = 1 + 2\n").syntax_node();
        let var = gdscript_syntax::ast::descendants(&root)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::VarDecl)
            .unwrap();
        let init = crate::cst::first_child_expr(&var).unwrap();
        let body = body_of_expr(&init);
        assert!(body.tail.is_some());
        assert!(matches!(
            body.expr(body.tail.unwrap()),
            Expr::Bin { op: BinOp::Add, .. }
        ));
    }
}
