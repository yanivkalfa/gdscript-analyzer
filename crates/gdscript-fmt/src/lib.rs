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
    let input_parses = gdscript_syntax::parse(source).errors().is_empty();
    // Safe mode: never reformat around a syntax error — we'd risk mis-indenting a mis-parsed block.
    if config.safe_mode && !input_parses {
        return source.to_owned();
    }
    let out = reindent(source, config);
    if config.safe_mode {
        // The safety net is two-layered, because each catches what the other cannot:
        // (1) significant-token equality catches a dropped / reordered / corrupted *token*;
        if !same_significant_tokens(source, &out) {
            return source.to_owned();
        }
        // (2) a parse-validity recheck catches a meaning-changing *indentation* edit — indentation
        //     lives entirely in trivia/synthetic layout, so it is invisible to (1). If the input
        //     parsed clean, the output must too, else we fall back to the verbatim source.
        if input_parses && !gdscript_syntax::parse(&out).errors().is_empty() {
            return source.to_owned();
        }
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
    // `NewlinePhys` *not* so flagged is either a bracketed-continuation physical newline (kept
    // verbatim, interior preserved) or the terminator of a comment-only/blank line the prepass
    // copies verbatim *without* a synthetic `Newline` — those two are told apart by `bracket_depth`.
    let mut just_broke = false;
    // Open-bracket nesting depth of the significant tokens emitted so far. The prepass suppresses
    // synthetic line breaks inside brackets, so a `NewlinePhys` with `bracket_depth == 0` always
    // ends a logical (or comment-only) line, and the *next* line's indentation must be re-emitted.
    let mut bracket_depth: usize = 0;

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
                    trim_trailing_inline_ws(&mut out);
                    out.push('\n');
                    // Outside brackets this newline ends a comment-only / blank line the prepass
                    // copied verbatim (no synthetic `Newline`), so the next line must be
                    // re-indented. Inside brackets it is a real continuation — leave it verbatim.
                    if bracket_depth == 0 {
                        line_start = true;
                    }
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
                match t.kind {
                    SyntaxKind::LParen | SyntaxKind::LBrack | SyntaxKind::LBrace => {
                        bracket_depth += 1;
                    }
                    SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace => {
                        bracket_depth = bracket_depth.saturating_sub(1);
                    }
                    _ => {}
                }
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

    /// `parse(src).errors()` must be empty — the formatter must never emit code that fails to parse.
    fn parses_clean(src: &str) -> bool {
        gdscript_syntax::parse(src).errors().is_empty()
    }

    #[test]
    fn comment_between_statements_does_not_corrupt_the_next_line() {
        // A comment-only line is copied verbatim by the prepass (no synthetic Newline); the line
        // AFTER it must still be re-indented to the block depth, not left at its original spacing.
        let src = "func g():\n  var a = 1\n  # c\n  var x = 1\n  var y = 2\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func g():\n\tvar a = 1\n\t# c\n\tvar x = 1\n\tvar y = 2\n"
        );
        assert!(
            parses_clean(&out),
            "formatter must not emit mixed indent: {out:?}"
        );
        assert_eq!(fmt(&out), out, "must be idempotent");
    }

    #[test]
    fn leading_body_comment_does_not_corrupt_the_body() {
        // A comment that is the FIRST line of a block lands at column 0 (the prepass emits `Indent`
        // only at the first *code* line, so the block depth isn't known yet — a documented cosmetic
        // limitation, NOT a corruption). The CODE must still be correctly indented + parse clean.
        let src = "func g():\n  # c\n  var x = 1\n  var y = 2\n";
        let out = fmt(src);
        assert_eq!(out, "func g():\n# c\n\tvar x = 1\n\tvar y = 2\n");
        assert!(
            parses_clean(&out),
            "code must be correctly indented: {out:?}"
        );
        assert_eq!(fmt(&out), out, "must be idempotent");
    }

    #[test]
    fn doc_comment_between_statements_is_reindented_and_does_not_corrupt() {
        // A doc comment AFTER a code line (depth known) is re-indented like any line, and the line
        // following it must not be mis-indented.
        let src = "func g():\n  var a = 1\n  ## doc\n  var x = 1\n";
        let out = fmt(src);
        assert_eq!(out, "func g():\n\tvar a = 1\n\t## doc\n\tvar x = 1\n");
        assert!(parses_clean(&out), "{out:?}");
    }

    #[test]
    fn bracketed_continuation_interior_is_preserved() {
        // A physical newline INSIDE brackets is a real continuation — its interior spacing must be
        // kept verbatim (not treated like a comment-line terminator that re-indents the next line).
        let src = "func f():\n\tvar a = [\n\t\t1,\n\t\t2,\n\t]\n\treturn a\n";
        let out = fmt(src);
        assert!(parses_clean(&out), "{out:?}");
        assert!(super::same_significant_tokens(src, &out));
        assert_eq!(fmt(&out), out, "must be idempotent");
    }
}
