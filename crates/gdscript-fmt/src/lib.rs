//! `gdscript-fmt` — the GDScript source formatter (Phase-6 Workstream 3).
//!
//! A pure `fn(source, &FmtConfig) -> String`: no engine model, no filesystem, `wasm32`-safe.
//! It re-emits the lexer/pre-pass token stream, normalizing **block indentation** (to the
//! configured unit), **trailing whitespace**, and the **final newline** — every *significant*
//! token (keywords, identifiers, literals — including multi-line strings, which are single tokens)
//! is emitted **verbatim**, so meaning cannot change.
//!
//! **Safe by construction.** In `safe_mode` (the default) the formatter (a) refuses to touch a
//! file with syntax errors, and (b) re-lexes its own output and **falls back to the original** if
//! the significant token sequence changed. So it never corrupts code, even input it doesn't fully
//! understand. The result is idempotent: `format(format(x)) == format(x)`.
//!
//! Intra-line spacing normalization and line-reflow (full `gdformat` parity, the Wadler/Prettier
//! `Doc`-IR pretty-printer) are the documented next step — see `TECH_DEBT.md`. Today the formatter
//! owns indentation + whitespace, which is the most common formatting need and the safest subset.
#![cfg_attr(docsrs, feature(doc_cfg))]

use gdscript_syntax::SyntaxKind;

/// Formatter options. Defaults match the Godot convention (tabs) and keep the safety net on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmtConfig {
    /// Indent with tabs (the Godot convention). `false` indents with [`indent_size`](Self::indent_size) spaces.
    pub use_tabs: bool,
    /// Spaces per indent level when `use_tabs` is `false`.
    pub indent_size: usize,
    /// The target line width for reflow. **Reserved** — line-wrapping is not yet implemented.
    pub line_width: usize,
    /// Re-parse + significant-token-equality fallback to verbatim. Keep on unless you have a
    /// reason not to: it is the guarantee the formatter never changes meaning.
    pub safe_mode: bool,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            use_tabs: true,
            indent_size: 4,
            line_width: 100,
            safe_mode: true,
        }
    }
}

impl FmtConfig {
    /// One level of indentation as a string.
    #[must_use]
    fn indent_unit(&self) -> String {
        if self.use_tabs {
            "\t".to_owned()
        } else {
            " ".repeat(self.indent_size)
        }
    }
}

/// Format `source`, returning the tidied text. In `safe_mode` (the default) this returns `source`
/// unchanged rather than risk a meaning-changing edit (a syntax error in the input, or output whose
/// significant tokens differ from the input's).
#[must_use]
pub fn format(source: &str, config: &FmtConfig) -> String {
    // Safe mode: never reformat around a syntax error — we'd risk mis-indenting a mis-parsed block.
    if config.safe_mode && !gdscript_syntax::parse(source).errors().is_empty() {
        return source.to_owned();
    }
    let out = reindent(source, config);
    // The safety net: formatting must preserve the significant (non-trivia) token stream exactly.
    if config.safe_mode && !same_significant_tokens(source, &out) {
        return source.to_owned();
    }
    out
}

/// Re-emit the pre-pass token stream with normalized indentation + trailing whitespace + a single
/// final newline. Significant tokens (and continuation lines inside bracketed expressions) are
/// emitted verbatim; only a *logical* line's leading indentation is rewritten, to its block depth.
fn reindent(source: &str, config: &FmtConfig) -> String {
    let raw = gdscript_syntax::tokenize(source);
    let (toks, _diags) = gdscript_syntax::run_prepass(&raw, source);
    let unit = config.indent_unit();

    let mut out = String::with_capacity(source.len() + 16);
    let mut depth: usize = 0;
    // `true` while we are at the start of a logical line, before its first significant token — the
    // point at which we (re)emit the indentation, once `depth` is final.
    let mut line_start = true;
    // A synthetic `Newline` (zero-width) precedes the real `NewlinePhys` that carries the line's
    // bytes; this flag swallows that one `NewlinePhys` so the break is emitted exactly once. A
    // `NewlinePhys` *not* so flagged is a bracketed-continuation physical newline (kept verbatim).
    let mut just_broke = false;

    for t in &toks {
        let text = &source[t.range];
        match t.kind {
            SyntaxKind::Indent => depth += 1,
            SyntaxKind::Dedent => depth = depth.saturating_sub(1),
            // A synthetic line break: ends the logical line; the next one is re-indented.
            SyntaxKind::Newline => {
                trim_trailing_inline_ws(&mut out);
                out.push('\n');
                line_start = true;
                just_broke = true;
            }
            SyntaxKind::NewlinePhys => {
                if just_broke {
                    just_broke = false; // its bytes belong to the synthetic break already emitted
                } else {
                    // A physical newline inside a logical line (a bracketed continuation).
                    trim_trailing_inline_ws(&mut out);
                    out.push('\n');
                }
            }
            SyntaxKind::Whitespace => {
                if line_start {
                    // A logical line's leading indentation — dropped; the normalized indentation is
                    // emitted at the first significant token (so `depth` is final by then).
                } else {
                    out.push_str(text);
                }
            }
            // A significant token or a comment.
            _ => {
                if line_start {
                    for _ in 0..depth {
                        out.push_str(&unit);
                    }
                    line_start = false;
                }
                just_broke = false;
                out.push_str(text);
            }
        }
    }
    // Trim a trailing blank/whitespace run and guarantee exactly one final newline.
    let trimmed = out.trim_end();
    let mut result = String::with_capacity(trimmed.len() + 1);
    result.push_str(trimmed);
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

/// Trim trailing spaces/tabs from the end of `out` (the current line).
fn trim_trailing_inline_ws(out: &mut String) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
}

/// Whether two sources lex to the same sequence of significant (non-trivia) tokens — the
/// meaning-preservation check. Whitespace / newline / comment trivia are ignored (that is what the
/// formatter is allowed to change); literals (including multi-line strings) are significant, so a
/// corrupted string would be caught here.
fn same_significant_tokens(a: &str, b: &str) -> bool {
    fn sig(s: &str) -> Vec<(SyntaxKind, &str)> {
        gdscript_syntax::tokenize(s)
            .into_iter()
            .filter(|t| !t.kind.is_trivia())
            .map(|t| (t.kind, &s[t.range]))
            .collect()
    }
    sig(a) == sig(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format(src, &FmtConfig::default())
    }

    #[test]
    fn normalizes_indentation_to_tabs() {
        // Four-space indentation becomes one tab per level.
        let src = "func f():\n    if true:\n        return 1\n";
        assert_eq!(fmt(src), "func f():\n\tif true:\n\t\treturn 1\n");
    }

    #[test]
    fn trims_trailing_whitespace_and_adds_final_newline() {
        let src = "var x = 1   \nvar y = 2"; // trailing spaces + no final newline
        assert_eq!(fmt(src), "var x = 1\nvar y = 2\n");
    }

    #[test]
    fn is_idempotent() {
        let src = "func f():\n  var a = 1\n  if a:\n      return a\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once, "formatting must be idempotent");
    }

    #[test]
    fn already_formatted_is_unchanged() {
        let src = "func f():\n\tvar a = 1\n\treturn a\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn preserves_significant_tokens_including_strings() {
        let src = "func f():\n\tvar s = \"a + b\"\n\treturn s\n";
        let out = fmt(src);
        assert!(super::same_significant_tokens(src, &out));
        assert!(out.contains("\"a + b\""));
    }

    #[test]
    fn multiline_string_content_is_untouched() {
        // The interior of a multi-line string must survive verbatim (it is a single token).
        let src = "func f():\n\tvar s = \"\"\"line1\n        keep   \nline2\"\"\"\n\treturn s\n";
        let out = fmt(src);
        assert!(
            out.contains("line1\n        keep   \nline2"),
            "got: {out:?}"
        );
    }

    #[test]
    fn safe_mode_returns_input_on_syntax_error() {
        let src = "func f(:\n\treturn"; // malformed
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(fmt(""), "");
        assert_eq!(fmt("\n\n\n"), "");
    }

    #[test]
    fn spaces_option_indents_with_spaces() {
        let cfg = FmtConfig {
            use_tabs: false,
            indent_size: 2,
            ..FmtConfig::default()
        };
        let src = "func f():\n\treturn 1\n";
        assert_eq!(format(src, &cfg), "func f():\n  return 1\n");
    }
}
