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

mod wrap;

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
    /// Split an inline suite body onto its own indented line (`if c: x` → `if c:` / `x`,
    /// `func f(): return` → split, `a; b` → two lines), matching gdformat. An inline *lambda* body
    /// (`func(): x`) is preserved. On by default. Token-preserving (adds only newlines/indent).
    pub expand_inline_blocks: bool,
    /// Remove redundant grouping parens from a *standalone-expression* position — a var/const/return
    /// value, a `for` iterable, an `if`/`while` condition, a call argument, an array/dict element, a
    /// nested `(…)` — matching gdformat's `remove_outer_parentheses`. Precedence-significant parens
    /// (`(a + b) * c`) are kept. On by default. Token-mutating; guarded by the meaning-equivalence net.
    pub strip_parens: bool,
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
            expand_inline_blocks: true,
            strip_parens: true,
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
    // A leading byte-order mark is preserved (gdformat keeps it): strip it, format the rest, re-add it.
    // Otherwise the reflow — which re-emits each statement from its *significant* tokens — would drop
    // the BOM (a trivia token) when it re-renders the first line.
    if let Some(rest) = source.strip_prefix('\u{feff}') {
        return format!("\u{feff}{}", format(rest, config));
    }
    if source.contains("\r\n") {
        let lf = source.replace("\r\n", "\n");
        return format_lf(&lf, config).replace('\n', "\r\n");
    }
    format_lf(source, config)
}

/// A single whole-line replacement edit produced by [`format_range`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEdit {
    /// The byte range in the original `source` to replace (snapped to whole lines).
    pub range: core::ops::Range<usize>,
    /// The replacement text.
    pub new_text: String,
}

/// Format only the part of `source` overlapping the byte range `sel` (for editor "format selection"
/// / LSP `textDocument/rangeFormatting`). The whole document is formatted for correct structure and
/// indentation; the result is the **minimal changed line-hunk** that intersects `sel`, or `None` if
/// nothing in the selection's lines changes. Applying the edit yields the same bytes the whole-file
/// [`format`] would have produced for that region.
#[must_use]
pub fn format_range(
    source: &str,
    config: &FmtConfig,
    sel: core::ops::Range<usize>,
) -> Option<RangeEdit> {
    let formatted = format(source, config);
    if formatted == source {
        return None;
    }
    let src: Vec<&str> = source.split_inclusive('\n').collect();
    let out: Vec<&str> = formatted.split_inclusive('\n').collect();
    // Trim the common prefix and suffix lines: the change is the span between them.
    let mut p = 0;
    while p < src.len() && p < out.len() && src[p] == out[p] {
        p += 1;
    }
    let mut s = 0;
    while s < src.len() - p && s < out.len() - p && src[src.len() - 1 - s] == out[out.len() - 1 - s]
    {
        s += 1;
    }
    let (changed_start, changed_end) = (p, src.len() - s); // source line span [start, end)

    // Byte offset of each source line start (so `starts[i]` is line `i`'s first byte).
    let mut starts = Vec::with_capacity(src.len() + 1);
    let mut acc = 0;
    for l in &src {
        starts.push(acc);
        acc += l.len();
    }
    starts.push(acc);
    let line_of = |off: usize| starts.partition_point(|&b| b <= off).saturating_sub(1);

    let last_byte = source.len().saturating_sub(1);
    let sel_first = line_of(sel.start.min(last_byte));
    let sel_last = line_of(sel.end.saturating_sub(1).max(sel.start).min(last_byte));
    if changed_end <= sel_first || changed_start > sel_last {
        return None; // the selection's lines are unchanged
    }
    Some(RangeEdit {
        range: starts[changed_start]..starts[changed_end],
        new_text: out[p..out.len() - s].concat(),
    })
}

/// `format` working purely in LF (the caller handles CRLF round-tripping).
fn format_lf(source: &str, config: &FmtConfig) -> String {
    let input_parses = gdscript_syntax::parse(source).errors().is_empty();
    // Safe mode: never reformat around a syntax error — we'd risk mis-indenting a mis-parsed block.
    if config.safe_mode && !input_parses {
        return source.to_owned();
    }
    // Strip redundant grouping parens, then de-inline suite bodies, so the rest of the pipeline sees a
    // paren-clean, one-statement-per-line tree. Each pre-pass self-validates and is a no-op otherwise.
    let stripped = if config.strip_parens {
        strip_outer_parens(source)
    } else {
        None
    };
    let source = stripped.as_deref().unwrap_or(source);
    let expanded = if config.expand_inline_blocks {
        expand_inline_blocks(source, config)
    } else {
        None
    };
    let source = expanded.as_deref().unwrap_or(source);
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

/// gdformat moves an inline suite body to its own indented line (`if c: x` → two lines, `func f():
/// return` → split), while keeping an inline *lambda* body (`func(): x`). We do it as a source
/// pre-pass driven by the parse tree: find each block whose first statement shares its header's line
/// — and whose parent is a real statement/declaration, not a `LambdaExpr` — and insert a newline +
/// indentation before the body. Returns `None` (leave the source untouched) if it does not parse,
/// nothing is inline, or the rewrite would drop a significant token / fail to parse.
fn expand_inline_blocks(source: &str, config: &FmtConfig) -> Option<String> {
    let parse = gdscript_syntax::parse(source);
    if !parse.errors().is_empty() {
        return None;
    }
    let unit = config.indent_unit();
    // Each split is `(start, replace_len, text)`: `replace_len` bytes at `start` become `text`.
    let mut splits: Vec<(usize, usize, String)> = Vec::new();
    collect_inline_splits(&parse.syntax_node(), source, 0, &unit, &mut splits);
    if splits.is_empty() {
        return None;
    }
    splits.sort_by_key(|(off, ..)| std::cmp::Reverse(*off)); // apply right-to-left so offsets stay valid
    let mut out = source.to_owned();
    for (off, len, text) in &splits {
        out.replace_range(*off..*off + *len, text);
    }
    // The rewrite only adds layout (newlines/indent), so it must keep every significant token and the
    // structure intact — verified by a token-equality + parse-validity check.
    if !same_significant_tokens(source, &out) || !gdscript_syntax::parse(&out).errors().is_empty() {
        return None;
    }
    Some(out)
}

/// Collect `(offset, inserted_text)` for each inline suite body, recursing with `depth` = the
/// indentation level of `node`'s own line (incremented when descending into a block).
fn collect_inline_splits(
    node: &gdscript_syntax::GdNode,
    src: &str,
    depth: usize,
    unit: &str,
    splits: &mut Vec<(usize, usize, String)>,
) {
    use SyntaxKind as S;
    use cstree::util::NodeOrToken;
    for elem in node.children_with_tokens() {
        let child = match elem {
            NodeOrToken::Node(n) => n,
            NodeOrToken::Token(t) => {
                // A statement-level `;` separates two statements — replace it (and any following
                // spaces) with a newline + this block's indent so each lands on its own line; a
                // *trailing* `;` (nothing after it on the line) is dropped.
                if t.kind() == S::Semicolon {
                    let r = t.text_range();
                    let (s, e) = (usize::from(r.start()), usize::from(r.end()));
                    let trail = src[e..].len() - src[e..].trim_start_matches([' ', '\t']).len();
                    let rest = &src[e + trail..];
                    let text = if rest.starts_with('\n') || rest.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", unit.repeat(depth))
                    };
                    splits.push((s, e + trail - s, text));
                }
                continue;
            }
        };
        // A suite-introducing child: a statement/declaration `Block` (but never a `LambdaExpr`'s,
        // which stays inline) or a property body (`var x: T: set = f`). Its body splits onto its own
        // indented line when it shares the header's line.
        let suite =
            matches!(child.kind(), S::Block | S::PropertyBody) && node.kind() != S::LambdaExpr;
        if suite {
            // The body offset: a `Block`'s first token, or — for a property body, whose own first
            // token is the `:` — its first getter/setter node (the content after the `:`).
            let body_offset = if child.kind() == S::PropertyBody {
                child.children().next().and_then(first_sig_offset)
            } else {
                first_sig_offset(child)
            };
            let inline_body =
                body_offset.filter(|&bs| src[..bs].trim_end_matches([' ', '\t']).ends_with(':'));
            if let Some(bs) = inline_body {
                splits.push((bs, 0, format!("\n{}", unit.repeat(depth + 1))));
            }
        }
        // A child is one indent level deeper when it is a suite, or a `match` arm (arms sit a level
        // below the `match` with no intervening `Block` node).
        let deeper = suite || (node.kind() == S::MatchStmt && child.kind() == S::MatchArm);
        collect_inline_splits(child, src, depth + usize::from(deeper), unit, splits);
    }
}

/// Remove redundant grouping parens from standalone-expression positions, matching gdformat's
/// `remove_outer_parentheses`. A `ParenExpr` is redundant when its parent uses it as a *whole
/// expression* (a value / condition / iterable / argument / element / nested paren) rather than as an
/// *operand* of a larger expression — `(a + b) * c` keeps its parens because the parent is the `*`
/// `BinExpr`. Returns `None` (leave the source) if it does not parse, has no such parens, or the
/// result is not meaning-equivalent / does not parse (the net catches a precedence-changing strip).
fn strip_outer_parens(source: &str) -> Option<String> {
    let parse = gdscript_syntax::parse(source);
    if !parse.errors().is_empty() {
        return None;
    }
    let mut dels: Vec<usize> = Vec::new(); // byte offsets of `(`/`)` tokens to delete (1 byte each)
    collect_redundant_parens(&parse.syntax_node(), &mut dels);
    if dels.is_empty() {
        return None;
    }
    dels.sort_unstable_by(|a, b| b.cmp(a)); // delete right-to-left
    let mut out = source.to_owned();
    for off in &dels {
        out.replace_range(*off..=*off, "");
    }
    if !meaning_preserved(source, &out) || !gdscript_syntax::parse(&out).errors().is_empty() {
        return None;
    }
    Some(out)
}

/// Whether a `ParenExpr` child of a node of `parent` is a redundant grouping paren (a standalone
/// expression), not a precedence-bearing operand.
fn paren_parent_strips(parent: SyntaxKind) -> bool {
    use SyntaxKind as S;
    matches!(
        parent,
        S::VarDecl
            | S::ConstDecl
            | S::ReturnStmt
            | S::ForStmt
            | S::IfStmt
            | S::ElifClause
            | S::WhileStmt
            | S::MatchStmt
            | S::ExprStmt
            | S::ArgList
            | S::ArrayLit
            | S::DictEntry
            | S::ParenExpr
    )
}

/// Collect the byte offsets of the `(`/`)` tokens of every redundant `ParenExpr`.
fn collect_redundant_parens(node: &gdscript_syntax::GdNode, dels: &mut Vec<usize>) {
    use cstree::util::NodeOrToken;
    for child in node.children() {
        if child.kind() == SyntaxKind::ParenExpr && paren_parent_strips(node.kind()) {
            for c in child.children_with_tokens() {
                let NodeOrToken::Token(t) = c else { continue };
                if matches!(t.kind(), SyntaxKind::LParen | SyntaxKind::RParen) {
                    dels.push(usize::from(t.text_range().start()));
                }
            }
        }
        collect_redundant_parens(child, dels);
    }
}

/// The byte offset of the first significant (non-trivia, non-synthetic) token within `node`.
fn first_sig_offset(node: &gdscript_syntax::GdNode) -> Option<usize> {
    use cstree::util::NodeOrToken;
    for c in node.children_with_tokens() {
        match c {
            NodeOrToken::Token(t) => {
                let k = t.kind();
                if !k.is_trivia() && !k.is_synthetic_layout() {
                    return Some(usize::from(t.text_range().start()));
                }
            }
            NodeOrToken::Node(n) => {
                if let Some(o) = first_sig_offset(n) {
                    return Some(o);
                }
            }
        }
    }
    None
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
    top_enum: bool,
) -> Spacing {
    use SyntaxKind as S;
    // An enum body is spaced inside (`{ A, B }`) — unlike a dict (`{"k": v}`) — but an empty `{}`
    // stays tight. This overrides the bracket-hug rule below for the enum braces only.
    if top_enum {
        if prev == S::LBrace && cur != S::RBrace {
            return Spacing::Single;
        }
        if cur == S::RBrace && prev != S::LBrace {
            return Spacing::Single;
        }
    }
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
    // Parallel to the `{` entries in `stack`: whether each brace is an **enum body** (spaced inside,
    // `{ A, B }`) rather than a dict (tight, `{"k": v}`). `pending_enum` is armed by an `enum` keyword
    // and consumed by the brace it opens.
    let mut brace_is_enum: Vec<bool> = Vec::new();
    let mut pending_enum = false;
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
                pending_enum = false; // an `enum` keyword is consumed by its `{` within one line
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
                        if comment_len == 0 {
                            // gdformat keeps a column-0 comment (`line.startswith("#")`) at column 0 —
                            // e.g. file-spanning `#region`/`#endregion` markers — rather than indenting
                            // it to the enclosing block.
                            0
                        } else {
                            let (next_depth, next_len) = next_code_info(&toks, idx, depth);
                            comment_depth(
                                comment_len,
                                depth,
                                prev_code_indent_len,
                                next_depth,
                                next_len,
                            )
                        }
                    } else {
                        prev_code_indent_len = comment_len;
                        depth
                    };
                    // Flush the buffered blank lines. gdformat squeezes *every* run of blank lines to a
                    // single blank, then re-inserts the 2nd (top-level) / 1st (nested) blank only around
                    // definitions (done later by `insert_def_blanks`). So the cap here is 1 everywhere,
                    // and 0 before the first content (leading blanks gone).
                    if collapse_on {
                        let cap = usize::from(seen_content);
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
                    } else if matches!(
                        t.kind,
                        SyntaxKind::LineComment
                            | SyntaxKind::DocComment
                            | SyntaxKind::RegionComment
                            | SyntaxKind::EndRegionComment
                    ) {
                        // An inline (trailing) comment is offset by exactly two spaces (gdformat's
                        // `INLINE_COMMENT_OFFSET`), regardless of the original spacing.
                        out.push_str("  ");
                        node_path = false;
                    } else if t.kind.is_trivia() {
                        // A line-continuation / BOM: keep the original spacing before it.
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
                        let path_continue = node_path
                            && (matches!(
                                t.kind,
                                SyntaxKind::Ident | SyntaxKind::Slash | SyntaxKind::String
                            ) || (t.kind == SyntaxKind::Percent
                                && matches!(
                                    prev_sig,
                                    Some(SyntaxKind::Dollar | SyntaxKind::Slash)
                                )));
                        let spacing = if path_continue {
                            // A `%` in sigil position (`$%Unique`, `$A/%Unique`) continues the path; a
                            // `%` after an identifier is modulo and falls through to `space_before`.
                            Spacing::Verbatim
                        } else {
                            node_path = false; // any non-path token ends the run.
                            match prev_sig {
                                Some(p) => {
                                    let top_enum = stack.last() == Some(&SyntaxKind::LBrace)
                                        && brace_is_enum.last() == Some(&true);
                                    space_before(
                                        p,
                                        t.kind,
                                        stack.last().copied(),
                                        prev_unary,
                                        top_enum,
                                    )
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
                    SyntaxKind::LBrace => {
                        stack.push(t.kind);
                        brace_is_enum.push(pending_enum);
                        pending_enum = false;
                    }
                    SyntaxKind::LParen | SyntaxKind::LBrack => {
                        stack.push(t.kind);
                    }
                    SyntaxKind::RBrace => {
                        stack.pop();
                        brace_is_enum.pop();
                    }
                    SyntaxKind::RParen | SyntaxKind::RBrack => {
                        stack.pop();
                    }
                    _ => {}
                }
                // Spacing state — significant tokens only (a comment is never an operand).
                if !t.kind.is_trivia() {
                    if t.kind == SyntaxKind::EnumKw {
                        pending_enum = true; // the next `{` opens an enum body
                    }
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
                    // A soft keyword (`match`/`when`) used as a *member* (`obj.match`) is an ordinary
                    // identifier operand — record it as `Ident` so a following `(` hugs it
                    // (`obj.match(x)`), while a leading `match (x):` statement keeps its space.
                    let member_soft_kw = matches!(t.kind, SyntaxKind::MatchKw | SyntaxKind::WhenKw)
                        && prev_sig == Some(SyntaxKind::Dot);
                    prev_sig = Some(if member_soft_kw {
                        SyntaxKind::Ident
                    } else {
                        t.kind
                    });
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
    /// A *standalone* comment (not a prefix that attaches to a following def/statement). gdformat does
    /// not force blank lines around such a comment (e.g. a trailing `#endregion`), and it is
    /// transparent to the surrounding defs' blank-line spacing.
    is_comment: bool,
    /// The unit is a definition reached through an **annotation** prefix (`@rpc func …`). gdformat
    /// forces blanks before such a unit only when the *previous* sibling was itself a def — not on the
    /// unit's own def-ness (the annotation line, not the def, owns the leading blanks, and an
    /// annotation is absent from the surrounding-empty-lines table). A *comment*-prefixed def forces
    /// like a plain def.
    ann_prefixed: bool,
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
                    is_comment: false,
                    ann_prefixed: h.role == LineRole::Annotation,
                });
                i = j + 1;
            } else {
                // Standalone comment/annotation lines (each its own non-def unit).
                for h in &heads[i..j] {
                    units.push(Unit {
                        head_line: h.line,
                        depth: h.depth,
                        is_def: false,
                        is_comment: h.role == LineRole::Comment,
                        ann_prefixed: false,
                    });
                }
                i = j;
            }
        } else {
            units.push(Unit {
                head_line: h.line,
                depth: h.depth,
                is_def: h.role == LineRole::Def,
                is_comment: false,
                ann_prefixed: false,
            });
            i += 1;
        }
    }

    // --- pass 3: the edge rule — N blanks before a unit whose own or previous sibling is a def ---
    let mut required: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut last_is_def: Vec<Option<bool>> = Vec::new();
    for (k, u) in units.iter().enumerate() {
        while last_is_def.len() <= u.depth {
            last_is_def.push(None);
        }
        let prev = last_is_def[u.depth];
        let first_in_block = prev.is_none();
        let n = if u.depth == 0 { 2 } else { 1 };
        // A *trailing* standalone comment — one that ends its block (no following unit at its depth or
        // shallower-after) such as a closing `#endregion` — keeps its source blanks unforced, matching
        // gdformat's end-of-block reconstruction. A standalone comment *between* statements still
        // resets def-adjacency (a following statement's spacing is measured from it, not the def above).
        let trailing_comment = u.is_comment && units.get(k + 1).is_none_or(|nx| nx.depth < u.depth);
        // An annotation-prefixed def forces blanks only after a previous def; a plain or
        // comment-prefixed def also forces on its own def-ness.
        let needs = if u.ann_prefixed {
            prev == Some(true)
        } else {
            u.is_def || prev == Some(true)
        };
        if !trailing_comment && !first_in_block && needs {
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

/// One rendered token: its kind, text, and whether a single space precedes it in the flat form.
struct Atom {
    kind: SyntaxKind,
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

/// Display width of a full line, counting a leading tab as `tw` columns.
fn display_cols(line: &str, tw: usize) -> usize {
    line.chars().map(|c| if c == '\t' { tw } else { 1 }).sum()
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
    // Flat width of each doc, so a group knows the width of the content that *follows* it on the line
    // (its "tail") — a group must wrap when the group + its tail overflow, even if the group alone fits.
    let widths: Vec<usize> = docs
        .iter()
        .map(|d| cols(&flat_seq(std::slice::from_ref(d))))
        .collect();
    let total: usize = widths.iter().sum();
    // gdformat wraps the *outermost* (last) bracket group — except a `func`/`static func` definition,
    // whose **parameter list** (the first group) is the one that wraps, not a `-> Array[T]` return
    // type. An earlier group otherwise stays flat unless it is itself forced (a magic comma).
    let first_text = match docs.first() {
        Some(ReflowDoc::Text { text, .. }) => Some(text.as_str()),
        _ => None,
    };
    let is_def = first_text == Some("func")
        || (first_text == Some("static")
            && matches!(docs.get(1), Some(ReflowDoc::Text { text, .. }) if text == "func"));
    let wrap_target = if is_def {
        docs.iter()
            .position(|d| matches!(d, ReflowDoc::Group { .. }))
    } else {
        docs.iter()
            .rposition(|d| matches!(d, ReflowDoc::Group { .. }))
    };
    let mut before = 0usize;
    for (i, d) in docs.iter().enumerate() {
        let tail = total - before - widths[i];
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
                let (g, end) = if Some(i) == wrap_target || group_forced(elems, *magic) {
                    render_group(open, elems, close, *magic, indent, col, tail, cfg, tw, unit)
                } else {
                    let flat = format!("{open}{}{close}", flat_group_contents(elems, *magic));
                    let end = col + cols(&flat);
                    (flat, end)
                };
                out.push_str(&g);
                col = end;
            }
        }
        before += widths[i];
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
    tail: usize,
    cfg: &FmtConfig,
    tw: usize,
    unit: &str,
) -> (String, usize) {
    let forced = group_forced(elems, magic);
    // flat: the whole group on the current line (only when it — plus the content that follows it on
    // the line — fits, and it is not forced multi-line).
    let flat = format!("{open}{}{close}", flat_group_contents(elems, magic));
    if !forced && (col + cols(&flat) + tail <= cfg.line_width || elems.is_empty()) {
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
                    kind: k,
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

/// Binary-operator precedence (lower binds looser → breaks first); `None` for non-binary tokens.
/// Mirrors the parser's `infix_prec`.
pub(crate) fn infix_prec(kind: SyntaxKind) -> Option<u8> {
    use SyntaxKind as S;
    Some(match kind {
        S::OrKw | S::PipePipe => 4,
        S::AndKw | S::AmpAmp => 5,
        S::InKw => 7,
        S::EqEq | S::Neq | S::Lt | S::Gt | S::Le | S::Ge => 8,
        S::Pipe => 9,
        S::Caret => 10,
        S::Amp => 11,
        S::Shl | S::Shr => 12,
        S::Plus | S::Minus => 13,
        S::Star | S::Slash | S::Percent => 14,
        S::StarStar => 17,
        _ => return None,
    })
}

fn is_assign_op(kind: SyntaxKind) -> bool {
    use SyntaxKind as S;
    matches!(
        kind,
        S::Eq
            | S::PlusEq
            | S::MinusEq
            | S::StarEq
            | S::SlashEq
            | S::StarStarEq
            | S::PercentEq
            | S::AmpEq
            | S::PipeEq
            | S::CaretEq
            | S::ShlEq
            | S::ShrEq
            | S::ColonEq
    )
}

/// Flat-render a run of atoms (the first atom never gets a leading space).
fn flat_atoms(atoms: &[Atom]) -> String {
    let mut s = String::new();
    for (i, a) in atoms.iter().enumerate() {
        if i > 0 && a.space {
            s.push(' ');
        }
        s.push_str(&a.text);
    }
    s
}

/// The `[prefix_end, suffix_start)` boundaries of the wrappable expression inside a statement's
/// atoms: the condition of `if`/`elif`/`while` (excluding the trailing `:`), the value of `return`,
/// or the right-hand side of a top-level assignment. `None` if there is no such expression.
fn expression_span(atoms: &[Atom]) -> Option<(usize, usize)> {
    use SyntaxKind as S;
    let first = atoms.first()?;
    match first.kind {
        S::IfKw | S::ElifKw | S::WhileKw => {
            let suffix = if atoms.last()?.kind == S::Colon {
                atoms.len() - 1
            } else {
                return None;
            };
            Some((1, suffix))
        }
        S::ReturnKw => Some((1, atoms.len())),
        _ => {
            let mut depth = 0i32;
            for (i, a) in atoms.iter().enumerate() {
                match a.kind {
                    S::LParen | S::LBrack | S::LBrace => depth += 1,
                    S::RParen | S::RBrack | S::RBrace => depth -= 1,
                    k if depth == 0 && is_assign_op(k) => return Some((i + 1, atoms.len())),
                    _ => {}
                }
            }
            None
        }
    }
}

/// Strip a redundant grouping paren that wraps the *entire* expression (e.g. the `(...)` this very
/// pass injected on a previous run) so re-flowing is idempotent. `(a and b)` → `a and b`, but
/// `(a + b) * c` is left alone (the paren is not the whole expression).
fn strip_redundant_parens(mut expr: &[Atom]) -> &[Atom] {
    while expr.len() >= 2 && expr[0].text == "(" {
        let mut depth = 0i32;
        let mut close = None;
        for (i, a) in expr.iter().enumerate() {
            if open_close(&a.text).is_some() {
                depth += 1;
            } else if matches!(a.text.as_str(), ")" | "]" | "}") {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
        }
        if close == Some(expr.len() - 1) {
            expr = &expr[1..expr.len() - 1];
        } else {
            break;
        }
    }
    expr
}

/// The indices (within `expr`) of top-level **binary** operators, paired with their precedence. A
/// `+`/`-` etc. is binary only when it follows an operand (not at a unary position); and a `/`/`%`
/// inside a **node-path run** (`$Node/Path`, `%Unique/Child`) is a path separator, not an operator —
/// the parser reads `$A / B` as the same node-path as `$A/B`, so such a chain must never be split.
fn top_level_binary_ops(expr: &[Atom]) -> Vec<(usize, u8)> {
    use SyntaxKind as S;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut node_path = false;
    let mut prev_operand_end = false;
    for (i, a) in expr.iter().enumerate() {
        match a.kind {
            S::LParen | S::LBrack | S::LBrace => depth += 1,
            S::RParen | S::RBrack | S::RBrace => depth -= 1,
            _ => {}
        }
        let in_path = node_path && matches!(a.kind, S::Ident | S::Slash | S::String);
        let binary_here = depth == 0 && !in_path && i > 0 && prev_operand_end;
        if let Some(p) = infix_prec(a.kind).filter(|_| binary_here) {
            out.push((i, p));
        }
        if node_path && !matches!(a.kind, S::Ident | S::Slash | S::String) {
            node_path = false;
        }
        if a.kind == S::Dollar || (a.kind == S::Percent && !prev_operand_end) {
            node_path = true;
        }
        prev_operand_end = is_operand_end(a.kind);
    }
    out
}

/// Wrap a too-long statement whose wrappable expression is a top-level **binary-operator chain** the
/// way gdformat does: inject parens and break at the lowest-precedence top-level operator,
/// operator-leading (each operand rendered recursively, so its own brackets reflow). `None` when
/// there is no wrappable binary-operator expression.
fn operator_chain_wrap(
    atoms: &[Atom],
    indent: usize,
    cfg: &FmtConfig,
    tw: usize,
    unit: &str,
) -> Option<String> {
    let (pre_end, suf_start) = expression_span(atoms)?;
    if pre_end >= suf_start {
        return None;
    }
    let expr = strip_redundant_parens(&atoms[pre_end..suf_start]);
    let ops = top_level_binary_ops(expr);
    let min_prec = ops.iter().map(|&(_, p)| p).min()?;
    let prefix = flat_atoms(&atoms[..pre_end]);
    let suffix = flat_atoms(&atoms[suf_start..]);
    let lead = |out: &mut String| {
        out.push_str(&unit.repeat(indent));
        out.push_str(&prefix);
        if !prefix.is_empty() {
            out.push(' ');
        }
        out.push_str("(\n");
    };
    // Compact first: the whole expression on one indented continuation line (gdformat tries this
    // before breaking — e.g. `(...).normalized() * power` that fits stays on one line).
    let expr_flat = flat_atoms(expr);
    if (indent + 1) * tw + cols(&expr_flat) <= cfg.line_width {
        let mut out = String::new();
        lead(&mut out);
        out.push_str(&unit.repeat(indent + 1));
        out.push_str(&expr_flat);
        out.push('\n');
        out.push_str(&unit.repeat(indent));
        out.push(')');
        out.push_str(&suffix);
        return Some(out);
    }

    let split: Vec<usize> = ops
        .iter()
        .filter(|&&(_, p)| p == min_prec)
        .map(|&(i, _)| i)
        .collect();

    // Build (leading-operator, operand-atoms) segments; the operator leads its following operand.
    let mut segs: Vec<(Option<&str>, &[Atom])> = Vec::new();
    let mut start = 0;
    let mut prev_op: Option<&str> = None;
    for &oi in &split {
        segs.push((prev_op, &expr[start..oi]));
        prev_op = Some(expr[oi].text.as_str());
        start = oi + 1;
    }
    segs.push((prev_op, &expr[start..]));

    let mut out = String::new();
    lead(&mut out);
    for (op, operand) in &segs {
        out.push_str(&unit.repeat(indent + 1));
        let mut col = (indent + 1) * tw;
        if let Some(op) = op {
            out.push_str(op);
            out.push(' ');
            col += cols(op) + 1;
        }
        let (od, _) = build_elems(operand, &mut 0, None);
        if let Some(seq) = od.first() {
            let (r, _) = render_seq(seq, indent + 1, col, cfg, tw, unit);
            out.push_str(&r);
        }
        out.push('\n');
    }
    out.push_str(&unit.repeat(indent));
    out.push(')');
    out.push_str(&suffix);
    Some(out)
}

/// Wrap the statement's expression in injected parens on a single indented continuation line (the
/// **compact** form gdformat uses for a too-long expression with no top-level binary operator — e.g.
/// a long method chain). `None` if there is no such expression, it has a top-level operator (handled
/// by [`operator_chain_wrap`]), or even the compact line would still overflow.
fn compact_paren_wrap(
    atoms: &[Atom],
    indent: usize,
    cfg: &FmtConfig,
    tw: usize,
    unit: &str,
) -> Option<String> {
    let (pre_end, suf_start) = expression_span(atoms)?;
    let expr = strip_redundant_parens(atoms.get(pre_end..suf_start)?);
    // Only wrap an expression that has a bracketed group (e.g. a method chain) and no top-level
    // binary operator: a bare expression / node-path is left on one line, as gdformat does.
    if expr.is_empty()
        || !top_level_binary_ops(expr).is_empty()
        || !expr.iter().any(|a| open_close(&a.text).is_some())
    {
        return None;
    }
    let expr_flat = flat_atoms(expr);
    if (indent + 1) * tw + cols(&expr_flat) > cfg.line_width {
        return None;
    }
    let prefix = flat_atoms(&atoms[..pre_end]);
    let suffix = flat_atoms(&atoms[suf_start..]);
    let mut out = String::new();
    out.push_str(&unit.repeat(indent));
    out.push_str(&prefix);
    if !prefix.is_empty() {
        out.push(' ');
    }
    out.push_str("(\n");
    out.push_str(&unit.repeat(indent + 1));
    out.push_str(&expr_flat);
    out.push('\n');
    out.push_str(&unit.repeat(indent));
    out.push(')');
    out.push_str(&suffix);
    Some(out)
}

/// Collapse a (possibly multi-line) statement to its canonical single-line body — re-synthesising the
/// inter-token spacing from scratch (so an already-wrapped statement is flattened). Returns `None`
/// when the statement must be kept **verbatim**: it contains a comment, a multi-line lambda body, a
/// backslash continuation, or a multi-line string (whose own newlines must be preserved).
fn flatten_statement(stmt: &str) -> Option<String> {
    use SyntaxKind as S;
    let raw = gdscript_syntax::tokenize(stmt);
    let (toks, _diags) = gdscript_syntax::run_prepass(&raw, stmt);
    let mut out = String::with_capacity(stmt.len());
    let mut stack: Vec<S> = Vec::new();
    let mut brace_enum: Vec<bool> = Vec::new();
    let mut pending_enum = false;
    let mut prev_sig: Option<S> = None;
    let mut prev_unary = false;
    let mut node_path = false;
    let mut pending_break = false; // a line break was seen; the next token re-spaces across it
    for t in &toks {
        match t.kind {
            S::NewlinePhys => pending_break = true,
            // A synthetic newline inside brackets is a multi-line lambda body — cannot collapse.
            S::Newline if !stack.is_empty() => return None,
            S::Indent | S::Dedent | S::Bom | S::Newline => {}
            // A backslash line continuation is collapsed away: trim the space before the `\` so the
            // join is not double-spaced; the following physical newline then sets `pending_break` and
            // the next token re-spaces once (and the statement is later re-wrapped) like gdformat,
            // which converts `\`-continued statements to paren-wrapped multi-line form.
            S::LineContinuation => trim_trailing_inline_ws(&mut out),
            S::Whitespace => {
                // Keep intra-line spacing verbatim (it is already canonical / the user's, when
                // spacing-normalisation is off); drop a line's leading indentation.
                if !pending_break && prev_sig.is_some() {
                    out.push_str(&stmt[t.range]);
                }
            }
            S::LineComment | S::DocComment | S::RegionComment | S::EndRegionComment => return None,
            k => {
                let text = &stmt[t.range];
                if matches!(k, S::String | S::StringName | S::NodePath) && text.contains('\n') {
                    return None; // a multi-line string — keep the statement verbatim
                }
                // Replace a line break by the spacing the intra-line rules would synthesize.
                if pending_break {
                    let sp = match prev_sig {
                        Some(_) if node_path && matches!(k, S::Ident | S::Slash | S::String) => {
                            Spacing::Verbatim
                        }
                        Some(p) => {
                            let top_enum = stack.last() == Some(&S::LBrace)
                                && brace_enum.last() == Some(&true);
                            space_before(p, k, stack.last().copied(), prev_unary, top_enum)
                        }
                        None => Spacing::None,
                    };
                    if matches!(sp, Spacing::Single) {
                        out.push(' ');
                    }
                    pending_break = false;
                }
                out.push_str(text);
                match k {
                    S::LBrace => {
                        stack.push(k);
                        brace_enum.push(pending_enum);
                        pending_enum = false;
                    }
                    S::LParen | S::LBrack => stack.push(k),
                    S::RBrace => {
                        stack.pop();
                        brace_enum.pop();
                    }
                    S::RParen | S::RBrack => {
                        stack.pop();
                    }
                    _ => {}
                }
                if k == S::EnumKw {
                    pending_enum = true;
                }
                let unary_ctx = prev_sig.is_none_or(|p| !is_operand_end(p));
                if k == S::Dollar || (k == S::Percent && unary_ctx) {
                    node_path = true;
                } else if node_path && !matches!(k, S::Ident | S::Slash | S::String) {
                    node_path = false;
                }
                prev_unary = match k {
                    S::Minus | S::Plus => unary_ctx,
                    S::Tilde | S::Bang => true,
                    _ => false,
                };
                prev_sig = Some(k);
            }
        }
    }
    Some(out)
}

/// Render a flattened statement `body` at `indent` into its canonical layout — flat if it fits, else
/// wrapped (operator-chain → bracketed compact/exploded → compact-paren). Always indented.
fn render_statement(body: &str, indent: usize, cfg: &FmtConfig, tw: usize, unit: &str) -> String {
    // Primary path: gdformat's own algorithm, driven from the CST (see `wrap`). It owns the layout and
    // is faithful to gdformat. `wrap::render` self-validates that its output is meaning-equivalent to
    // the body (allowing exactly gdformat's legitimate rewrites — redundant grouping parens, trailing
    // commas, string quotes); it returns `None` otherwise, and we fall back to the heuristic below.
    if let Some(out) = wrap::render(body, indent, cfg) {
        return out;
    }
    let flat = || format!("{}{body}", unit.repeat(indent));
    let Some(atoms) = line_atoms(body) else {
        return flat();
    };
    let width = indent * tw + cols(body);
    // A too-long top-level binary-operator chain is wrapped in injected parens (highest priority).
    if width > cfg.line_width {
        let oc = operator_chain_wrap(&atoms, indent, cfg, tw, unit);
        if let Some(oc) = oc {
            return oc;
        }
    }
    if !atoms.iter().any(|a| open_close(&a.text).is_some()) {
        return flat();
    }
    let (docs, _) = build_elems(&atoms, &mut 0, None);
    if docs.len() != 1 {
        return flat();
    }
    let top = &docs[0];
    // Faithfulness: the atom tree must round-trip the body's *tokens* (spacing may differ — e.g. enum
    // braces / type-annotation colons — which `flat_seq` does not reproduce).
    if !same_significant_tokens(&flat_seq(top), body) {
        return flat();
    }
    // Stay flat unless the line is too long or a magic trailing comma forces it exploded.
    let forced = top
        .iter()
        .any(|d| matches!(d, ReflowDoc::Group { elems, magic, .. } if group_forced(elems, *magic)));
    if width <= cfg.line_width && !forced {
        return flat();
    }
    let (rendered, _) = render_seq(top, indent, indent * tw, cfg, tw, unit);
    let result = format!("{}{rendered}", unit.repeat(indent));
    // If bracket reflow still overflows (e.g. a long method chain), wrap the expression compact.
    let overflows = result.lines().any(|l| display_cols(l, tw) > cfg.line_width);
    if overflows {
        let cp = compact_paren_wrap(&atoms, indent, cfg, tw, unit);
        if let Some(cp) = cp {
            return cp;
        }
    }
    result
}

/// Re-flow the layout of every statement: a statement that now fits is collapsed onto one line, one
/// that does not is re-wrapped to its canonical form — gdformat-style layout ownership. Statements
/// that cannot be safely collapsed (comments, multi-line lambdas, multi-line strings) are preserved.
/// Token-preserving (modulo the trailing commas / parens the meaning-equivalence net allows).
#[allow(
    clippy::too_many_lines,
    reason = "one cohesive pass: per-line bracket/straddle bookkeeping then logical-statement grouping"
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

    // Per line: bracket depth at its start, whether a multi-line token (e.g. a `"""..."""` string)
    // covers it, and whether it ends in a `\` continuation — used to group physical lines into the
    // logical statement they belong to.
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
            // The next line starts at the current depth — covers blank lines (which have no token of
            // their own to set `start_depth`, so they would otherwise stay unset and be absorbed).
            S::NewlinePhys if sl + 1 < start_depth.len() && start_depth[sl + 1] == i32::MIN => {
                start_depth[sl + 1] = depth;
            }
            _ => {}
        }
    }
    let sd = |i: usize| {
        let d = start_depth.get(i).copied().unwrap_or(0);
        if d == i32::MIN { 0 } else { d }
    };
    let strad = |i: usize| straddled.get(i).copied().unwrap_or(false);
    let cont = |i: usize| ends_cont.get(i).copied().unwrap_or(false);

    let mut out = String::with_capacity(formatted.len());
    let mut li = 0;
    while li < lines.len() {
        let line = lines[li];
        // A statement head: a non-blank line at bracket depth 0 that is not the continuation of the
        // previous line (a `\` or a multi-line token spanning the boundary).
        let head = !line.trim().is_empty()
            && sd(li) == 0
            && (li == 0 || !cont(li - 1))
            && !(li > 0 && strad(li) && strad(li - 1));
        if !head {
            out.push_str(line);
            if li + 1 < lines.len() {
                out.push('\n');
            }
            li += 1;
            continue;
        }
        // Extend to the end of the logical statement (brackets open, a multi-line token spanning the
        // boundary, or a backslash continuation).
        let mut j = li;
        while j + 1 < lines.len() && (sd(j + 1) != 0 || (strad(j) && strad(j + 1)) || cont(j)) {
            j += 1;
        }
        let stmt = lines[li..=j].join("\n");
        let indent = if config.use_tabs {
            line.bytes().take_while(|&b| b == b'\t').count()
        } else {
            line.bytes()
                .take_while(|&b| b == b' ')
                .count()
                .checked_div(config.indent_size)
                .unwrap_or(0)
        };
        let rendered =
            flatten_statement(&stmt).map(|body| render_statement(&body, indent, config, tw, &unit));
        out.push_str(rendered.as_deref().unwrap_or(&stmt));
        if j + 1 < lines.len() {
            out.push('\n');
        }
        li = j + 1;
    }
    out
}

/// Trim trailing spaces/tabs from the end of `out` (the current line).
fn trim_trailing_inline_ws(out: &mut String) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
}

/// Whether two sources lex to the same sequence of significant (non-trivia) tokens — a
/// spacing-insensitive equality used by the reflow faithfulness check and the token-preserving tests.
fn same_significant_tokens(a: &str, b: &str) -> bool {
    fn sig(s: &str) -> Vec<(SyntaxKind, &str)> {
        gdscript_syntax::tokenize(s)
            .into_iter()
            // A `;` is a statement separator equivalent to a newline — ignore it, so splitting
            // `a; b` onto two lines is recognised as token-preserving.
            .filter(|t| !t.kind.is_trivia() && t.kind != SyntaxKind::Semicolon)
            .map(|t| (t.kind, &s[t.range]))
            .collect()
    }
    sig(a) == sig(b)
}

/// Re-emit a string-literal token's text in gdformat's canonical quote style: prefer `"`, fall back
/// to `'` only when the body has more `"` than `'` (fewer escapes), keeping the prefix (`r`/`&`/`^`)
/// and the decoded value. Idempotent. Triple-quoted strings are left verbatim (rare; not normalized).
pub(crate) fn canonical_string(text: &str) -> String {
    let Some(qpos) = text.find(['"', '\'']) else {
        return text.to_owned(); // defensive: not actually a string literal
    };
    let prefix = &text[..qpos];
    let rest = &text[qpos..];
    let rb = rest.as_bytes();
    let quote = rb[0];
    // Triple-quoted. gdformat converts a *single-line* triple-**single**-quoted string (`'''…'''`)
    // to a regular string (strip the outer `''` and apply the regular quote rule to `'…'`); a
    // triple-**double** (`"""…"""`) and any *multi-line* triple-quoted string are left verbatim.
    if rb.len() >= 6 && rb[1] == quote && rb[2] == quote {
        if quote == b'\'' && rest.ends_with("'''") {
            let body = &rest[3..rest.len() - 3];
            if !body.contains('\n') {
                return canonical_string(&format!("{prefix}'{body}'"));
            }
        }
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

/// A normalised parse-tree event used by [`meaning_preserved`].
#[derive(Clone, PartialEq, Eq)]
enum TreeEvent {
    Open(SyntaxKind),
    Close,
    Token(SyntaxKind, String),
}

/// Walk a parse-tree node, appending normalised events: trivia is dropped, a `ParenExpr` is
/// **unwrapped** (its node + `(`/`)` tokens removed, its inner expression spliced in — so a
/// *redundant* grouping paren is invisible while a precedence-changing one still differs, because the
/// surrounding `BinExpr` nesting changes), and string literals are recorded by canonical quote form.
fn emit_tree_events(node: &gdscript_syntax::GdNode, out: &mut Vec<TreeEvent>) {
    use cstree::util::NodeOrToken;
    if node.kind() == SyntaxKind::ParenExpr {
        for child in node.children() {
            emit_tree_events(child, out);
        }
        return;
    }
    out.push(TreeEvent::Open(node.kind()));
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(n) => emit_tree_events(n, out),
            NodeOrToken::Token(t) => {
                let kind = t.kind();
                // A `;` is a statement separator (equivalent to a newline) — dropping it lets the
                // formatter split `a; b` onto two lines without the net seeing a meaning change.
                if kind.is_trivia() || kind == SyntaxKind::Semicolon {
                    continue;
                }
                let text = if matches!(
                    kind,
                    SyntaxKind::String | SyntaxKind::StringName | SyntaxKind::NodePath
                ) {
                    canonical_string(t.text())
                } else {
                    t.text().to_owned()
                };
                out.push(TreeEvent::Token(kind, text));
            }
        }
    }
    out.push(TreeEvent::Close);
}

/// Whether `a` and `b` are **meaning-equivalent** — the relaxed safety net used once the formatter
/// performs token-*mutating* rewrites. It compares the **parse-tree structure** (so token order,
/// nesting and operator precedence must all match), normalising away exactly the differences gdformat
/// is allowed to introduce: **redundant grouping parens** are unwrapped, a **trailing comma** before a
/// closing bracket is dropped, and **string literals are compared by canonical quote form**. A
/// dropped/added/reordered token, a changed string *value*, or a precedence change is still caught.
pub(crate) fn meaning_preserved(a: &str, b: &str) -> bool {
    fn events(s: &str) -> Vec<TreeEvent> {
        let mut raw = Vec::new();
        emit_tree_events(&gdscript_syntax::parse(s).syntax_node(), &mut raw);
        let mut out = Vec::with_capacity(raw.len());
        for i in 0..raw.len() {
            // Drop a trailing comma (a `,` immediately before a closing-bracket token).
            let trailing_comma = matches!(&raw[i], TreeEvent::Token(SyntaxKind::Comma, _))
                && matches!(
                    raw.get(i + 1),
                    Some(TreeEvent::Token(
                        SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace,
                        _
                    ))
                );
            if !trailing_comma {
                out.push(raw[i].clone());
            }
        }
        out
    }
    events(a) == events(b)
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
        // a redundant grouping paren after a value-keyword is stripped; a precedence-significant one
        // is kept (with its space after the keyword), and a call paren hugs.
        assert_eq!(fmt_stmt("return ( x )"), "return x");
        assert_eq!(fmt_stmt("return ( a + b ) * c"), "return (a + b) * c");
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
    fn reflow_keeps_an_already_canonical_wrapped_statement() {
        // A wrapped statement that is too long to collapse stays in its canonical exploded form.
        let src = "func f():\n\tvar n = outermost_call(\n\t\tinner_first(aaaa, bbbb, cccc, dddd),\n\t\tinner_second(eeee, ffff, gggg, hhhh),\n\t\tinner_third(iiii, jjjj, kkkk)\n\t)\n";
        assert_eq!(fmt(src), src);
    }

    // ---- Phase-4: layout ownership (re-flow already-multi-line statements) ----

    #[test]
    fn reflow_collapses_a_short_hand_wrapped_statement() {
        // A statement the author wrapped that now fits is collapsed back onto one line.
        let src = "func f():\n\tvar x = call(\n\t\ta,\n\t\tb,\n\t\tc\n\t)\n";
        assert_eq!(fmt(src), "func f():\n\tvar x = call(a, b, c)\n");
    }

    #[test]
    fn reflow_rewraps_a_still_too_long_wrapped_statement_idempotently() {
        // A wrapped statement still over the limit is re-laid-out to canonical form, idempotently.
        let src = "func f():\n\tvar x = some_long_function_name(argument_number_one, argument_number_two,\n\t\targument_number_three, argument_number_four, argument_number_five)\n";
        let out = fmt(src);
        assert!(
            out.contains("some_long_function_name(\n"),
            "should wrap: {out:?}"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn reflow_keeps_a_statement_with_an_inner_comment_verbatim() {
        // A comment inside the brackets blocks a safe collapse — the statement is preserved.
        let src = "func f():\n\tvar x = call(\n\t\ta,  # first\n\t\tb,\n\t)\n";
        let out = fmt(src);
        assert!(out.contains("# first"), "{out:?}");
        assert!(super::same_significant_tokens(src, &out));
        assert_eq!(fmt(&out), out, "idempotent");
    }

    // ---- Phase-4: CST-driven wrapping (gdformat parity — see `wrap`) ----

    #[test]
    fn wrap_func_param_list_explodes_with_return_type_on_close_line() {
        // A func header over the limit wraps its *parameter list*; the `-> void:` stays a suffix on the
        // closing-paren line (it is never itself wrapped), matching gdformat.
        let src = "func process(first_argument: int, second_argument: String, third_argument: float, fourth: bool) -> void:\n\tpass\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func process(\n\tfirst_argument: int, second_argument: String, third_argument: float, fourth: bool\n) -> void:\n\tpass\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn wrap_method_chain_bottom_up_wraps_final_call_args() {
        // When the chain prefix fits, gdformat wraps only the final call's arguments (bottom-up).
        let src = "func f():\n\tobject.method_one(argument).method_two(argument).method_three(argument).method_four(argument_xxxx)\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func f():\n\tobject.method_one(argument).method_two(argument).method_three(argument).method_four(\n\t\targument_xxxx\n\t)\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn wrap_method_chain_explodes_leading_dot_when_compact_overflows() {
        // When even the compact chain overflows, gdformat wraps it in parens and breaks at each `.`,
        // leading-dot style (`. method`).
        let src = "func _ready():\n\ttween.tween_property(self, ^\"modulate:a\", 0.0, fade_out_duration).set_trans(Tween.TRANS_LINEAR).set_ease(Tween.EASE_OUT)\n";
        let out = fmt(src);
        assert!(
            out.contains("\t(\n\t\ttween\n\t\t. tween_property("),
            "{out}"
        );
        assert!(
            out.contains("\n\t\t. set_ease(Tween.EASE_OUT)\n\t)\n"),
            "{out}"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn wrap_assignment_operator_chain_wraps_in_parens_compact_first() {
        // A too-long assignment RHS that is an operator chain is wrapped in injected parens; the chain
        // stays on one continuation line while it fits there (gdformat's compact-first), exploding at
        // the operator only when even that overflows.
        let src = "func f():\n\tgravity_value = first_long_operand_value_xx * second_long_operand_value_yy * third_long_operand_value_zz\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func f():\n\tgravity_value = (\n\t\tfirst_long_operand_value_xx * second_long_operand_value_yy * third_long_operand_value_zz\n\t)\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn wrap_dict_entry_drops_multiline_value_below_the_key() {
        // gdformat's kv-pair rule: a multi-line dict-entry value drops to its own line(s) below the
        // `key =`, and a magic trailing comma forces the whole nest exploded.
        let src = "func f():\n\tvar d := {player = {position = a, health = b,}, enemies = [],}\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func f():\n\tvar d := {\n\t\tplayer =\n\t\t{\n\t\t\tposition = a,\n\t\t\thealth = b,\n\t\t},\n\t\tenemies = [],\n\t}\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn wrap_magic_comma_chain_explodes_leading_dot() {
        // A method chain forced multi-line by a magic comma inside it goes straight to leading-dot.
        let src = "func f():\n\treturn obj.method({\"a\": 1, \"b\": 2,})\n";
        let out = fmt(src);
        assert_eq!(
            out,
            "func f():\n\treturn (\n\t\tobj\n\t\t. method(\n\t\t\t{\n\t\t\t\t\"a\": 1,\n\t\t\t\t\"b\": 2,\n\t\t\t}\n\t\t)\n\t)\n"
        );
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn column_zero_trailing_region_comment_stays_put_without_forced_blanks() {
        // gdformat keeps a column-0 `#endregion` at column 0 and forces no blank lines before a
        // *trailing* comment (one that ends its block).
        let src = "func f():\n\tvar x = 1\n#endregion\n";
        assert_eq!(fmt(src), src);
        let src2 = "#region Section\nvar a = 1\nvar b = 2\n#endregion\n";
        assert_eq!(fmt(src2), src2);
    }

    #[test]
    fn inline_suite_bodies_split_but_lambdas_stay_inline() {
        // gdformat moves an inline suite body to its own indented line; an inline *lambda* body stays.
        assert_eq!(
            fmt("func f():\n\tif cond: do_thing()\n"),
            "func f():\n\tif cond:\n\t\tdo_thing()\n"
        );
        assert_eq!(fmt("func g(): return 1\n"), "func g():\n\treturn 1\n");
        assert_eq!(
            fmt("func i():\n\tif a: b()\n\telse: c()\n"),
            "func i():\n\tif a:\n\t\tb()\n\telse:\n\t\tc()\n"
        );
        // an inline lambda body is preserved
        assert_eq!(
            fmt("func h():\n\tvar a := func(): return 1\n"),
            "func h():\n\tvar a := func(): return 1\n"
        );
    }

    #[test]
    fn backslash_line_continuations_collapse_and_rewrap() {
        // gdformat collapses a `\`-continued statement and re-lays it out — onto one line when it now
        // fits, or re-wrapped (an operator chain becomes paren-wrapped) when it does not.
        assert_eq!(
            fmt("func f():\n\tvar x = a + \\\n\t\tb\n"),
            "func f():\n\tvar x = a + b\n"
        );
        let out = fmt(
            "func f():\n\tif long_condition_name_one == 1 or \\\n\t\t\tlong_condition_name_two == 2 or long_condition_name_three == 3:\n\t\tpass\n",
        );
        assert!(out.contains("\tif (\n"), "{out}");
        assert!(!out.contains('\\'), "backslash removed: {out}");
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn redundant_grouping_parens_are_stripped() {
        // gdformat strips parens that merely group a standalone expression (value / condition /
        // iterable / argument / element / nested), but keeps precedence-significant ones.
        assert_eq!(fmt("var x = (y)\n"), "var x = y\n");
        assert_eq!(
            fmt("func f():\n\treturn (g(a))\n"),
            "func f():\n\treturn g(a)\n"
        );
        assert_eq!(
            fmt("func f():\n\tfor i in (a * b):\n\t\tpass\n"),
            "func f():\n\tfor i in a * b:\n\t\tpass\n"
        );
        assert_eq!(fmt("var a = g((x))\n"), "var a = g(x)\n"); // call arg + nested
        assert_eq!(fmt("var a = {(k): (v)}\n"), "var a = {k: v}\n");
        // precedence parens kept; an expr-statement assignment RHS keeps its parens (gdformat does too)
        assert_eq!(fmt("var a = (b + c) * d\n"), "var a = (b + c) * d\n");
        assert_eq!(fmt("func f():\n\tx = (y)\n"), "func f():\n\tx = (y)\n");
    }

    #[test]
    fn semicolon_separated_statements_split() {
        // gdformat splits `;`-separated statements onto their own lines and drops a trailing `;`.
        assert_eq!(
            fmt("func f():\n\ta = 1; b = 2\n"),
            "func f():\n\ta = 1\n\tb = 2\n"
        );
        assert_eq!(fmt("func g():\n\tpass;\n"), "func g():\n\tpass\n");
        assert_eq!(
            fmt("func h():\n\tif c: a(); b()\n"),
            "func h():\n\tif c:\n\t\ta()\n\t\tb()\n"
        );
    }

    #[test]
    fn inline_match_arm_and_property_bodies_split() {
        // A match arm sits one level below `match`, so its inline body splits to two deeper levels.
        assert_eq!(
            fmt("func f():\n\tmatch x:\n\t\t\"inc\": return state + 1\n"),
            "func f():\n\tmatch x:\n\t\t\"inc\":\n\t\t\treturn state + 1\n"
        );
        // A property setter shorthand splits below the `var`.
        assert_eq!(
            fmt("var active: bool = false: set = set_active\n"),
            "var active: bool = false:\n\tset = set_active\n"
        );
        // Inline getter/setter bodies split too.
        assert_eq!(
            fmt("var q: int = 0:\n\tget: return _q\n\tset(v): _q = v\n"),
            "var q: int = 0:\n\tget:\n\t\treturn _q\n\tset(v):\n\t\t_q = v\n"
        );
    }

    #[test]
    fn blank_runs_collapse_to_one_then_defs_restore_two() {
        // gdformat squeezes every blank run to one, then re-adds the 2nd blank only around defs.
        // Between two non-defs, that means a single blank regardless of how many were authored.
        assert_eq!(
            fmt("var a = 1\n\n\nvar b = 2\n"),
            "var a = 1\n\nvar b = 2\n"
        );
        assert_eq!(
            fmt("extends Node\n\n\nvar b = 2\n"),
            "extends Node\n\nvar b = 2\n"
        );
        // Two top-level funcs still get two blanks (def-forced), no matter the authored count.
        assert_eq!(
            fmt("func a():\n\tpass\n\n\n\nfunc b():\n\tpass\n"),
            "func a():\n\tpass\n\n\nfunc b():\n\tpass\n"
        );
    }

    #[test]
    fn annotation_prefixed_def_forces_blanks_only_after_a_def() {
        // An `@rpc func` after a non-def (`extends`/`var`) keeps its source blanks; after a def it gets
        // the usual 2; a plain func always forces 2.
        assert_eq!(
            fmt("extends Node\n\n@rpc(\"x\")\nfunc f():\n\tpass\n"),
            "extends Node\n\n@rpc(\"x\")\nfunc f():\n\tpass\n"
        );
        assert_eq!(
            fmt("func a():\n\tpass\n@rpc(\"x\")\nfunc b():\n\tpass\n"),
            "func a():\n\tpass\n\n\n@rpc(\"x\")\nfunc b():\n\tpass\n"
        );
        assert_eq!(
            fmt("extends Node\nfunc f():\n\tpass\n"),
            "extends Node\n\n\nfunc f():\n\tpass\n"
        );
    }

    #[test]
    fn leading_bom_is_preserved() {
        // gdformat keeps a leading byte-order mark; the reflow must not drop it when it re-renders the
        // first statement from its significant tokens.
        let src = "\u{feff}class_name Foo\nvar x = 1\n";
        let out = fmt(src);
        assert!(out.starts_with('\u{feff}'), "{out:?}");
        assert_eq!(&out[3..], "class_name Foo\nvar x = 1\n");
        assert_eq!(fmt(&out), out, "idempotent");
    }

    #[test]
    fn soft_keyword_member_call_hugs_paren_but_statement_keeps_space() {
        // `obj.match(x)` is a member call (tight `(`), while a `match (x):` statement keeps its space.
        assert_eq!(
            fmt("func f():\n\tvar m = obj.match(\"a\", \"b\")\n"),
            "func f():\n\tvar m = obj.match(\"a\", \"b\")\n"
        );
        assert_eq!(
            fmt("func f():\n\tmatch (x):\n\t\tpass\n"),
            "func f():\n\tmatch (x):\n\t\tpass\n"
        );
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
        // gdformat collapses a single-line triple-SINGLE to a regular string; triple-double + any
        // multi-line triple are left verbatim.
        assert_eq!(c("'''triple'''"), "\"triple\"");
        assert_eq!(c("'''say \"hi\"'''"), "'say \"hi\"'"); // body has " -> regular single
        assert_eq!(c("\"\"\"triple\"\"\""), "\"\"\"triple\"\"\""); // triple-double verbatim
        assert_eq!(c("'''line1\nline2'''"), "'''line1\nline2'''"); // multi-line verbatim
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

    // ---- Phase-4: inline-comment offset ----

    #[test]
    fn inline_comments_get_two_spaces() {
        assert_eq!(fmt("var x = 1 # one\n"), "var x = 1  # one\n");
        assert_eq!(fmt("var y = 2     # many\n"), "var y = 2  # many\n");
        assert_eq!(
            fmt("func f(): # c\n\tpass ## doc\n"),
            "func f():  # c\n\tpass  ## doc\n"
        );
        // a standalone comment (its own line) is unaffected — it is indentation, not an offset
        assert_eq!(
            fmt("func f():\n\t# standalone\n\tpass\n"),
            "func f():\n\t# standalone\n\tpass\n"
        );
    }

    // ---- Phase-4: format_range ----

    #[test]
    fn format_range_edits_only_the_changed_lines_overlapping_the_selection() {
        let src = "func f():\n\tvar x = 1\n\tvar y=2\n";
        // line 2 (bytes 21..30) needs spacing; selecting it returns just that line's edit
        let e = super::format_range(src, &FmtConfig::default(), 21..30).unwrap();
        assert_eq!(e.range, 21..30);
        assert_eq!(e.new_text, "\tvar y = 2\n");
        // selecting the already-formatted line 1 → no edit
        assert!(super::format_range(src, &FmtConfig::default(), 10..21).is_none());
        // a fully-formatted document → no edit
        assert!(
            super::format_range("func f():\n\tvar y = 2\n", &FmtConfig::default(), 0..20).is_none()
        );
    }

    // ---- Phase-4: enum-brace spacing ----

    #[test]
    fn enum_braces_are_spaced_dicts_are_not() {
        assert_eq!(fmt("enum E {A, B, C}\n"), "enum E { A, B, C }\n");
        assert_eq!(fmt("enum {A, B}\n"), "enum { A, B }\n"); // anonymous
        assert_eq!(
            fmt("enum Named {RED = 1, GREEN = 2}\n"),
            "enum Named { RED = 1, GREEN = 2 }\n"
        );
        assert_eq!(fmt("enum Empty {}\n"), "enum Empty {}\n"); // empty stays tight
        // a dict literal stays tight even right after an enum on the previous line
        assert_eq!(
            fmt("enum E {A}\nvar d = {\"k\": 1}\n"),
            "enum E { A }\nvar d = {\"k\": 1}\n"
        );
    }

    // ---- Phase-4: operator-chain wrapping ----

    #[test]
    fn operator_chain_if_condition_breaks_operator_leading() {
        let src = "func f():\n\tif condition_number_one and condition_number_two and condition_number_three and condition_number_four:\n\t\tpass\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tif (\n\t\tcondition_number_one\n\t\tand condition_number_two\n\t\tand condition_number_three\n\t\tand condition_number_four\n\t):\n\t\tpass\n"
        );
        assert_eq!(fmt(&fmt(src)), fmt(src), "idempotent");
    }

    #[test]
    fn operator_chain_breaks_at_lowest_precedence_only() {
        // `a and b or c and d`: break at the lower-precedence `or`, keeping the `and` groups inline.
        let src = "func f():\n\tif aaaaaaaaaaaaaaaaaaaaaaaa and bbbbbbbbbbbbbbbbbbbbbbbb or cccccccccccccccccccccccc and dddddddd:\n\t\tpass\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tif (\n\t\taaaaaaaaaaaaaaaaaaaaaaaa and bbbbbbbbbbbbbbbbbbbbbbbb\n\t\tor cccccccccccccccccccccccc and dddddddd\n\t):\n\t\tpass\n"
        );
    }

    #[test]
    fn operator_chain_dot_chain_wraps_compact() {
        let src = "func f():\n\tvar chain = some_object.first_method().second_method().third_method().fourth_method().fifth_method_x()\n";
        assert_eq!(
            fmt(src),
            "func f():\n\tvar chain = (\n\t\tsome_object.first_method().second_method().third_method().fourth_method().fifth_method_x()\n\t)\n"
        );
    }

    #[test]
    fn operator_chain_never_splits_a_node_path() {
        // A node-path's `/` are path separators, not division — an over-long node-path must stay on
        // one line (the parser reads `$A / B` as the same path as `$A/B`, so splitting it would be a
        // silent meaning change in disguise — and gdformat leaves it alone anyway).
        let src = "func f():\n\tvar n = $LongContainerNameHere/AnotherLongChildNode/YetAnotherChildNode/AndOneMoreChildNodeHere/FinalNode\n";
        assert_eq!(fmt(src), src);
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
    fn meaning_preserved_accepts_quote_trailing_comma_and_redundant_parens() {
        use super::meaning_preserved as mp;
        // string quotes, trailing commas, and *redundant* grouping parens are all accepted
        assert!(mp("var a = 'x'\n", "var a = \"x\"\n"));
        assert!(mp("func f():\n\tg(a, b,)\n", "func f():\n\tg(a, b)\n"));
        assert!(mp(
            "func f():\n\tvar x = [1, 2,]\n",
            "func f():\n\tvar x = [1, 2]\n"
        ));
        assert!(mp("var x = (a + b)\n", "var x = a + b\n"));
        assert!(mp(
            "func f():\n\tif (a and b):\n\t\tpass\n",
            "func f():\n\tif a and b:\n\t\tpass\n"
        ));
        // but real changes — value, dropped token, and PRECEDENCE — are still caught
        assert!(!mp("var a = 'x'\n", "var a = \"y\"\n"));
        assert!(!mp("func f():\n\tg(a, b)\n", "func f():\n\tg(a)\n"));
        assert!(!mp("var x = (a + b) * c\n", "var x = a + b * c\n"));
    }
}
