//! WS1 — the lexer.
//!
//! A `logos` DFA lexer turns source bytes into a flat stream of [`RawToken`]s. It
//! is **lossless**: every byte of the input lands in exactly one token, including
//! whitespace, comments, and line continuations (trivia are first-class tokens, never
//! skipped — see `plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §WS1). The lexer is
//! indentation-unaware; the pre-pass (WS2) turns physical newlines into the synthetic
//! `Newline`/`Indent`/`Dedent` markers the parser consumes.
//!
//! Invariant (tested): `concat(src[t.range] for t in tokenize(src)) == src`.

use logos::{Lexer, Logos};
use text_size::{TextRange, TextSize};

use crate::SyntaxKind;

/// A lexed token: its [`SyntaxKind`] and the byte range it covers in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawToken {
    /// The token kind (keywords already reclassified from identifiers).
    pub kind: SyntaxKind,
    /// The byte range in the original source (`text-size`, `u32`-based).
    pub range: TextRange,
}

/// The lexer's own token alphabet. A subset of [`SyntaxKind`]: keywords are lexed as
/// [`LexKind::Ident`] and reclassified by text (avoids same-priority keyword/ident
/// ties), and the several string flavours collapse to one of three kinds.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
enum LexKind {
    // ---- trivia ----
    // A UTF-8 BOM (`U+FEFF`). High priority so it wins over any other rule; lexed
    // wherever it appears (a leading BOM is the real case — Godot strips it), kept as
    // trivia for losslessness.
    #[token("\u{feff}", priority = 10)]
    Bom,
    #[regex(r"[ \t]+")]
    Whitespace,
    #[regex(r"\r\n|\n|\r")]
    NewlinePhys,
    #[regex(r"\\(\r\n|\n|\r)")]
    LineContinuation,
    // `allow_greedy`: a comment legitimately consumes to end-of-line; the greedy
    // `[^\r\n]*` scan is the intended (and O(line)) behavior. logos 0.16 requires the
    // opt-in for any dot-equivalent repetition.
    #[regex(r"#region[^\r\n]*", priority = 5, allow_greedy = true)]
    RegionComment,
    #[regex(r"#endregion[^\r\n]*", priority = 5, allow_greedy = true)]
    EndRegionComment,
    #[regex(r"##[^\r\n]*", priority = 4, allow_greedy = true)]
    DocComment,
    #[regex(r"#[^\r\n]*", priority = 2, allow_greedy = true)]
    LineComment,

    // ---- literals & names ----
    #[regex(r"0[xX][0-9a-fA-F_]+|0[bB][01_]+|[0-9][0-9_]*")]
    Int,
    #[regex(r"[0-9][0-9_]*\.[0-9_]*([eE][+-]?[0-9_]+)?|\.[0-9][0-9_]*([eE][+-]?[0-9_]+)?|[0-9][0-9_]*[eE][+-]?[0-9_]+")]
    Float,
    // String flavours: single/triple, raw (`r`), all via one scanning callback that
    // determines the closer from the matched opener slice. Unterminated → consume to
    // end-of-line (single) or EOF (triple), still emitting a String (lossless).
    #[token("\"", lex_string)]
    #[token("'", lex_string)]
    #[token("\"\"\"", lex_string)]
    #[token("'''", lex_string)]
    #[token("r\"", lex_string)]
    #[token("r'", lex_string)]
    #[token("r\"\"\"", lex_string)]
    #[token("r'''", lex_string)]
    String,
    #[token("&\"", lex_string)]
    #[token("&'", lex_string)]
    StringName,
    #[token("^\"", lex_string)]
    #[token("^'", lex_string)]
    NodePath,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    // ---- brackets & punctuation ----
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBrack,
    #[token("]")]
    RBrack,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(";")]
    Semicolon,
    #[token(".")]
    Dot,
    #[token("..")]
    DotDot,
    #[token("...")]
    Ellipsis,
    #[token("@")]
    At,
    #[token("$")]
    Dollar,
    #[token("%")]
    Percent,
    #[token("&")]
    Amp,
    #[token("->")]
    Arrow,
    #[token(":=")]
    ColonEq,

    // ---- operators ----
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("**")]
    StarStar,
    #[token("=")]
    Eq,
    #[token("==")]
    EqEq,
    #[token("!=")]
    Neq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("<=")]
    Le,
    #[token(">=")]
    Ge,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("!")]
    Bang,
    #[token("~")]
    Tilde,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("<<")]
    Shl,
    #[token(">>")]
    Shr,
    #[token("+=")]
    PlusEq,
    #[token("-=")]
    MinusEq,
    #[token("*=")]
    StarEq,
    #[token("/=")]
    SlashEq,
    #[token("**=")]
    StarStarEq,
    #[token("%=")]
    PercentEq,
    #[token("&=")]
    AmpEq,
    #[token("|=")]
    PipeEq,
    #[token("^=")]
    CaretEq,
    #[token("<<=")]
    ShlEq,
    #[token(">>=")]
    ShrEq,
}

/// Scan a string body after the opening delimiter has been matched. The opener slice
/// (`"`, `'''`, `r"`, `&'`, …) tells us the quote byte and whether it is a triple
/// (multiline) string. Backslash escapes the next byte for *termination* purposes in
/// every flavour (matching Godot/Python: `\"` never closes the string, even raw).
fn lex_string(lex: &mut Lexer<LexKind>) {
    let opener = lex.slice().as_bytes();
    let quote = opener[opener.len() - 1];
    let triple =
        opener.len() >= 3 && opener[opener.len() - 2] == quote && opener[opener.len() - 3] == quote;

    let rem = lex.remainder().as_bytes();
    let n = rem.len();
    let mut i = 0usize;
    while i < n {
        let c = rem[i];
        if c == b'\\' {
            i += 2; // skip the escaped byte (may step past `n`; clamped below)
            continue;
        }
        if triple {
            if c == quote && i + 2 < n && rem[i + 1] == quote && rem[i + 2] == quote {
                i += 3; // consume the closing triple-quote
                break;
            }
        } else {
            if c == quote {
                i += 1; // consume the closing quote
                break;
            }
            if c == b'\n' || c == b'\r' {
                break; // unterminated single-line string — stop before the newline
            }
        }
        i += 1;
    }
    lex.bump(i.min(n));
}

/// Lex `src` into a lossless [`RawToken`] stream. Never fails: an unlexable byte
/// becomes a [`SyntaxKind::Error`] token, so the concatenation of token ranges always
/// reproduces the source.
#[must_use]
pub fn tokenize(src: &str) -> Vec<RawToken> {
    let mut out = Vec::new();
    let mut lexer = LexKind::lexer(src);
    while let Some(result) = lexer.next() {
        let span = lexer.span();
        let kind = match result {
            Ok(lex_kind) => map_kind(lex_kind, &src[span.clone()]),
            Err(()) => SyntaxKind::Error,
        };
        out.push(RawToken {
            kind,
            range: TextRange::new(text_size(span.start), text_size(span.end)),
        });
    }
    out
}

/// Convert a byte offset into a `TextSize`, asserting the source fits in `u32`.
fn text_size(offset: usize) -> TextSize {
    TextSize::new(u32::try_from(offset).expect("source files must be smaller than 4 GiB"))
}

/// Map a lexer token kind (plus its text, for identifier reclassification) to the
/// shared [`SyntaxKind`].
fn map_kind(kind: LexKind, text: &str) -> SyntaxKind {
    use LexKind as L;
    use SyntaxKind as S;
    match kind {
        L::Bom => S::Bom,
        L::Whitespace => S::Whitespace,
        L::NewlinePhys => S::NewlinePhys,
        L::LineContinuation => S::LineContinuation,
        L::RegionComment => S::RegionComment,
        L::EndRegionComment => S::EndRegionComment,
        L::DocComment => S::DocComment,
        L::LineComment => S::LineComment,
        L::Int => S::Int,
        L::Float => S::Float,
        L::String => S::String,
        L::StringName => S::StringName,
        L::NodePath => S::NodePath,
        L::Ident => reclassify_ident(text),
        L::LParen => S::LParen,
        L::RParen => S::RParen,
        L::LBrack => S::LBrack,
        L::RBrack => S::RBrack,
        L::LBrace => S::LBrace,
        L::RBrace => S::RBrace,
        L::Comma => S::Comma,
        L::Colon => S::Colon,
        L::Semicolon => S::Semicolon,
        L::Dot => S::Dot,
        L::DotDot => S::DotDot,
        L::Ellipsis => S::Ellipsis,
        L::At => S::At,
        L::Dollar => S::Dollar,
        L::Percent => S::Percent,
        L::Amp => S::Amp,
        L::Arrow => S::Arrow,
        L::ColonEq => S::ColonEq,
        L::Plus => S::Plus,
        L::Minus => S::Minus,
        L::Star => S::Star,
        L::Slash => S::Slash,
        L::StarStar => S::StarStar,
        L::Eq => S::Eq,
        L::EqEq => S::EqEq,
        L::Neq => S::Neq,
        L::Lt => S::Lt,
        L::Gt => S::Gt,
        L::Le => S::Le,
        L::Ge => S::Ge,
        L::AmpAmp => S::AmpAmp,
        L::PipePipe => S::PipePipe,
        L::Bang => S::Bang,
        L::Tilde => S::Tilde,
        L::Pipe => S::Pipe,
        L::Caret => S::Caret,
        L::Shl => S::Shl,
        L::Shr => S::Shr,
        L::PlusEq => S::PlusEq,
        L::MinusEq => S::MinusEq,
        L::StarEq => S::StarEq,
        L::SlashEq => S::SlashEq,
        L::StarStarEq => S::StarStarEq,
        L::PercentEq => S::PercentEq,
        L::AmpEq => S::AmpEq,
        L::PipeEq => S::PipeEq,
        L::CaretEq => S::CaretEq,
        L::ShlEq => S::ShlEq,
        L::ShrEq => S::ShrEq,
    }
}

/// Reclassify an identifier's text to a keyword / literal-keyword / built-in constant
/// kind, or [`SyntaxKind::Ident`] if it is an ordinary name. `true`/`false`/`null` are
/// literals (not keywords) per Godot's tokenizer; `PI`/`TAU`/`INF`/`NAN` are the
/// engine's built-in constant tokens.
fn reclassify_ident(text: &str) -> SyntaxKind {
    use SyntaxKind as S;
    match text {
        "if" => S::IfKw,
        "elif" => S::ElifKw,
        "else" => S::ElseKw,
        "for" => S::ForKw,
        "while" => S::WhileKw,
        "match" => S::MatchKw,
        "when" => S::WhenKw,
        "break" => S::BreakKw,
        "continue" => S::ContinueKw,
        "pass" => S::PassKw,
        "return" => S::ReturnKw,
        "var" => S::VarKw,
        "const" => S::ConstKw,
        "enum" => S::EnumKw,
        "func" => S::FuncKw,
        "static" => S::StaticKw,
        "signal" => S::SignalKw,
        "class" => S::ClassKw,
        "class_name" => S::ClassNameKw,
        "extends" => S::ExtendsKw,
        "is" => S::IsKw,
        "in" => S::InKw,
        "as" => S::AsKw,
        "self" => S::SelfKw,
        "super" => S::SuperKw,
        "void" => S::VoidKw,
        "await" => S::AwaitKw,
        "preload" => S::PreloadKw,
        "assert" => S::AssertKw,
        "breakpoint" => S::BreakpointKw,
        "not" => S::NotKw,
        "and" => S::AndKw,
        "or" => S::OrKw,
        "yield" => S::YieldKw,
        "namespace" => S::NamespaceKw,
        "trait" => S::TraitKw,
        "true" => S::True,
        "false" => S::False,
        "null" => S::Null,
        "PI" => S::ConstPi,
        "TAU" => S::ConstTau,
        "INF" => S::ConstInf,
        "NAN" => S::ConstNan,
        _ => S::Ident,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lossless invariant: every byte is covered exactly once, in order.
    fn assert_lossless(src: &str) {
        let toks = tokenize(src);
        // Ranges are contiguous, start at 0, end at len.
        let mut prev_end = TextSize::new(0);
        let mut rebuilt = String::new();
        for t in &toks {
            assert_eq!(
                t.range.start(),
                prev_end,
                "gap/overlap before {t:?} in {src:?}"
            );
            prev_end = t.range.end();
            rebuilt.push_str(&src[t.range]);
        }
        assert_eq!(prev_end, TextSize::of(src), "did not cover to EOF: {src:?}");
        assert_eq!(rebuilt, src, "round-trip mismatch for {src:?}");
    }

    fn kinds(src: &str) -> Vec<SyntaxKind> {
        tokenize(src).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lossless_over_a_realistic_snippet() {
        let src = "## doc\n@export var hp: int = 100 # hi\nfunc _ready() -> void:\n\tprint($Player, %Unique)\n";
        assert_lossless(src);
    }

    #[test]
    fn keywords_and_literals_reclassified() {
        use SyntaxKind as S;
        assert_eq!(kinds("func"), vec![S::FuncKw]);
        assert_eq!(
            kinds("true false null"),
            vec![S::True, S::Whitespace, S::False, S::Whitespace, S::Null]
        );
        assert_eq!(kinds("PI"), vec![S::ConstPi]);
        assert_eq!(kinds("my_var"), vec![S::Ident]);
        assert_eq!(kinds("class_name"), vec![S::ClassNameKw]);
    }

    #[test]
    fn numbers() {
        use SyntaxKind as S;
        assert_eq!(kinds("0x8f51"), vec![S::Int]);
        assert_eq!(kinds("0b1010"), vec![S::Int]);
        assert_eq!(kinds("12_345"), vec![S::Int]);
        assert_eq!(kinds("3.14"), vec![S::Float]);
        assert_eq!(kinds(".5"), vec![S::Float]);
        assert_eq!(kinds("1."), vec![S::Float]);
        assert_eq!(kinds("58.1e-10"), vec![S::Float]);
    }

    #[test]
    fn strings_all_flavours() {
        use SyntaxKind as S;
        assert_eq!(kinds(r#""hello""#), vec![S::String]);
        assert_eq!(kinds("'world'"), vec![S::String]);
        assert_eq!(kinds(r#""with \" escape""#), vec![S::String]);
        assert_eq!(kinds(r#"r"raw\n""#), vec![S::String]);
        assert_eq!(kinds("\"\"\"multi\nline\"\"\""), vec![S::String]);
        assert_eq!(kinds(r#"&"sname""#), vec![S::StringName]);
        assert_eq!(kinds(r#"^"node/path""#), vec![S::NodePath]);
        // $"x" is two tokens: Dollar then String.
        assert_eq!(kinds(r#"$"Player""#), vec![S::Dollar, S::String]);
    }

    #[test]
    fn unterminated_string_is_lossless() {
        // Single-line unterminated: stops before the newline, still a String.
        let src = "\"oops\nok";
        assert_lossless(src);
        assert_eq!(kinds(src)[0], SyntaxKind::String);
        // Triple unterminated: consumes to EOF.
        assert_lossless("\"\"\"never closed");
    }

    #[test]
    fn operators_longest_match() {
        use SyntaxKind as S;
        assert_eq!(kinds("**="), vec![S::StarStarEq]);
        assert_eq!(kinds(">>="), vec![S::ShrEq]);
        assert_eq!(kinds(":="), vec![S::ColonEq]);
        assert_eq!(kinds("->"), vec![S::Arrow]);
        assert_eq!(kinds("..."), vec![S::Ellipsis]);
        assert_eq!(kinds("&&"), vec![S::AmpAmp]);
    }

    #[test]
    fn unlexable_byte_becomes_error_token() {
        // A stray backtick matches no rule → Error, but still lossless.
        let src = "a ` b";
        assert_lossless(src);
        assert!(kinds(src).contains(&SyntaxKind::Error));
    }

    #[test]
    fn comments_distinguished() {
        use SyntaxKind as S;
        assert_eq!(kinds("# plain"), vec![S::LineComment]);
        assert_eq!(kinds("## doc"), vec![S::DocComment]);
        assert_eq!(kinds("#region A"), vec![S::RegionComment]);
        assert_eq!(kinds("#endregion"), vec![S::EndRegionComment]);
    }
}
