//! `SyntaxKind` — the single `#[repr(u32)]` kind enum shared by the lexer, the
//! indentation pre-pass, the parser, and the typed AST.
//!
//! This is the one source of truth for every terminal (token) and non-terminal
//! (node) in the GDScript grammar. `cstree` keys its green nodes by this enum via
//! the derived [`cstree::Syntax`] impl. Fixed-lexeme kinds (keywords, operators,
//! punctuation) carry `#[static_text(...)]` so `cstree` stores them without
//! interning a string (and validates that the byte text matches).
//!
//! Design notes (see `plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §4.1):
//! - **`#[repr(u32)]`** — `cstree::RawSyntaxKind` is a `u32` newtype (not rowan's
//!   `u16`). The discriminants are contiguous from 0, so the derived `from_raw`
//!   round-trips every variant.
//! - **`true`/`false`/`null` are literals, not keywords** — Godot tokenizes them as
//!   literal tokens, and they parse as primary expressions. They still have fixed
//!   text, so they get `#[static_text]`.
//! - **`Newline`/`Indent`/`Dedent`** are *synthetic, zero-width* structural tokens
//!   injected by the pre-pass. The parser reads them to recover block structure;
//!   the sink emits them as empty-text tokens, so they never affect the byte-exact
//!   round-trip (the real newline/space bytes live in the retained `NewlinePhys` /
//!   `Whitespace` trivia tokens).

use cstree::Syntax;

/// Every terminal and non-terminal kind in the GDScript syntax tree.
///
/// Variants are grouped: trivia, synthetic block-structure, literals/names,
/// keywords, built-in constant names, punctuation/operators, error tokens, then
/// grammar-production nodes. `Tombstone` is kept last as the count sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Syntax)]
#[repr(u32)]
pub enum SyntaxKind {
    // ---- tokens: trivia (carry source bytes; the parser skips them) ----
    /// A run of spaces and/or tabs.
    Whitespace,
    /// `# ...` to end of line.
    LineComment,
    /// `## ...` documentation comment (feeds hover later).
    DocComment,
    /// `#region ...` fold-region opener (a comment, flagged for folding).
    RegionComment,
    /// `#endregion ...` fold-region closer.
    EndRegionComment,
    /// A `\` immediately before a newline — joins the logical line.
    LineContinuation,
    /// A physical line break (`\n`, `\r\n`, or `\r`). The pre-pass keeps it as
    /// trivia for losslessness and emits a synthetic [`SyntaxKind::Newline`] where
    /// a statement actually terminates.
    NewlinePhys,
    /// A UTF-8 byte-order mark (`U+FEFF`). Some editors prepend one to a saved `.gd`
    /// file; Godot strips a leading BOM, so we keep it as its own trivia token (not
    /// `Whitespace` — that would mis-count the first line's indentation by 3 bytes, and
    /// not an `Error` — the file is valid GDScript).
    Bom,

    // ---- tokens: synthetic block structure (zero-width; injected by the pre-pass) ----
    /// Logical statement terminator (outside brackets, not after a continuation).
    Newline,
    /// Block open — indentation increased.
    Indent,
    /// Block close — indentation decreased (possibly several in a row).
    Dedent,

    // ---- tokens: literals & names (interned text) ----
    /// Integer literal: `45`, `0x8f51`, `0b101010`, `12_345`.
    Int,
    /// Float literal: `3.14`, `.5`, `1.`, `58.1e-10`.
    Float,
    /// String literal: `"..."`, `'...'`, `"""..."""`, `'''...'''`, raw `r"..."`.
    String,
    /// StringName literal: `&"..."` / `&'...'`.
    StringName,
    /// NodePath literal: `^"..."` / `^'...'`.
    NodePath,
    /// An identifier: `[A-Za-z_][A-Za-z0-9_]*`.
    Ident,

    // ---- tokens: literal keywords (fixed text) ----
    #[static_text("true")]
    True,
    #[static_text("false")]
    False,
    #[static_text("null")]
    Null,

    // ---- tokens: built-in math constants (fixed text; engine CONST_* tokens) ----
    #[static_text("PI")]
    ConstPi,
    #[static_text("TAU")]
    ConstTau,
    #[static_text("INF")]
    ConstInf,
    #[static_text("NAN")]
    ConstNan,

    // ---- tokens: keywords (~35; fixed text) ----
    #[static_text("if")]
    IfKw,
    #[static_text("elif")]
    ElifKw,
    #[static_text("else")]
    ElseKw,
    #[static_text("for")]
    ForKw,
    #[static_text("while")]
    WhileKw,
    #[static_text("match")]
    MatchKw,
    #[static_text("when")]
    WhenKw,
    #[static_text("break")]
    BreakKw,
    #[static_text("continue")]
    ContinueKw,
    #[static_text("pass")]
    PassKw,
    #[static_text("return")]
    ReturnKw,
    #[static_text("var")]
    VarKw,
    #[static_text("const")]
    ConstKw,
    #[static_text("enum")]
    EnumKw,
    #[static_text("func")]
    FuncKw,
    #[static_text("static")]
    StaticKw,
    #[static_text("signal")]
    SignalKw,
    #[static_text("class")]
    ClassKw,
    #[static_text("class_name")]
    ClassNameKw,
    #[static_text("extends")]
    ExtendsKw,
    #[static_text("is")]
    IsKw,
    #[static_text("in")]
    InKw,
    #[static_text("as")]
    AsKw,
    #[static_text("self")]
    SelfKw,
    #[static_text("super")]
    SuperKw,
    #[static_text("void")]
    VoidKw,
    #[static_text("await")]
    AwaitKw,
    #[static_text("preload")]
    PreloadKw,
    #[static_text("assert")]
    AssertKw,
    #[static_text("breakpoint")]
    BreakpointKw,
    #[static_text("not")]
    NotKw,
    #[static_text("and")]
    AndKw,
    #[static_text("or")]
    OrKw,
    /// Deprecated since GDScript 2.0 — still lexed so we can diagnose it.
    #[static_text("yield")]
    YieldKw,
    /// Reserved but unused — lexed to reject as an identifier.
    #[static_text("namespace")]
    NamespaceKw,
    /// Reserved but unused.
    #[static_text("trait")]
    TraitKw,

    // ---- tokens: punctuation / brackets (fixed text) ----
    #[static_text("(")]
    LParen,
    #[static_text(")")]
    RParen,
    #[static_text("[")]
    LBrack,
    #[static_text("]")]
    RBrack,
    #[static_text("{")]
    LBrace,
    #[static_text("}")]
    RBrace,
    #[static_text(",")]
    Comma,
    #[static_text(":")]
    Colon,
    #[static_text(";")]
    Semicolon,
    #[static_text(".")]
    Dot,
    #[static_text("..")]
    DotDot,
    #[static_text("...")]
    Ellipsis,
    #[static_text("@")]
    At,
    #[static_text("$")]
    Dollar,
    #[static_text("%")]
    Percent,
    #[static_text("&")]
    Amp,
    #[static_text("->")]
    Arrow,
    #[static_text(":=")]
    ColonEq,

    // ---- tokens: operators (fixed text) ----
    #[static_text("+")]
    Plus,
    #[static_text("-")]
    Minus,
    #[static_text("*")]
    Star,
    #[static_text("/")]
    Slash,
    #[static_text("**")]
    StarStar,
    #[static_text("=")]
    Eq,
    #[static_text("==")]
    EqEq,
    #[static_text("!=")]
    Neq,
    #[static_text("<")]
    Lt,
    #[static_text(">")]
    Gt,
    #[static_text("<=")]
    Le,
    #[static_text(">=")]
    Ge,
    #[static_text("&&")]
    AmpAmp,
    #[static_text("||")]
    PipePipe,
    #[static_text("!")]
    Bang,
    #[static_text("~")]
    Tilde,
    #[static_text("|")]
    Pipe,
    #[static_text("^")]
    Caret,
    #[static_text("<<")]
    Shl,
    #[static_text(">>")]
    Shr,
    #[static_text("+=")]
    PlusEq,
    #[static_text("-=")]
    MinusEq,
    #[static_text("*=")]
    StarEq,
    #[static_text("/=")]
    SlashEq,
    #[static_text("**=")]
    StarStarEq,
    #[static_text("%=")]
    PercentEq,
    #[static_text("&=")]
    AmpEq,
    #[static_text("|=")]
    PipeEq,
    #[static_text("^=")]
    CaretEq,
    #[static_text("<<=")]
    ShlEq,
    #[static_text(">>=")]
    ShrEq,

    // ---- tokens: error / sentinel ----
    /// An unlexable byte — carried into the tree (never dropped) for losslessness.
    Error,
    /// Virtual end-of-input. Used by the parser; never emitted into the tree.
    Eof,

    // ---- nodes: file & top-level ----
    SourceFile,
    ExtendsClause,
    ClassNameDecl,
    Annotation,
    AnnotationArgList,

    // ---- nodes: declarations ----
    InnerClassDecl,
    ClassBody,
    FuncDecl,
    ParamList,
    Param,
    VarargParam,
    VarDecl,
    ConstDecl,
    EnumDecl,
    EnumVariant,
    SignalDecl,
    PropertyBody,
    Getter,
    Setter,
    Name,

    // ---- nodes: types ----
    TypeRef,
    TypedArray,
    TypedDict,

    // ---- nodes: statements ----
    Block,
    IfStmt,
    ElifClause,
    ElseClause,
    ForStmt,
    WhileStmt,
    MatchStmt,
    MatchArm,
    ReturnStmt,
    BreakStmt,
    ContinueStmt,
    PassStmt,
    AssertStmt,
    BreakpointStmt,
    ExprStmt,
    VarStmt,

    // ---- nodes: match patterns ----
    PatternLiteral,
    PatternBind,
    PatternWildcard,
    PatternArray,
    PatternDict,
    PatternRest,
    PatternGuard,

    // ---- nodes: expressions ----
    BinExpr,
    UnaryExpr,
    TernaryExpr,
    CastExpr,
    IsExpr,
    InExpr,
    CallExpr,
    ArgList,
    IndexExpr,
    FieldExpr,
    AwaitExpr,
    LambdaExpr,
    ParenExpr,
    ArrayLit,
    DictLit,
    DictEntry,
    NameRef,
    Literal,
    GetNodeExpr,
    UniqueNodeExpr,
    PreloadExpr,

    // ---- nodes: error recovery ----
    ErrorNode,

    /// Count sentinel — keep last (drives the `u32` ↔ kind range).
    Tombstone,
}

impl SyntaxKind {
    /// Trivia carry source bytes but are skipped by the parser (re-attached by the
    /// tree sink). The synthetic `Newline`/`Indent`/`Dedent` markers are **not**
    /// trivia — the parser consumes them to recover block structure.
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace
                | Self::LineComment
                | Self::DocComment
                | Self::RegionComment
                | Self::EndRegionComment
                | Self::LineContinuation
                | Self::NewlinePhys
                | Self::Bom
        )
    }

    /// The synthetic, zero-width block-structure markers injected by the pre-pass.
    #[must_use]
    pub const fn is_synthetic_layout(self) -> bool {
        matches!(self, Self::Newline | Self::Indent | Self::Dedent)
    }

    /// Whether this kind is a node (grammar production) rather than a token.
    #[must_use]
    pub const fn is_node(self) -> bool {
        // Nodes are exactly the kinds at/after `SourceFile`.
        (self as u32) >= (Self::SourceFile as u32)
    }
}

/// A resolved (interner-carrying) red node — supports `Display`/`.text()` and the
/// byte-exact round-trip. This is the public tree type for callers that need text.
pub type GdNode = cstree::syntax::ResolvedNode<SyntaxKind>;
/// A resolved red token.
pub type GdToken = cstree::syntax::ResolvedToken<SyntaxKind>;
/// A bare (resolver-less) red node — cheap, `Send + Sync`; text needs a resolver.
pub type SyntaxNode = cstree::syntax::SyntaxNode<SyntaxKind>;

#[cfg(test)]
mod tests {
    use super::*;
    use cstree::build::GreenNodeBuilder;
    use cstree::syntax::ResolvedNode;

    /// The Step-1 gate: build a tiny `func foo` tree by hand and prove it
    /// round-trips byte-for-byte — exercising `static_token` (keyword), an interned
    /// trivia token (whitespace), an interned `Ident`, and a zero-width synthetic
    /// `Newline` marker that must contribute no bytes.
    #[test]
    fn three_node_tree_round_trips() {
        let mut builder: GreenNodeBuilder<'_, '_, SyntaxKind> = GreenNodeBuilder::new();
        builder.start_node(SyntaxKind::SourceFile);
        builder.start_node(SyntaxKind::FuncDecl);
        builder.static_token(SyntaxKind::FuncKw); // "func"
        builder.token(SyntaxKind::Whitespace, " ");
        builder.start_node(SyntaxKind::Name);
        builder.token(SyntaxKind::Ident, "foo");
        builder.finish_node(); // Name
        builder.token(SyntaxKind::Newline, ""); // zero-width synthetic marker
        builder.finish_node(); // FuncDecl
        builder.finish_node(); // SourceFile

        let (green, cache) = builder.finish();
        let interner = cache.unwrap().into_interner().unwrap();
        let root = ResolvedNode::<SyntaxKind>::new_root_with_resolver(green, interner);

        // Byte-for-byte round-trip — the defining lossless invariant.
        assert_eq!(root.to_string(), "func foo");
        assert_eq!(root.kind(), SyntaxKind::SourceFile);
    }

    #[test]
    fn raw_kind_round_trips() {
        // The derived `from_raw`/`into_raw` must be inverses across every variant.
        for raw in 0..(SyntaxKind::Tombstone as u32) {
            let kind = <SyntaxKind as Syntax>::from_raw(cstree::RawSyntaxKind(raw));
            assert_eq!(<SyntaxKind as Syntax>::into_raw(kind).0, raw);
        }
    }

    #[test]
    fn classification_helpers() {
        assert!(SyntaxKind::Whitespace.is_trivia());
        assert!(!SyntaxKind::Newline.is_trivia());
        assert!(SyntaxKind::Indent.is_synthetic_layout());
        assert!(SyntaxKind::FuncDecl.is_node());
        assert!(!SyntaxKind::FuncKw.is_node());
    }
}
