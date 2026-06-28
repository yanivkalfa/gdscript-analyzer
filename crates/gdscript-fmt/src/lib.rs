//! `gdscript-fmt` — the GDScript source formatter (Phase-6 Workstream 3).
//!
//! A pure `fn(source, &FmtConfig) -> String`: no engine model, no filesystem, `wasm32`-safe.
//! It re-emits the lexer/pre-pass token stream, normalizing **block indentation** (to the
//! configured unit), **trailing whitespace**, and the **final newline** — every *significant*
//! token (keywords, identifiers, literals — including multi-line strings, which are single tokens)
//! is emitted **verbatim**, so meaning cannot change.
//!
//! It also normalizes **intra-line spacing** (Phase-4 increment A): one space around binary
//! operators / assignments / `->` / `:=`, after `,` and `:` (in type-annotation and dict contexts),
//! hugged brackets (`f(x, y)`, `[1, 2]`), tight member access (`a.b`), and tight unary `-x`. The
//! decision is purely local (previous significant token + innermost bracket), and the genuinely
//! ambiguous contexts — slice colons `arr[a:b]`, and node-path sigils `$Node/Path` / `%Unique`
//! (where a stray space around `/` would silently change meaning **without** changing the token
//! sequence) — are left **verbatim** / kept tight by a small node-path state machine.
//!
//! **Safe by construction.** In `safe_mode` (the default) the formatter (a) refuses to touch a
//! file with syntax errors, and (b) re-lexes its own output and **falls back to the original** if
//! the significant token sequence changed. So it never corrupts code, even input it doesn't fully
//! understand. The result is idempotent: `format(format(x)) == format(x)`.
//!
//! Line-reflow / wrapping (full `gdformat` parity via a Wadler/Prettier `Doc`-IR pretty-printer)
//! is the documented next step — see `TECH_DEBT.md`.
#![cfg_attr(docsrs, feature(doc_cfg))]

use gdscript_syntax::SyntaxKind;

/// Formatter options. Defaults match the Godot convention (tabs) and keep the safety net on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "a plain user-facing options bag; each bool is an independent formatter toggle, not a state machine"
)]
pub struct FmtConfig {
    /// Indent with tabs (the Godot convention). `false` indents with [`indent_size`](Self::indent_size) spaces.
    pub use_tabs: bool,
    /// Spaces per indent level when `use_tabs` is `false`.
    pub indent_size: usize,
    /// The target line width for reflow. **Reserved** — line-wrapping is not yet implemented.
    pub line_width: usize,
    /// Normalize intra-line spacing between tokens (one space around binary operators, after
    /// `,`/`:`, hugged brackets, tight member access + unary). On by default. Turn off to format
    /// **indentation only** (the pre-increment-A behavior).
    pub normalize_spacing: bool,
    /// Collapse runs of blank lines (max 2 at top level, max 1 inside a block) and strip leading
    /// blank lines. On by default. (Does not yet *insert* blank lines around definitions.)
    pub collapse_blank_lines: bool,
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
            normalize_spacing: true,
            collapse_blank_lines: true,
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

/// Inter-token spacing: how to join two adjacent significant tokens on one logical line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spacing {
    /// Tight — no space (`a.b`, `f(`, before `,`/`)`).
    None,
    /// Exactly one space (` + `, `, ` after a comma, ` = `).
    Single,
    /// Keep the original inter-token whitespace — the ambiguous contexts we refuse to normalize
    /// (a slice colon `arr[a:b]`). Emits nothing when there was no original whitespace.
    Verbatim,
}

/// A value-completing token: a following `+`/`-`/`~` is **binary** (not unary), and a following
/// `(`/`[` is a **call**/**subscript** (not a grouping paren / array literal).
fn is_operand_end(k: SyntaxKind) -> bool {
    use SyntaxKind as S;
    matches!(
        k,
        S::Int
            | S::Float
            | S::String
            | S::StringName
            | S::NodePath
            | S::Ident
            | S::True
            | S::False
            | S::Null
            | S::ConstPi
            | S::ConstTau
            | S::ConstInf
            | S::ConstNan
            | S::SelfKw
            | S::SuperKw
            | S::RParen
            | S::RBrack
            | S::RBrace
    )
}

fn is_open_bracket(k: SyntaxKind) -> bool {
    matches!(
        k,
        SyntaxKind::LParen | SyntaxKind::LBrack | SyntaxKind::LBrace
    )
}

fn is_close_bracket(k: SyntaxKind) -> bool {
    matches!(
        k,
        SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace
    )
}

/// The spacing to insert *before* `cur`, given the previous significant token `prev` on the same
/// logical line, the innermost open-bracket kind `top`, and whether `prev` was a **unary** prefix
/// operator. Node-path runs (`$Node/Path`, `%Unique`) are forced tight by the caller and never
/// reach here, so a bare `Slash`/`Percent` here is always division/modulo.
fn space_before(
    prev: SyntaxKind,
    cur: SyntaxKind,
    top: Option<SyntaxKind>,
    prev_unary: bool,
) -> Spacing {
    use SyntaxKind as S;
    // --- tight-forcing rules (these carve out every no-space case; the default is one space) ---
    // Hug the inside of brackets: `(x`, `x)`, `[1`, `1]`, `{k`, `v}`.
    if is_open_bracket(prev) || is_close_bracket(cur) {
        return Spacing::None;
    }
    // Member access is tight both sides (`.5` / `1.0` are single Float tokens, so a bare `Dot` is
    // always member access).
    if cur == S::Dot || prev == S::Dot {
        return Spacing::None;
    }
    // No space before a separator; `@export` is tight after the `@`.
    if cur == S::Comma || cur == S::Semicolon || prev == S::At {
        return Spacing::None;
    }
    // Tight after a unary prefix operator: `-x`, `+x`, `~x`, `!x`.
    if prev == S::Tilde || prev == S::Bang || ((prev == S::Minus || prev == S::Plus) && prev_unary)
    {
        return Spacing::None;
    }
    // Colon: never a space *before* it; *after* it, a dict/type-annotation colon gets one space,
    // while a slice colon inside `[ ]` is left verbatim (a subscript and a slice are not
    // distinguishable from local context). `:=` is its own token, handled by the default.
    if cur == S::Colon {
        return if top == Some(S::LBrack) {
            Spacing::Verbatim
        } else {
            Spacing::None
        };
    }
    if prev == S::Colon {
        return if top == Some(S::LBrack) {
            Spacing::Verbatim
        } else {
            Spacing::Single
        };
    }
    // After a separator: one space (`a, b`).
    if prev == S::Comma || prev == S::Semicolon {
        return Spacing::Single;
    }
    // Open paren: a call hugs an operand callee (`f(`, `a.b(`, `preload(`, `assert(`), and a lambda
    // header hugs its `func` (`func(x):` — a bare `func(` is always a lambda; a named func has
    // `func name(`). A grouping paren after a value-keyword keeps its space (`return (x)`, `if (c)`).
    if cur == S::LParen {
        return if is_operand_end(prev)
            || prev == S::PreloadKw
            || prev == S::AssertKw
            || prev == S::FuncKw
        {
            Spacing::None
        } else {
            Spacing::Single
        };
    }
    // Open bracket: a subscript / typed-collection hugs an operand (`arr[i]`, `Array[int]`); an
    // array literal after an operator/keyword/`,` is spaced.
    if cur == S::LBrack {
        return if is_operand_end(prev) {
            Spacing::None
        } else {
            Spacing::Single
        };
    }
    // Everything else — binary/keyword operators, assignments, `->`, `:=`, `{`, atoms, words, and a
    // unary prefix *before* its operand — takes exactly one space.
    Spacing::Single
}

/// Emit a logical-line break: a real `\n` for a content line, or — when collapsing blank lines —
/// buffer a blank line into `pending_blanks` (flushed, capped, before the next content line).
fn emit_break(
    out: &mut String,
    collapse_on: bool,
    line_had_content: &mut bool,
    pending_blanks: &mut usize,
) {
    if collapse_on && !*line_had_content {
        *pending_blanks += 1; // a blank line — capped + flushed when the next content line starts.
    } else {
        out.push('\n');
    }
    *line_had_content = false;
}

/// Re-emit the pre-pass token stream with normalized **indentation**, **intra-line spacing** (when
/// `config.normalize_spacing`), trailing whitespace, and a single final newline. Significant token
/// *text* is always emitted verbatim (so meaning is preserved); only the whitespace *between*
/// tokens — and a logical line's leading indentation — is rewritten. Bracketed-continuation
/// interiors and node-path runs are kept verbatim / tight (see the module docs).
#[allow(
    clippy::too_many_lines,
    reason = "one cohesive token-stream state machine; the indentation, spacing, bracket-stack and node-path transitions are interdependent and clearer kept together than split across helpers"
)]
fn reindent(source: &str, config: &FmtConfig) -> String {
    let raw = gdscript_syntax::tokenize(source);
    let (toks, _diags) = gdscript_syntax::run_prepass(&raw, source);
    let unit = config.indent_unit();
    let spacing_on = config.normalize_spacing;
    let collapse_on = config.collapse_blank_lines;

    let mut out = String::with_capacity(source.len() + 16);
    let mut depth: usize = 0;
    // --- blank-line state (only used when `collapse_on`) ---
    // Blank lines are buffered, not emitted, until the next content line: we then flush at most
    // `cap` of them (2 at top level, 1 nested) — knowing the *next* line's depth, since the prepass
    // emits the `Dedent` between a block and a following top-level line AFTER the blank lines.
    let mut pending_blanks: usize = 0;
    // Whether the current logical line emitted any content (a significant token or a comment) — a
    // line that did not is a blank line.
    let mut line_had_content = false;
    // Whether any content has been emitted yet (leading blank lines are stripped entirely).
    let mut seen_content = false;
    // `true` at the start of a logical line, before its first significant token (when we re-emit
    // the indentation, `depth` being final by then).
    let mut line_start = true;
    // A synthetic `Newline` precedes the `NewlinePhys` carrying the line's bytes; this swallows that
    // one `NewlinePhys` so the break is emitted once. See the original notes below.
    let mut just_broke = false;
    // Innermost-first stack of open-bracket kinds: `.len()` is the old `bracket_depth` (drives the
    // continuation logic), `.last()` is the colon-context discriminator for spacing.
    let mut stack: Vec<SyntaxKind> = Vec::new();
    // --- intra-line spacing state (all reset at a logical-line break) ---
    let mut prev_sig: Option<SyntaxKind> = None;
    let mut prev_unary = false;
    let mut node_path = false;
    // Original inter-token whitespace, buffered so the next token can keep it (`Verbatim`) or drop
    // it (and re-synthesize). Only used when `spacing_on`.
    let mut pending_ws: Option<&str> = None;
    // Set after a physical newline *inside* brackets: the next line's leading whitespace (alignment)
    // is kept verbatim — increment A does not reflow bracketed continuations.
    let mut cont_line_start = false;

    for t in &toks {
        let text = &source[t.range];
        match t.kind {
            SyntaxKind::Indent => depth += 1,
            SyntaxKind::Dedent => depth = depth.saturating_sub(1),
            // A synthetic line break: ends the logical line; the next one is re-indented. A synthetic
            // `Newline` only follows a statement, so it always terminates a content line.
            SyntaxKind::Newline => {
                trim_trailing_inline_ws(&mut out);
                if stack.is_empty() {
                    // A normal logical-line break: the next line is re-indented from `depth`.
                    emit_break(
                        &mut out,
                        collapse_on,
                        &mut line_had_content,
                        &mut pending_blanks,
                    );
                    line_start = true;
                    prev_sig = None;
                } else {
                    // A synthetic break *inside brackets* is a multi-line lambda-body line break:
                    // the prepass suppresses synthetic layout inside brackets EXCEPT for a lambda
                    // body, for which it re-emits `Newline`/`Indent`/`Dedent`. Treat it like a
                    // `NewlinePhys` continuation and keep the bracketed interior verbatim — do NOT
                    // re-indent the body from `depth`. The body's verbatim-aligned *header* arrives
                    // via `NewlinePhys` (kept verbatim), so depth-re-indenting only the body makes
                    // the two disagree and produces non-parsing output. Canonical reflow of a
                    // bracketed lambda block is the increment-C reflow's job, not the indenter's.
                    out.push('\n');
                    cont_line_start = true;
                }
                just_broke = true;
                node_path = false;
                pending_ws = None;
            }
            SyntaxKind::NewlinePhys => {
                if just_broke {
                    just_broke = false; // its bytes belong to the synthetic break already emitted
                } else {
                    trim_trailing_inline_ws(&mut out);
                    node_path = false;
                    pending_ws = None;
                    // Outside brackets this ends a comment-only line (content) or a blank line, and
                    // the next line is re-indented. Inside brackets it is a real continuation — keep
                    // the next line's alignment verbatim and never collapse it.
                    if stack.is_empty() {
                        emit_break(
                            &mut out,
                            collapse_on,
                            &mut line_had_content,
                            &mut pending_blanks,
                        );
                        line_start = true;
                        prev_sig = None;
                    } else {
                        out.push('\n');
                        cont_line_start = true;
                    }
                }
            }
            SyntaxKind::Whitespace => {
                if line_start {
                    // Leading indentation — dropped; re-synthesized at the first significant token.
                } else if spacing_on {
                    pending_ws = Some(text); // buffered; the next token decides whether to keep it.
                } else {
                    out.push_str(text); // indentation-only mode: original verbatim behavior.
                }
            }
            // A significant token or a comment.
            _ => {
                if line_start {
                    // Flush the buffered blank lines, capped to the *next* line's context: 2 at top
                    // level, 1 inside a block, and 0 before the first content (leading blanks gone).
                    if collapse_on {
                        let cap = if !seen_content {
                            0
                        } else if depth == 0 {
                            2
                        } else {
                            1
                        };
                        for _ in 0..pending_blanks.min(cap) {
                            out.push('\n');
                        }
                        pending_blanks = 0;
                    }
                    for _ in 0..depth {
                        out.push_str(&unit);
                    }
                    line_start = false;
                    cont_line_start = false;
                } else if spacing_on {
                    if cont_line_start {
                        // First token of a bracketed continuation: keep its leading alignment.
                        if let Some(ws) = pending_ws {
                            out.push_str(ws);
                        }
                        cont_line_start = false;
                    } else if t.kind.is_trivia() {
                        // A comment / line-continuation / BOM: keep the original spacing before it.
                        if let Some(ws) = pending_ws {
                            out.push_str(ws);
                        }
                        node_path = false;
                    } else {
                        // A significant token: synthesize the spacing. Inside a node-path run we
                        // keep the *original* spacing verbatim — a real `$Node/Path` is already
                        // tight (so it stays tight), and we must never *collapse* a genuinely-spaced
                        // `$A / b` (a division) into a node path, which would silently change meaning
                        // with an identical token sequence (the safety net cannot catch it).
                        let spacing = if node_path
                            && matches!(
                                t.kind,
                                SyntaxKind::Ident | SyntaxKind::Slash | SyntaxKind::String
                            ) {
                            Spacing::Verbatim
                        } else {
                            node_path = false; // any non-path token ends the run.
                            match prev_sig {
                                Some(p) => {
                                    space_before(p, t.kind, stack.last().copied(), prev_unary)
                                }
                                None => Spacing::None,
                            }
                        };
                        match spacing {
                            Spacing::None => {}
                            Spacing::Single => out.push(' '),
                            Spacing::Verbatim => {
                                if let Some(ws) = pending_ws {
                                    out.push_str(ws);
                                }
                            }
                        }
                    }
                }
                pending_ws = None;
                just_broke = false;
                out.push_str(text);
                // Any token reaching here (a significant token or a comment) is line content.
                line_had_content = true;
                seen_content = true;
                match t.kind {
                    SyntaxKind::LParen | SyntaxKind::LBrack | SyntaxKind::LBrace => {
                        stack.push(t.kind);
                    }
                    SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace => {
                        stack.pop();
                    }
                    _ => {}
                }
                // Spacing state — significant tokens only (a comment is never an operand).
                if !t.kind.is_trivia() {
                    let unary_ctx = prev_sig.is_none_or(|p| !is_operand_end(p));
                    // Enter node-path mode after a `$` (always) or a `%` in sigil position (a `%`
                    // after an operand is modulo, and stays a normal binary operator).
                    if t.kind == SyntaxKind::Dollar || (t.kind == SyntaxKind::Percent && unary_ctx)
                    {
                        node_path = true;
                    }
                    prev_unary = match t.kind {
                        SyntaxKind::Minus | SyntaxKind::Plus => unary_ctx,
                        SyntaxKind::Tilde | SyntaxKind::Bang => true,
                        _ => false,
                    };
                    prev_sig = Some(t.kind);
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

    // ---- Phase-4 increment A: intra-line spacing ----

    /// Format a single statement inside a function body, returning just the (de-indented) body line.
    /// Wrapping keeps `safe_mode` happy (a bare statement is not a valid top-level form).
    fn fmt_stmt(stmt: &str) -> String {
        let src = format!("func _f():\n\t{stmt}\n");
        let out = fmt(&src);
        out.strip_prefix("func _f():\n\t")
            .and_then(|s| s.strip_suffix('\n'))
            .unwrap_or(&out)
            .to_owned()
    }

    #[test]
    fn spacing_operators_and_assignment() {
        assert_eq!(fmt_stmt("var x=a+b"), "var x = a + b");
        assert_eq!(fmt_stmt("var x = a-b"), "var x = a - b"); // binary minus
        assert_eq!(fmt_stmt("var t = a   *   b"), "var t = a * b"); // collapse runs
        assert_eq!(
            fmt_stmt("var z = a==b and c!=d"),
            "var z = a == b and c != d"
        );
        assert_eq!(fmt_stmt("x+=1"), "x += 1");
    }

    #[test]
    fn spacing_brackets_and_commas() {
        assert_eq!(fmt_stmt("foo( x ,y )"), "foo(x, y)");
        assert_eq!(fmt_stmt("var a = [1,2]"), "var a = [1, 2]");
        assert_eq!(
            fmt_stmt("var d = {\"x\":1,\"y\":2}"),
            "var d = {\"x\": 1, \"y\": 2}"
        );
        assert_eq!(
            fmt_stmt("var n = obj . field . method ( )"),
            "var n = obj.field.method()"
        );
    }

    #[test]
    fn spacing_type_annotation_and_default_args() {
        assert_eq!(fmt_stmt("var x:int=1"), "var x: int = 1");
        // Typed default `=` is spaced (we do not replicate Black's untyped `x=1`); arrow + header colon.
        assert_eq!(
            fmt("func f(a,b:int=1)->int:\n\treturn 0\n"),
            "func f(a, b: int = 1) -> int:\n\treturn 0\n"
        );
        assert_eq!(fmt_stmt("var a:Array[int]=[]"), "var a: Array[int] = []");
    }

    #[test]
    fn spacing_unary_minus() {
        assert_eq!(fmt_stmt("var x = -1"), "var x = -1"); // unary after `=`
        assert_eq!(fmt_stmt("foo( -1 , -2 )"), "foo(-1, -2)"); // unary in call
        assert_eq!(fmt_stmt("var a = [-1,-2]"), "var a = [-1, -2]"); // unary in array
        assert_eq!(fmt_stmt("var n = -2**2"), "var n = -2 ** 2"); // unary then power
        assert_eq!(fmt_stmt("var d = a - -b"), "var d = a - -b"); // binary then unary
    }

    #[test]
    fn spacing_percent_is_modulo_or_format_when_after_an_operand() {
        assert_eq!(fmt_stmt("var r = a%b"), "var r = a % b"); // modulo
        assert_eq!(fmt_stmt("var s = \"%d\"%n"), "var s = \"%d\" % n"); // format operator
    }

    #[test]
    fn spacing_node_paths_stay_tight() {
        // The correctness-critical cases: a node path must NOT gain spaces around `/` (that would
        // turn `$Player/Bone` into a division with an identical token sequence).
        assert_eq!(
            fmt_stmt("var n = get_node($Player/Bone)"),
            "var n = get_node($Player/Bone)"
        );
        assert_eq!(fmt_stmt("var u = %Unique/Child"), "var u = %Unique/Child");
        assert_eq!(
            fmt_stmt("var p = $\"Player\".position"),
            "var p = $\"Player\".position"
        );
        // StringName / NodePath literals are single tokens — untouched atoms.
        assert_eq!(fmt_stmt("var v = &\"Name\""), "var v = &\"Name\"");
        assert_eq!(fmt_stmt("var q = ^\"a/b\""), "var q = ^\"a/b\"");
    }

    #[test]
    fn spacing_keywords_paren_callee_and_grouping() {
        assert_eq!(
            fmt_stmt("var p = preload ( \"res://x.gd\" )"),
            "var p = preload(\"res://x.gd\")"
        );
        assert_eq!(fmt_stmt("var x = a if c else b"), "var x = a if c else b"); // ternary
        assert_eq!(fmt_stmt("var y = not  flag"), "var y = not flag");
        assert_eq!(fmt_stmt("var z = n is int"), "var z = n is int");
        // a grouping paren after a value-keyword keeps its space; a call paren hugs.
        assert_eq!(fmt_stmt("return ( x )"), "return (x)");
    }

    #[test]
    fn spacing_lambda_func_paren_is_tight() {
        // Corpus regression: a lambda `func(...)` must hug its `func` — `func (` does not parse.
        // (A named function declaration has `func name(`, which is unaffected.)
        assert_eq!(
            fmt_stmt("var cb = func( ) -> void:\n\t\tpass"),
            "var cb = func() -> void:\n\t\tpass"
        );
        assert_eq!(
            fmt_stmt("var g = func(_text:String)->void:\n\t\tpass"),
            "var g = func(_text: String) -> void:\n\t\tpass"
        );
        assert_eq!(
            fmt("func named(a,b):\n\tpass\n"),
            "func named(a, b):\n\tpass\n"
        );
    }

    #[test]
    fn multiline_lambda_argument_interior_is_kept_verbatim() {
        // A multi-line lambda passed as a call argument is a block *inside* brackets. The prepass
        // re-emits synthetic layout (`Newline`/`Indent`/`Dedent`) for the lambda body even inside
        // the brackets; the indenter treats that synthetic break like a `NewlinePhys` continuation
        // and keeps the whole bracketed interior **verbatim** — it does NOT re-indent the body from
        // block depth (which used to disagree with the verbatim-aligned header and emit non-parsing
        // code, the one pre-existing limitation that previously needed a safe_mode fallback).
        // Canonical reflow of the bracketed block is the increment-C reflow's job.
        let src = "func _r():\n\tx.connect(func() -> void:\n\t\tdo_thing()\n\t)\n";
        let out = fmt(src);
        assert_eq!(
            out, src,
            "bracketed lambda interior must be preserved verbatim"
        );
        assert!(parses_clean(&out), "{out:?}");
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn over_indented_lambda_in_brackets_does_not_corrupt() {
        // The exact corpus shapes (godot-demo-projects rhythm_game) that used to format to
        // non-parsing code: the lambda HEADER sits on its own bracket-continuation line at an
        // author-chosen over-indent, and the BODY (synthetic-Newline) must stay aligned with it,
        // not snap back to block depth. Both are now kept verbatim and parse.
        let note_manager = "func _ready() -> void:\n\t_play_stats.changed.connect(\n\t\t\tfunc() -> void:\n\t\t\t\tplay_stats_updated.emit(_play_stats)\n\t\t\t\t)\n";
        let main_gd = "func _r() -> void:\n\tlatency_line_edit.text_submitted.connect(\n\t\tfunc(_text: String) -> void:\n\t\t\tlatency_line_edit.release_focus())\n";
        for src in [note_manager, main_gd] {
            // safe_mode OFF so a regression would surface as a real assert, not a silent fallback.
            let cfg = FmtConfig {
                safe_mode: false,
                ..FmtConfig::default()
            };
            let out = format(src, &cfg);
            assert!(parses_clean(&out), "lambda-in-brackets must parse: {out:?}");
            assert!(
                super::same_significant_tokens(src, &out),
                "tokens changed: {out:?}"
            );
            assert_eq!(format(&out, &cfg), out, "idempotent: {out:?}");
        }
    }

    #[test]
    fn spacing_annotation_is_tight() {
        assert_eq!(
            fmt("@export_range(0,100)\nvar speed = 1\n"),
            "@export_range(0, 100)\nvar speed = 1\n"
        );
        assert_eq!(fmt("@export var hp=100\n"), "@export var hp = 100\n");
    }

    #[test]
    fn spacing_colon_in_brackets_left_verbatim() {
        // GDScript has no Python-style slice syntax, so a colon inside `[ ]` never appears in valid
        // code — but defensively we leave its spacing verbatim (not locally distinguishable from
        // other colon roles). Exercised with safe_mode OFF, since the construct does not parse; the
        // rest of the line is still normalized while the `[a:b]` colon spacing is preserved.
        let cfg = FmtConfig {
            safe_mode: false,
            ..FmtConfig::default()
        };
        assert_eq!(
            format("func _f():\n\tvar d = data[a:b]\n", &cfg),
            "func _f():\n\tvar d = data[a:b]\n"
        );
        assert_eq!(
            format("func _f():\n\tvar e=data[a : b]\n", &cfg),
            "func _f():\n\tvar e = data[a : b]\n"
        );
    }

    #[test]
    fn spacing_is_idempotent_across_cases() {
        let cases = [
            "var x = a+b*c-d",
            "func f(a,b:int=1)->int:\n\treturn a",
            "var n = get_node($Player/Bone).position",
            "var d = {\"k\": foo(-1, 2), \"m\": items.slice(i, j)}",
            "var z = a if b<c else -d",
        ];
        for c in cases {
            let src = format!("func _w():\n\t{c}\n");
            let once = fmt(&src);
            assert_eq!(fmt(&once), once, "not idempotent for {c:?}: {once:?}");
            assert!(parses_clean(&once), "did not parse: {once:?}");
            assert!(
                super::same_significant_tokens(&src, &once),
                "tokens changed for {c:?}"
            );
        }
    }

    // ---- Phase-4 increment B: blank-line policy ----

    #[test]
    fn blank_lines_collapsed_top_level_to_two() {
        // 4 blank lines between two top-level functions collapse to 2 (the cap uses the *next*
        // line's depth — 0 here — even though the Dedent lands after the blanks).
        let src = "func a():\n\tpass\n\n\n\n\nfunc b():\n\tpass\n";
        assert_eq!(fmt(src), "func a():\n\tpass\n\n\nfunc b():\n\tpass\n");
    }

    #[test]
    fn blank_lines_collapsed_inside_block_to_one() {
        let src = "func a():\n\tvar x = 1\n\n\n\n\tvar y = 2\n";
        assert_eq!(fmt(src), "func a():\n\tvar x = 1\n\n\tvar y = 2\n");
    }

    #[test]
    fn leading_blank_lines_stripped() {
        assert_eq!(fmt("\n\n\nfunc a():\n\tpass\n"), "func a():\n\tpass\n");
    }

    #[test]
    fn single_blank_line_is_preserved() {
        let src = "func a():\n\tpass\n\nfunc b():\n\tpass\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn blank_lines_inside_a_multiline_string_are_untouched() {
        // The blank line lives inside a `"""..."""` token, NOT between logical lines — the
        // token-level pass must never see it as a collapsible blank.
        let src = "func a():\n\tvar s = \"\"\"x\n\n\n\ny\"\"\"\n\treturn s\n";
        let out = fmt(src);
        assert!(
            out.contains("x\n\n\n\ny"),
            "string interior collapsed: {out:?}"
        );
        assert!(super::same_significant_tokens(src, &out));
    }

    #[test]
    fn blank_lines_off_preserved() {
        let cfg = FmtConfig {
            collapse_blank_lines: false,
            ..FmtConfig::default()
        };
        let src = "func a():\n\tpass\n\n\n\n\nfunc b():\n\tpass\n";
        assert_eq!(format(src, &cfg), src);
    }

    #[test]
    fn spacing_off_is_indentation_only() {
        // With `normalize_spacing` off, the formatter touches indentation only (the old behavior):
        // intra-line spacing is left exactly as written.
        let cfg = FmtConfig {
            normalize_spacing: false,
            ..FmtConfig::default()
        };
        let src = "func f():\n    var x=a+b\n";
        assert_eq!(format(src, &cfg), "func f():\n\tvar x=a+b\n");
    }
}
