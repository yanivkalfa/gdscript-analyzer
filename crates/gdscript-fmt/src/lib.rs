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
//! It also performs **length-driven line reflow** (Phase-4 increment C): a single-line statement
//! that exceeds `line_width` and contains a bracketed group is wrapped flat → compact → exploded via
//! a small `Doc`-IR, matching gdformat and preserving the token sequence. The remaining gdformat
//! behaviours (magic trailing comma, operator-chain paren injection, quote normalization) are
//! token-mutating and documented in `DEVIATIONS.md`.
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
    /// The target line width for [`reflow`](Self::reflow) (default 100).
    pub line_width: usize,
    /// Normalize intra-line spacing between tokens (one space around binary operators, after
    /// `,`/`:`, hugged brackets, tight member access + unary). On by default. Turn off to format
    /// **indentation only** (the pre-increment-A behavior).
    pub normalize_spacing: bool,
    /// Collapse runs of blank lines (max 2 at top level, max 1 inside a block) and strip leading
    /// blank lines. On by default.
    pub collapse_blank_lines: bool,
    /// Insert blank lines around definitions to match gdformat (2 around top-level `func`/`class`/
    /// `static func`, 1 around nested ones; comments/annotations attached to a def move with it). On
    /// by default. Purely additive — never changes the significant token sequence.
    pub insert_blank_lines: bool,
    /// Wrap a single-line statement that exceeds [`line_width`](Self::line_width) and contains a
    /// bracketed group (call / array / dict / parameter list) — flat → compact → exploded, matching
    /// gdformat's length-driven layout. On by default. Token-preserving (no trailing comma added).
    pub reflow: bool,
    /// Normalize string-literal quotes to gdformat's style: prefer `"`, fall back to `'` only when
    /// the body has more `"` than `'`. On by default. Preserves the string's value (a token-mutating
    /// rewrite guarded by the meaning-equivalence net).
    pub normalize_strings: bool,
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
            insert_blank_lines: true,
            reflow: true,
            normalize_strings: true,
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
///
/// The source's line-ending style is preserved: a file using `\r\n` (a Windows checkout) is
/// formatted internally in `\n` and re-emitted with `\r\n`, so the formatter never churns every line
/// by flipping CRLF to LF (matching gdformat, which preserves line endings).
#[must_use]
pub fn format(source: &str, config: &FmtConfig) -> String {
    if source.contains("\r\n") {
        let lf = source.replace("\r\n", "\n");
        return format_lf(&lf, config).replace('\n', "\r\n");
    }
    format_lf(source, config)
}

/// `format` working purely in LF (the caller handles CRLF round-tripping).
fn format_lf(source: &str, config: &FmtConfig) -> String {
    let input_parses = gdscript_syntax::parse(source).errors().is_empty();
    // Safe mode: never reformat around a syntax error — we'd risk mis-indenting a mis-parsed block.
    if config.safe_mode && !input_parses {
        return source.to_owned();
    }
    let mut out = reindent(source, config);
    if config.insert_blank_lines {
        // Purely additive (only inserts blank lines), so the significant-token net still holds.
        out = insert_def_blanks(&out, config);
    }
    if config.reflow {
        // Length-driven wrapping; token-preserving (no trailing comma added).
        out = reflow(&out, config);
    }
    if config.safe_mode {
        // The safety net is two-layered, because each catches what the other cannot:
        // (1) meaning-equivalence catches a dropped / reordered / corrupted *token* — while
        //     normalising away the rewrites the formatter is allowed to make (string-quote style,
        //     trailing commas);
        if !meaning_preserved(source, &out) {
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

/// The depth and leading-indentation length of the next code line after token index `idx` (skipping
/// comment / blank lines), given the current raw depth `cur`. Used to place a block-boundary comment.
fn next_code_info(toks: &[gdscript_syntax::RawToken], idx: usize, cur: usize) -> (usize, usize) {
    use SyntaxKind as S;
    let mut delta: i32 = 0;
    let mut indent_len = 0usize;
    for t in &toks[idx + 1..] {
        match t.kind {
            S::Indent => delta += 1,
            S::Dedent => delta -= 1,
            S::NewlinePhys | S::Newline => indent_len = 0,
            S::Whitespace => indent_len = usize::from(t.range.len()),
            k if k.is_trivia() => {} // comments / line-continuation / BOM
            _ => {
                let d = usize::try_from(i32::try_from(cur).unwrap_or(0) + delta).unwrap_or(0);
                return (d, indent_len);
            }
        }
    }
    (cur, 0)
}

/// The intended depth of a block-boundary comment, matching gdformat: its authored indentation
/// clamped to the surrounding structure. We compare the comment's indentation *length* against the
/// previous and next code lines' — if it reaches the deeper line's indentation it joins that block,
/// otherwise it stays at the shallower one (so a column-0 comment stays at column 0 and an
/// over-indented one snaps in). Comparing lengths (not levels) makes it indent-width agnostic.
fn comment_depth(
    comment_len: usize,
    prev_depth: usize,
    prev_len: usize,
    next_depth: usize,
    next_len: usize,
) -> usize {
    let ((deep_depth, deep_len), (shallow_depth, _)) = if prev_depth >= next_depth {
        ((prev_depth, prev_len), (next_depth, next_len))
    } else {
        ((next_depth, next_len), (prev_depth, prev_len))
    };
    if comment_len >= deep_len {
        deep_depth
    } else {
        shallow_depth
    }
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
    // The leading whitespace of the current logical line (used to recover a *comment*'s authored
    // indentation, since the prepass attributes a block-boundary comment to the wrong raw `depth`).
    let mut line_indent_ws: Option<&str> = None;
    // The leading-indentation length of the most recent *code* line — the "previous code line" a
    // block-boundary comment is placed against.
    let mut prev_code_indent_len: usize = 0;

    for (idx, t) in toks.iter().enumerate() {
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
                line_indent_ws = None;
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
                        line_indent_ws = None;
                    } else {
                        out.push('\n');
                        cont_line_start = true;
                    }
                }
            }
            SyntaxKind::Whitespace => {
                if line_start {
                    // Leading indentation — dropped; re-synthesized at the first significant token.
                    // Remember it: a *comment*'s authored indentation is recovered from it below.
                    line_indent_ws = Some(text);
                } else if spacing_on {
                    pending_ws = Some(text); // buffered; the next token decides whether to keep it.
                } else {
                    out.push_str(text); // indentation-only mode: original verbatim behavior.
                }
            }
            // A significant token or a comment.
            _ => {
                if line_start {
                    // A block-boundary *comment* is attributed by the prepass to the wrong raw
                    // `depth` (the `Indent`/`Dedent` lands on the next *code* line, not the comment).
                    // Recover its intended depth the way gdformat does: its authored indentation,
                    // clamped to the surrounding structure — `min(authored, max(prev, next))`.
                    let is_comment = matches!(
                        t.kind,
                        SyntaxKind::LineComment
                            | SyntaxKind::DocComment
                            | SyntaxKind::RegionComment
                            | SyntaxKind::EndRegionComment
                    );
                    let comment_len = line_indent_ws.map_or(0, str::len);
                    let emit_depth = if is_comment {
                        let (next_depth, next_len) = next_code_info(&toks, idx, depth);
                        comment_depth(
                            comment_len,
                            depth,
                            prev_code_indent_len,
                            next_depth,
                            next_len,
                        )
                    } else {
                        prev_code_indent_len = comment_len;
                        depth
                    };
                    // Flush the buffered blank lines, capped to this line's context: 2 at top level,
                    // 1 inside a block, and 0 before the first content (leading blanks gone).
                    if collapse_on {
                        let cap = if !seen_content {
                            0
                        } else if emit_depth == 0 {
                            2
                        } else {
                            1
                        };
                        for _ in 0..pending_blanks.min(cap) {
                            out.push('\n');
                        }
                        pending_blanks = 0;
                    }
                    for _ in 0..emit_depth {
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
                // String literals are normalized to gdformat's canonical quote style (value-
                // preserving; guarded by the meaning-equivalence net). Everything else is verbatim.
                if config.normalize_strings
                    && matches!(
                        t.kind,
                        SyntaxKind::String | SyntaxKind::StringName | SyntaxKind::NodePath
                    )
                {
                    out.push_str(&canonical_string(text));
                } else {
                    out.push_str(text);
                }
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

/// The role a statement (logical) line plays in the blank-line-insertion policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineRole {
    /// A `func` / `static func` / `class` declaration — gdformat surrounds these with blank lines.
    Def,
    /// A standalone comment line (`#` / `##`) — attaches to a following statement.
    Comment,
    /// A standalone annotation line (`@foo` with nothing else on the line) — attaches to a following
    /// statement.
    Annotation,
    /// Any other statement (`var` / `const` / `signal` / `class_name` / an expression / …).
    Other,
}

/// A statement-level "head" line discovered in already-formatted text: its 0-based physical line
/// number, its block depth, and its [`LineRole`]. Bracket-continuation and lambda-body lines are
/// not heads.
#[derive(Clone, Copy)]
struct HeadLine {
    line: usize,
    depth: usize,
    role: LineRole,
}

/// One logical unit for the blank-line edge rule: a definition (or other statement) together with
/// any comment/annotation prefix that attaches to it. `head_line` is the unit's first physical line.
#[derive(Clone, Copy)]
struct Unit {
    head_line: usize,
    depth: usize,
    is_def: bool,
}

/// Classify the statement line whose first significant-or-comment token is `toks[start]`. Scans the
/// rest of the logical line (across bracketed continuations) to look past an annotation prefix.
fn classify_line(toks: &[gdscript_syntax::RawToken], start: usize) -> LineRole {
    use SyntaxKind as S;
    if matches!(
        toks[start].kind,
        S::LineComment | S::DocComment | S::RegionComment | S::EndRegionComment
    ) {
        return LineRole::Comment;
    }
    // Collect the line's significant token kinds (skip trivia; stop at the logical line end).
    let mut kinds: Vec<S> = Vec::new();
    let mut local_stack = 0usize;
    for t in &toks[start..] {
        match t.kind {
            S::Newline => break,
            S::NewlinePhys if local_stack == 0 => break,
            S::LParen | S::LBrack | S::LBrace => {
                local_stack += 1;
                kinds.push(t.kind);
            }
            S::RParen | S::RBrack | S::RBrace => {
                local_stack = local_stack.saturating_sub(1);
                kinds.push(t.kind);
            }
            k if k.is_trivia() || k.is_synthetic_layout() => {}
            k => kinds.push(k),
        }
    }
    // Skip a leading annotation prefix: `@ Ident (balanced parens)?`, repeated.
    let mut i = 0;
    while kinds.get(i) == Some(&S::At) {
        i += 1; // `@`
        if kinds.get(i) == Some(&S::Ident) {
            i += 1; // annotation name
        }
        if kinds.get(i) == Some(&S::LParen) {
            // skip the balanced argument list
            let mut d = 0usize;
            while let Some(&k) = kinds.get(i) {
                i += 1;
                match k {
                    S::LParen => d += 1,
                    S::RParen => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    match kinds.get(i) {
        None => LineRole::Annotation, // an annotation alone on its line
        Some(S::FuncKw | S::ClassKw) => LineRole::Def,
        Some(S::StaticKw) if kinds.get(i + 1) == Some(&S::FuncKw) => LineRole::Def,
        _ => LineRole::Other,
    }
}

/// Insert blank lines around definitions to match gdformat's policy (2 around top-level defs, 1
/// around nested defs), operating on already-formatted, already-collapsed text. Purely additive —
/// it only inserts blank lines, so it never changes the significant token sequence. Idempotent: the
/// blanks it would add are already present on a second pass.
#[allow(
    clippy::too_many_lines,
    reason = "three cohesive sequential passes (find heads, group units, apply the edge rule) over the same token stream; clearer kept together than split"
)]
fn insert_def_blanks(formatted: &str, config: &FmtConfig) -> String {
    use SyntaxKind as S;
    let raw = gdscript_syntax::tokenize(formatted);
    let (toks, _diags) = gdscript_syntax::run_prepass(&raw, formatted);

    let lines: Vec<&str> = formatted.lines().collect();
    let is_blank: Vec<bool> = lines.iter().map(|l| l.trim().is_empty()).collect();
    let blank_above = |l: usize| l > 0 && is_blank.get(l - 1).copied().unwrap_or(false);

    // Byte offset -> 0-based physical line number. Built from the raw bytes (so newlines *inside*
    // multi-line string tokens count too) — `formatted.lines()` indices must align with this.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            formatted
                .bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        )
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset) - 1;
    // The (already-normalized) leading-indentation depth of a formatted line.
    let line_depth = |l: usize| -> usize {
        let s = lines.get(l).copied().unwrap_or("");
        if config.use_tabs {
            s.bytes().take_while(|&b| b == b'\t').count()
        } else {
            s.bytes()
                .take_while(|&b| b == b' ')
                .count()
                .checked_div(config.indent_size)
                .unwrap_or(0)
        }
    };

    // --- pass 1: find statement-level head lines (skip bracket / lambda continuations) ---
    let mut heads: Vec<HeadLine> = Vec::new();
    let mut depth = 0usize;
    let mut stack = 0usize;
    let mut this_line_continues = false; // a trailing `\` continues onto the next physical line
    let mut next_line_is_cont = false; // the logical line we are about to start is a continuation
    let mut seen_first_on_line = false;
    for (idx, t) in toks.iter().enumerate() {
        match t.kind {
            S::Indent => depth += 1,
            S::Dedent => depth = depth.saturating_sub(1),
            S::NewlinePhys => {
                next_line_is_cont = stack > 0 || this_line_continues;
                this_line_continues = false;
                seen_first_on_line = false;
            }
            S::LineContinuation => this_line_continues = true,
            S::Newline | S::Whitespace | S::Bom => {}
            _ => {
                if !seen_first_on_line {
                    seen_first_on_line = true;
                    if stack == 0 && !next_line_is_cont {
                        let line = line_of(usize::from(t.range.start()));
                        let role = classify_line(&toks, idx);
                        // A comment's *structural* block (which one it belongs to, for the edge
                        // rule) is not its visual indentation: a comment that *leads* a deeper block
                        // (next code is deeper) belongs to that block — even at column 0 — so it is
                        // not mistaken for a sibling of the preceding def; otherwise it sits at its
                        // own (visual) depth, which correctly attaches a column-0 doc-comment to a
                        // following *dedented* def.
                        let head_depth = if role == LineRole::Comment {
                            let next_depth = next_code_info(&toks, idx, depth).0;
                            if next_depth > depth {
                                next_depth
                            } else {
                                line_depth(line)
                            }
                        } else {
                            depth
                        };
                        heads.push(HeadLine {
                            line,
                            depth: head_depth,
                            role,
                        });
                    }
                }
                match t.kind {
                    S::LParen | S::LBrack | S::LBrace => stack += 1,
                    S::RParen | S::RBrack | S::RBrace => stack = stack.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }

    // --- pass 2: group heads into units (a def/statement + its attached comment/annotation prefix) ---
    let mut units: Vec<Unit> = Vec::new();
    let mut i = 0;
    while i < heads.len() {
        let h = heads[i];
        if matches!(h.role, LineRole::Comment | LineRole::Annotation) {
            // Accumulate a contiguous, same-depth prefix run of comments/annotations.
            let mut j = i + 1;
            while j < heads.len()
                && matches!(heads[j].role, LineRole::Comment | LineRole::Annotation)
                && heads[j].depth == h.depth
                && !blank_above(heads[j].line)
            {
                j += 1;
            }
            // Does a same-depth statement follow contiguously (no blank gap)? Then the run attaches.
            if j < heads.len()
                && heads[j].depth == h.depth
                && !blank_above(heads[j].line)
                && matches!(heads[j].role, LineRole::Def | LineRole::Other)
            {
                units.push(Unit {
                    head_line: h.line,
                    depth: h.depth,
                    is_def: heads[j].role == LineRole::Def,
                });
                i = j + 1;
            } else {
                // Standalone comment/annotation lines (each its own non-def unit).
                for h in &heads[i..j] {
                    units.push(Unit {
                        head_line: h.line,
                        depth: h.depth,
                        is_def: false,
                    });
                }
                i = j;
            }
        } else {
            units.push(Unit {
                head_line: h.line,
                depth: h.depth,
                is_def: h.role == LineRole::Def,
            });
            i += 1;
        }
    }

    // --- pass 3: the edge rule — N blanks before a unit whose own or previous sibling is a def ---
    let mut required: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut last_is_def: Vec<Option<bool>> = Vec::new();
    for u in &units {
        while last_is_def.len() <= u.depth {
            last_is_def.push(None);
        }
        let prev = last_is_def[u.depth];
        let first_in_block = prev.is_none();
        let n = if u.depth == 0 { 2 } else { 1 };
        if !first_in_block && (u.is_def || prev == Some(true)) {
            required.insert(u.head_line, n);
        }
        last_is_def[u.depth] = Some(u.is_def);
        last_is_def.truncate(u.depth + 1); // entering a deeper block starts its siblings fresh
    }
    if required.is_empty() {
        return formatted.to_owned();
    }

    // --- rebuild: top up the blank run before each required head to exactly N ---
    let mut out = String::with_capacity(formatted.len() + required.len() * 2);
    let mut trailing_blanks = 0usize;
    for (lno, content) in lines.iter().enumerate() {
        if let Some(&k) = required.get(&lno) {
            for _ in trailing_blanks..k {
                out.push('\n');
            }
        }
        out.push_str(content);
        out.push('\n');
        trailing_blanks = if is_blank[lno] {
            trailing_blanks + 1
        } else {
            0
        };
    }
    out
}

// ===================== line reflow (length-driven wrapping) =====================
//
// A statement that does not fit in `line_width` and contains a bracketed group is wrapped the way
// gdformat does: try the group **flat**; if it does not fit, **compact** (all elements on one
// indented continuation line, close bracket on its own line); if even that does not fit, **exploded**
// (one element per line, recursively). A **magic trailing comma** in the source forces a group
// exploded-with-comma even when it would fit (and forces every enclosing group multi-line) — the one
// case that mutates the token sequence (a trailing comma), guarded by the meaning-equivalence net.
// Only statements that occupy a single physical line are reflowed (so the pass is trivially
// idempotent — a wrapped statement spans bracket-continuation lines that are skipped on the next run).

/// One rendered token: its text and whether a single space precedes it in the flat form.
struct Atom {
    text: String,
    space: bool,
}

/// A reflow document node: a text run, or a bracketed group of comma-separated element sequences.
enum ReflowDoc {
    Text {
        text: String,
        space: bool,
    },
    Group {
        space: bool,
        open: String,
        elems: Vec<Vec<ReflowDoc>>,
        close: &'static str,
        /// The source had a **magic trailing comma** — gdformat forces this group exploded one per
        /// line *with* the trailing comma, even when it would fit.
        magic: bool,
    },
}

/// Whether a group must be broken across lines: it has a magic trailing comma, or a descendant does
/// (a magic comma anywhere forces every enclosing group multi-line, matching gdformat).
fn group_forced(elems: &[Vec<ReflowDoc>], magic: bool) -> bool {
    magic
        || elems.iter().any(|e| {
            e.iter().any(|d| match d {
                ReflowDoc::Group { elems, magic, .. } => group_forced(elems, *magic),
                ReflowDoc::Text { .. } => false,
            })
        })
}

fn open_close(text: &str) -> Option<&'static str> {
    match text {
        "(" => Some(")"),
        "[" => Some("]"),
        "{" => Some("}"),
        _ => None,
    }
}

/// Display columns of a content string (no tabs — those only appear in leading indentation).
fn cols(s: &str) -> usize {
    s.chars().count()
}

/// Build the comma-separated element sequences of a group whose contents start at `atoms[*i]`,
/// consuming through the matching `close` (or to the end when `close` is `None`, for the top level).
/// Returns the elements and whether the group ended with a magic trailing comma.
fn build_elems(atoms: &[Atom], i: &mut usize, close: Option<&str>) -> (Vec<Vec<ReflowDoc>>, bool) {
    let mut elems: Vec<Vec<ReflowDoc>> = Vec::new();
    let mut cur: Vec<ReflowDoc> = Vec::new();
    let mut elem_start = true;
    while *i < atoms.len() {
        let a = &atoms[*i];
        if close == Some(a.text.as_str()) {
            *i += 1;
            break;
        }
        if a.text == "," {
            elems.push(std::mem::take(&mut cur));
            elem_start = true;
            *i += 1;
            continue;
        }
        let space = !elem_start && a.space;
        if let Some(cl) = open_close(&a.text) {
            let open = a.text.clone();
            *i += 1;
            let (inner, magic) = build_elems(atoms, i, Some(cl));
            cur.push(ReflowDoc::Group {
                space,
                open,
                elems: inner,
                close: cl,
                magic,
            });
        } else {
            cur.push(ReflowDoc::Text {
                text: a.text.clone(),
                space,
            });
            *i += 1;
        }
        elem_start = false;
    }
    // A trailing comma leaves `elem_start` set with `cur` empty after at least one real element.
    let magic = elem_start && !elems.is_empty();
    if !cur.is_empty() {
        elems.push(cur);
    }
    (elems, magic)
}

fn flat_seq(docs: &[ReflowDoc]) -> String {
    let mut out = String::new();
    for d in docs {
        match d {
            ReflowDoc::Text { text, space } => {
                if *space {
                    out.push(' ');
                }
                out.push_str(text);
            }
            ReflowDoc::Group {
                space,
                open,
                elems,
                close,
                magic,
            } => {
                if *space {
                    out.push(' ');
                }
                out.push_str(open);
                out.push_str(&flat_group_contents(elems, *magic));
                out.push_str(close);
            }
        }
    }
    out
}

fn flat_group_contents(elems: &[Vec<ReflowDoc>], magic: bool) -> String {
    let mut s = elems
        .iter()
        .map(|e| flat_seq(e))
        .collect::<Vec<_>>()
        .join(", ");
    if magic && !elems.is_empty() {
        s.push(',');
    }
    s
}

/// Render a sequence (one element) at base `indent`, starting at column `col`, wrapping any group
/// that does not fit. Returns the text and the ending column.
fn render_seq(
    docs: &[ReflowDoc],
    indent: usize,
    mut col: usize,
    cfg: &FmtConfig,
    tw: usize,
    unit: &str,
) -> (String, usize) {
    let mut out = String::new();
    for d in docs {
        match d {
            ReflowDoc::Text { text, space } => {
                if *space {
                    out.push(' ');
                    col += 1;
                }
                out.push_str(text);
                col += cols(text);
            }
            ReflowDoc::Group {
                space,
                open,
                elems,
                close,
                magic,
            } => {
                if *space {
                    out.push(' ');
                    col += 1;
                }
                let (g, end) = render_group(open, elems, close, *magic, indent, col, cfg, tw, unit);
                out.push_str(&g);
                col = end;
            }
        }
    }
    (out, col)
}

#[allow(
    clippy::too_many_arguments,
    reason = "a focused internal renderer threading layout state"
)]
fn render_group(
    open: &str,
    elems: &[Vec<ReflowDoc>],
    close: &str,
    magic: bool,
    indent: usize,
    col: usize,
    cfg: &FmtConfig,
    tw: usize,
    unit: &str,
) -> (String, usize) {
    let forced = group_forced(elems, magic);
    // flat: the whole group on the current line (only when not forced multi-line).
    let flat = format!("{open}{}{close}", flat_group_contents(elems, magic));
    if !forced && (col + cols(&flat) <= cfg.line_width || elems.is_empty()) {
        let end = col + cols(&flat);
        return (flat, end);
    }
    let inner = indent + 1;
    let inner_col = inner * tw;
    let close_end = indent * tw + cols(close);
    // compact: all elements on one indented continuation line (not when forced exploded).
    if !forced {
        let contents = flat_group_contents(elems, magic);
        if inner_col + cols(&contents) <= cfg.line_width {
            let s = format!(
                "{open}\n{ind}{contents}\n{base}{close}",
                ind = unit.repeat(inner),
                base = unit.repeat(indent),
            );
            return (s, close_end);
        }
    }
    // exploded: one element per line (each rendered recursively). A comma follows every element
    // except the last — unless this group is magic, when it follows the last one too.
    let mut s = String::from(open);
    s.push('\n');
    for (k, elem) in elems.iter().enumerate() {
        s.push_str(&unit.repeat(inner));
        let (es, _) = render_seq(elem, inner, inner_col, cfg, tw, unit);
        s.push_str(&es);
        if magic || k + 1 < elems.len() {
            s.push(',');
        }
        s.push('\n');
    }
    s.push_str(&unit.repeat(indent));
    s.push_str(close);
    (s, close_end)
}

/// Parse a single physical line's content into reflow atoms, or `None` if it is not a clean,
/// reflowable single-line statement (has a comment, an error/continuation token, or unbalanced /
/// negative bracket nesting). A magic trailing comma is kept (it drives gdformat's exploded mode).
fn line_atoms(body: &str) -> Option<Vec<Atom>> {
    use SyntaxKind as S;
    let toks = gdscript_syntax::tokenize(body);
    let mut atoms: Vec<Atom> = Vec::new();
    let mut space = false;
    let mut depth: i32 = 0;
    for t in &toks {
        match t.kind {
            S::Whitespace => space = true,
            S::Bom => {}
            S::LineComment
            | S::DocComment
            | S::RegionComment
            | S::EndRegionComment
            | S::LineContinuation => return None,
            k => {
                if matches!(k, S::LParen | S::LBrack | S::LBrace) {
                    depth += 1;
                } else if matches!(k, S::RParen | S::RBrack | S::RBrace) {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                atoms.push(Atom {
                    text: body[t.range].to_string(),
                    space,
                });
                space = false;
            }
        }
    }
    if depth != 0 {
        return None;
    }
    Some(atoms)
}

/// Reflow over-long single-line statements. Token-preserving; safe to run last.
#[allow(
    clippy::too_many_lines,
    reason = "one cohesive pass: cross-line bracket/straddle bookkeeping then a per-line reflow decision"
)]
fn reflow(formatted: &str, config: &FmtConfig) -> String {
    use SyntaxKind as S;
    if config.line_width == 0 {
        return formatted.to_owned();
    }
    let tw = if config.use_tabs {
        4
    } else {
        config.indent_size.max(1)
    };
    let unit = config.indent_unit();
    let lines: Vec<&str> = formatted.split('\n').collect();

    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            formatted
                .bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        )
        .collect();
    let line_of = |off: usize| line_starts.partition_point(|&s| s <= off).saturating_sub(1);

    // Cross-line bracket depth at each line's start + lines covered by a multi-line token (e.g. a
    // `"""..."""` string) + lines ending in a `\` continuation — all disqualify a line from reflow.
    let raw = gdscript_syntax::tokenize(formatted);
    let mut start_depth = vec![i32::MIN; lines.len()];
    let mut straddled = vec![false; lines.len()];
    let mut ends_cont = vec![false; lines.len()];
    let mut depth: i32 = 0;
    for t in &raw {
        let s = usize::from(t.range.start());
        let e = usize::from(t.range.end()).saturating_sub(1).max(s);
        let (sl, el) = (line_of(s), line_of(e));
        if sl < start_depth.len() && start_depth[sl] == i32::MIN {
            start_depth[sl] = depth;
        }
        if el > sl {
            let end = el.min(lines.len().saturating_sub(1));
            straddled[sl..=end].fill(true);
        }
        match t.kind {
            S::LParen | S::LBrack | S::LBrace => depth += 1,
            S::RParen | S::RBrack | S::RBrace => depth -= 1,
            S::LineContinuation if sl < ends_cont.len() => ends_cont[sl] = true,
            _ => {}
        }
    }

    let mut out = String::with_capacity(formatted.len());
    for (li, &line) in lines.iter().enumerate() {
        let last = li + 1 == lines.len();
        if let Some(wrapped) = try_reflow_line(
            line,
            li,
            &start_depth,
            &straddled,
            &ends_cont,
            config,
            tw,
            &unit,
        ) {
            out.push_str(&wrapped);
        } else {
            out.push_str(line);
        }
        if !last {
            out.push('\n');
        }
    }
    out
}

#[allow(
    clippy::too_many_arguments,
    reason = "a focused single-call-site helper"
)]
fn try_reflow_line(
    line: &str,
    li: usize,
    start_depth: &[i32],
    straddled: &[bool],
    ends_cont: &[bool],
    config: &FmtConfig,
    tw: usize,
    unit: &str,
) -> Option<String> {
    if line.trim().is_empty()
        || start_depth.get(li).copied().unwrap_or(0) != 0
        || straddled.get(li).copied().unwrap_or(false)
        || ends_cont.get(li).copied().unwrap_or(false)
        || (li > 0 && ends_cont.get(li - 1).copied().unwrap_or(false))
    {
        return None;
    }
    let indent_chars = line.len() - line.trim_start_matches(['\t', ' ']).len();
    let body = &line[indent_chars..];
    let indent = if config.use_tabs {
        line.bytes().take_while(|&b| b == b'\t').count()
    } else {
        line.bytes()
            .take_while(|&b| b == b' ')
            .count()
            .checked_div(config.indent_size)
            .unwrap_or(0)
    };
    let atoms = line_atoms(body)?;
    if !atoms.iter().any(|a| open_close(&a.text).is_some()) {
        return None; // nothing to wrap
    }
    let (docs, _) = build_elems(&atoms, &mut 0, None);
    if docs.len() != 1 {
        return None; // a top-level comma — not a normal statement; leave it
    }
    let top = &docs[0];
    // Faithfulness check: only reflow when our flat reconstruction is byte-identical to the input.
    if flat_seq(top) != *body {
        return None;
    }
    // Reflow when the line is too long OR a magic trailing comma forces it exploded.
    let width = indent * tw + cols(body);
    let forced = top
        .iter()
        .any(|d| matches!(d, ReflowDoc::Group { elems, magic, .. } if group_forced(elems, *magic)));
    if width <= config.line_width && !forced {
        return None;
    }
    let (rendered, _) = render_seq(top, indent, indent * tw, config, tw, unit);
    let result = format!("{}{rendered}", unit.repeat(indent));
    (result != line).then_some(result)
}

/// Trim trailing spaces/tabs from the end of `out` (the current line).
fn trim_trailing_inline_ws(out: &mut String) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
}

/// Whether two sources lex to the same sequence of significant (non-trivia) tokens — the strict
/// token-preservation check, used by the token-*preserving* passes' tests.
#[cfg(test)]
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

/// Re-emit a string-literal token's text in gdformat's canonical quote style: prefer `"`, fall back
/// to `'` only when the body has more `"` than `'` (fewer escapes), keeping the prefix (`r`/`&`/`^`)
/// and the decoded value. Idempotent. Triple-quoted strings are left verbatim (rare; not normalized).
fn canonical_string(text: &str) -> String {
    let Some(qpos) = text.find(['"', '\'']) else {
        return text.to_owned(); // defensive: not actually a string literal
    };
    let prefix = &text[..qpos];
    let rest = &text[qpos..];
    let rb = rest.as_bytes();
    let quote = rb[0];
    // Triple-quoted (`'''`/`"""`): leave verbatim.
    if rb.len() >= 6 && rb[1] == quote && rb[2] == quote {
        return text.to_owned();
    }
    if rest.len() < 2 || rb[rest.len() - 1] != quote {
        return text.to_owned(); // unterminated / malformed: don't touch
    }
    let body = &rest[1..rest.len() - 1];
    // Raw strings (`r"..."`) cannot escape — only switch quotes if the body lacks the target.
    if prefix.contains('r') {
        let target = if !body.contains('"') {
            '"'
        } else if !body.contains('\'') {
            '\''
        } else {
            quote as char
        };
        return format!("{prefix}{target}{body}{target}");
    }
    // Parse the body into units (escaped or literal) and count the value's quote characters.
    let mut units: Vec<(bool, char)> = Vec::new();
    let (mut dq, mut sq) = (0usize, 0usize);
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                units.push((true, n));
                match n {
                    '"' => dq += 1,
                    '\'' => sq += 1,
                    _ => {}
                }
            } else {
                units.push((false, '\\'));
            }
        } else {
            units.push((false, c));
            match c {
                '"' => dq += 1,
                '\'' => sq += 1,
                _ => {}
            }
        }
    }
    let target = if dq > sq { '\'' } else { '"' };
    let mut out = String::with_capacity(text.len());
    out.push_str(prefix);
    out.push(target);
    for (esc, c) in units {
        if c == '"' || c == '\'' {
            if c == target {
                out.push('\\');
            }
            out.push(c);
        } else {
            if esc {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push(target);
    out
}

/// Whether `a` and `b` are **meaning-equivalent** — the relaxed safety net used once the formatter
/// performs token-*mutating* rewrites. Equivalent to [`same_significant_tokens`] except it normalises
/// away exactly the differences gdformat is allowed to introduce: a **trailing comma** (a `,`
/// immediately before a closing bracket) is dropped, and **string literals are compared by their
/// canonical quote form** (so `'x'` ≡ `"x"`). A dropped/added real token or a changed string *value*
/// is still caught.
fn meaning_preserved(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> Vec<(SyntaxKind, String)> {
        let toks: Vec<_> = gdscript_syntax::tokenize(s)
            .into_iter()
            .filter(|t| !t.kind.is_trivia())
            .collect();
        let mut out = Vec::with_capacity(toks.len());
        for (i, t) in toks.iter().enumerate() {
            if t.kind == SyntaxKind::Comma
                && toks.get(i + 1).is_some_and(|n| {
                    matches!(
                        n.kind,
                        SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace
                    )
                })
            {
                continue; // a trailing comma — gdformat may add or drop it
            }
            let text = &s[t.range];
            let norm = if matches!(
                t.kind,
                SyntaxKind::String | SyntaxKind::StringName | SyntaxKind::NodePath
            ) {
                canonical_string(text)
            } else {
                text.to_owned()
            };
            out.push((t.kind, norm));
        }
        out
    }
    norm(a) == norm(b)
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
    fn leading_body_comment_is_indented_to_the_block() {
        // A comment that is the FIRST line of a block: the prepass emits `Indent` only at the first
        // *code* line, so the comment's raw depth is wrong — but it is re-indented to its intended
        // block depth by comparing its authored indentation against the surrounding code lines
        // (gdformat's rule). Works for the space-indented input here too (length comparison is
        // indent-width agnostic).
        let src = "func g():\n  # c\n  var x = 1\n  var y = 2\n";
        let out = fmt(src);
        assert_eq!(out, "func g():\n\t# c\n\tvar x = 1\n\tvar y = 2\n");
        assert!(parses_clean(&out), "{out:?}");
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
    fn single_blank_between_top_defs_is_grown_to_two() {
        // gdformat enforces exactly 2 blank lines around top-level defs; a single blank is grown.
        let src = "func a():\n\tpass\n\nfunc b():\n\tpass\n";
        assert_eq!(fmt(src), "func a():\n\tpass\n\n\nfunc b():\n\tpass\n");
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

    // ---- Phase-4 increment C: block-boundary comment indentation ----

    #[test]
    fn trailing_body_comment_stays_at_block_depth() {
        // A comment after the last body statement, before a dedented def, stays at the body depth
        // (gdformat keeps it attached to the block it was written in, not the following def).
        let src = "func foo():\n\tpass\n\t# trailing\n\n\nfunc bar():\n\tpass\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func foo():\n\tpass\n\t# trailing\n\n\nfunc bar():\n\tpass\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn comment_after_nested_block_stays_at_outer_body_depth() {
        // The "after-if" case: a comment authored at body depth, sitting between a deeper block and
        // a following body statement, keeps the body depth (not the deeper raw prepass depth).
        let src = "func f():\n\tif x:\n\t\tpass\n\t# back at body level\n\treturn\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func f():\n\tif x:\n\t\tpass\n\t# back at body level\n\treturn\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn over_indented_comment_snaps_to_block_and_col0_stays() {
        // An over-indented comment snaps to the surrounding block; a column-0 comment stays at 0.
        assert_eq!(
            fmt("func f():\n\t\t\t# over\n\tpass\n"),
            "func f():\n\t# over\n\tpass\n"
        );
        assert_eq!(
            fmt("func f():\n# at col 0\n\tpass\n"),
            "func f():\n# at col 0\n\tpass\n"
        );
    }

    // ---- Phase-4 increment C: blank-line insertion around definitions ----

    #[test]
    fn two_blanks_inserted_around_top_level_defs() {
        let src = "extends Node\nfunc a():\n\tpass\nfunc b():\n\tpass\n";
        assert_eq!(
            fmt(src),
            "extends Node\n\n\nfunc a():\n\tpass\n\n\nfunc b():\n\tpass\n"
        );
    }

    #[test]
    fn one_blank_inserted_between_methods_in_a_class() {
        // Inside a class: 1 blank between methods; none before the first member (after the header).
        let src = "class C:\n\tfunc a():\n\t\tpass\n\tfunc b():\n\t\tpass\n";
        assert_eq!(
            fmt(src),
            "class C:\n\tfunc a():\n\t\tpass\n\n\tfunc b():\n\t\tpass\n"
        );
    }

    #[test]
    fn blanks_go_before_an_attached_comment_or_annotation_prefix() {
        // The 2 blanks land before the doc-comment/annotation that belongs to the def, not between.
        let src = "func a():\n\tpass\n## docs for b\n@warning_ignore(\"x\")\nfunc b():\n\tpass\n";
        assert_eq!(
            fmt(src),
            "func a():\n\tpass\n\n\n## docs for b\n@warning_ignore(\"x\")\nfunc b():\n\tpass\n"
        );
    }

    #[test]
    fn blanks_inserted_after_a_def_before_a_following_non_def() {
        // gdformat surrounds a def: a top-level statement after a func body gets 2 blanks too.
        let src = "func a():\n\tpass\nvar x = 1\n";
        assert_eq!(fmt(src), "func a():\n\tpass\n\n\nvar x = 1\n");
    }

    #[test]
    fn static_var_is_not_a_def_but_static_func_is() {
        // `static var` is an ordinary member (no surrounding blanks); `static func` is a def.
        let src = "var a = 1\nstatic var b = 2\nstatic func c():\n\tpass\n";
        assert_eq!(
            fmt(src),
            "var a = 1\nstatic var b = 2\n\n\nstatic func c():\n\tpass\n"
        );
    }

    #[test]
    fn no_blank_before_the_first_def_in_the_file() {
        let src = "func a():\n\tpass\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn blank_insertion_off_leaves_blanks_alone() {
        let cfg = FmtConfig {
            insert_blank_lines: false,
            ..FmtConfig::default()
        };
        let src = "func a():\n\tpass\nfunc b():\n\tpass\n";
        assert_eq!(format(src, &cfg), src);
    }

    // ---- Phase-4 increment C: line-ending preservation ----

    #[test]
    fn crlf_line_endings_are_preserved() {
        // A CRLF file is formatted (spacing + blank policy applied) but stays CRLF — never churned
        // to LF.
        let src = "func a():\r\n\tvar x=1\r\nfunc b():\r\n\tpass\r\n";
        assert_eq!(
            fmt(src),
            "func a():\r\n\tvar x = 1\r\n\r\n\r\nfunc b():\r\n\tpass\r\n"
        );
    }

    #[test]
    fn crlf_preserved_including_multiline_string_interior() {
        // A `\r\n` inside a multi-line string round-trips (normalize to LF, format, restore CRLF).
        let src = "var s = \"\"\"a\r\nb\"\"\"\r\n";
        let out = fmt(src);
        assert_eq!(out, "var s = \"\"\"a\r\nb\"\"\"\r\n");
        assert!(super::same_significant_tokens(src, &out));
    }

    #[test]
    fn lf_files_stay_lf() {
        let src = "func a():\n\tpass\n";
        assert!(!fmt(src).contains('\r'));
    }

    // ---- Phase-4 increment C: length-driven reflow ----

    #[test]
    fn reflow_compact_call() {
        let src = "func f():\n\tvar long_call = some_function(argument_one, argument_two, argument_three, argument_four, arg_five)\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tvar long_call = some_function(\n\t\targument_one, argument_two, argument_three, argument_four, arg_five\n\t)\n"
        );
        assert_eq!(fmt(&fmt(src)), fmt(src), "idempotent");
    }

    #[test]
    fn reflow_compact_array_and_dict() {
        let arr = "func f():\n\tvar arr = [element_one, element_two, element_three, element_four, element_five, element_six, seven]\n";
        assert_eq!(
            fmt(arr),
            "func f():\n\tvar arr = [\n\t\telement_one, element_two, element_three, element_four, element_five, element_six, seven\n\t]\n"
        );
        let dct = "func f():\n\tvar d = {\"key_one\": value_one, \"key_two\": value_two, \"key_three\": value_three, \"key4\": value_four}\n";
        assert_eq!(
            fmt(dct),
            "func f():\n\tvar d = {\n\t\t\"key_one\": value_one, \"key_two\": value_two, \"key_three\": value_three, \"key4\": value_four\n\t}\n"
        );
    }

    #[test]
    fn reflow_exploded_when_compact_too_long() {
        // When even the single compact continuation line exceeds the width, explode one per line —
        // with NO trailing comma (length-driven). Byte-identical to gdformat.
        let src = "func f():\n\tvar x = process_data(first_long_argument_name_here, second_long_argument_name_here, third_long_argument_name_here, fourth_argument)\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tvar x = process_data(\n\t\tfirst_long_argument_name_here,\n\t\tsecond_long_argument_name_here,\n\t\tthird_long_argument_name_here,\n\t\tfourth_argument\n\t)\n"
        );
        assert_eq!(fmt(&fmt(src)), fmt(src), "idempotent");
    }

    #[test]
    fn reflow_nested_outer_explodes_inner_stays_inline() {
        let src = "func f():\n\tvar n = outermost_call(inner_first(aaaa, bbbb, cccc, dddd), inner_second(eeee, ffff, gggg, hhhh), inner_third(iiii, jjjj, kkkk))\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tvar n = outermost_call(\n\t\tinner_first(aaaa, bbbb, cccc, dddd),\n\t\tinner_second(eeee, ffff, gggg, hhhh),\n\t\tinner_third(iiii, jjjj, kkkk)\n\t)\n"
        );
    }

    #[test]
    fn reflow_short_lines_stay_flat() {
        let src = "func f():\n\tvar short = call(a, b, c)\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn reflow_off_leaves_long_lines() {
        let cfg = FmtConfig {
            reflow: false,
            ..FmtConfig::default()
        };
        let src = "func f():\n\tvar long_call = some_function(argument_one, argument_two, argument_three, argument_four, arg_five)\n";
        // spacing still normalized, but no wrapping
        assert_eq!(format(src, &cfg), src);
    }

    #[test]
    fn reflow_preserves_already_wrapped_statements() {
        // A statement that is already wrapped (spans bracket-continuation lines) is left as-is — the
        // reflow only touches single-physical-line statements, which is what makes it idempotent.
        let src = "func f():\n\tvar n = outermost_call(\n\t\tinner_first(aaaa, bbbb, cccc, dddd),\n\t\tinner_second(eeee, ffff, gggg, hhhh),\n\t\tinner_third(iiii, jjjj, kkkk)\n\t)\n";
        assert_eq!(fmt(src), src);
    }

    // ---- Phase-4: string-quote normalization (gdformat / Black rule) ----

    #[test]
    fn canonical_string_rules() {
        use super::canonical_string as c;
        assert_eq!(c("'simple'"), "\"simple\""); // prefer double
        assert_eq!(c("\"already\""), "\"already\""); // unchanged
        assert_eq!(c("'has \"x\" in'"), "'has \"x\" in'"); // more " than ' -> keep single
        assert_eq!(c("'a\\'b'"), "\"a'b\""); // escaped ' -> double, unescaped
        assert_eq!(c("'both \" and \\' x'"), "\"both \\\" and ' x\""); // tie -> double, re-escape
        assert_eq!(c("&'name'"), "&\"name\""); // StringName prefix kept
        assert_eq!(c("^'a/b'"), "^\"a/b\""); // NodePath prefix kept
        assert_eq!(c("r'raw\\n'"), "r\"raw\\n\""); // raw: body verbatim
        assert_eq!(c("r'has \"x\"'"), "r'has \"x\"'"); // raw with " -> keep single (cannot escape)
        assert_eq!(c("'''triple'''"), "'''triple'''"); // triple left verbatim
        assert_eq!(c("'\\t\\n'"), "\"\\t\\n\""); // non-quote escapes preserved
    }

    #[test]
    fn quote_normalization_in_format() {
        assert_eq!(fmt("var a = 'simple'\n"), "var a = \"simple\"\n");
        assert_eq!(fmt("var b = 'has \"x\" in'\n"), "var b = 'has \"x\" in'\n");
        assert_eq!(fmt("var f = &'n'\n"), "var f = &\"n\"\n");
        // idempotent
        assert_eq!(fmt("var a = \"simple\"\n"), "var a = \"simple\"\n");
    }

    // ---- Phase-4: magic trailing comma ----

    #[test]
    fn magic_trailing_comma_explodes_with_comma() {
        // A magic trailing comma forces exploded-one-per-line WITH the comma, even though it fits.
        assert_eq!(
            fmt("var a = call(x, y,)\n"),
            "var a = call(\n\tx,\n\ty,\n)\n"
        );
        assert_eq!(fmt("var b = [1, 2,]\n"), "var b = [\n\t1,\n\t2,\n]\n");
        assert_eq!(fmt("var g = call(only,)\n"), "var g = call(\n\tonly,\n)\n");
    }

    #[test]
    fn magic_trailing_comma_nested_forces_outer_without_own_comma() {
        // The inner magic group explodes WITH its comma; the outer is forced multi-line by the
        // descendant but, not being magic itself, has no trailing comma after its last element.
        assert_eq!(
            fmt("var d = outer(inner(a, b,), c)\n"),
            "var d = outer(\n\tinner(\n\t\ta,\n\t\tb,\n\t),\n\tc\n)\n"
        );
    }

    #[test]
    fn magic_trailing_comma_is_idempotent() {
        let once = fmt("var a = call(x, y,)\n");
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn quote_normalization_off() {
        let cfg = FmtConfig {
            normalize_strings: false,
            ..FmtConfig::default()
        };
        assert_eq!(format("var a = 'simple'\n", &cfg), "var a = 'simple'\n");
    }

    #[test]
    fn meaning_preserved_accepts_quote_and_trailing_comma_changes() {
        assert!(super::meaning_preserved("var a = 'x'", "var a = \"x\""));
        assert!(super::meaning_preserved("f(a, b,)", "f(a, b)"));
        assert!(super::meaning_preserved("[1, 2,]", "[1, 2]"));
        // but a real change is still caught
        assert!(!super::meaning_preserved("var a = 'x'", "var a = \"y\""));
        assert!(!super::meaning_preserved("f(a, b)", "f(a)"));
    }
}
