//! WS2 — the indentation pre-pass (the highest-risk module).
//!
//! GDScript has Python-like significant indentation. This pass consumes the flat
//! [`RawToken`] stream from the lexer and injects the synthetic, **zero-width**
//! `Newline`/`Indent`/`Dedent` markers the parser needs to recover block structure,
//! while leaving every original byte-carrying token (real tokens **and** trivia)
//! exactly where it was — so the round-trip stays byte-exact.
//!
//! Modeled on Godot's own `gdscript_tokenizer.cpp`
//! (`plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §WS2), **not** tree-sitter's
//! `scanner.c`. Key engine-faithful choices:
//! - **Tab width is a flat `tab_size` (default 4)**, `+1` per space — Godot adds a
//!   flat `tab_size` per tab, not 8-column tab stops.
//! - **Bracket suppression:** inside `()`/`[]`/`{}` newlines/indentation are not
//!   significant (a depth counter pauses marker emission).
//! - **`\` line continuations** are already merged into single `LineContinuation`
//!   tokens by the lexer, so splitting logical lines on physical newlines joins them
//!   for free.
//! - **Blank / comment-only lines keep indentation state** (no spurious `Dedent`),
//!   so a column-0 comment inside a body never closes the scope.
//! - **Two distinct diagnostics** (same-line tab+space mix; cross-line deviation from
//!   the file's first indent character) — both recover, never abort.
//!
//! - **Lambda bodies inside brackets** re-enable indentation. Inside `()[]{}`
//!   indentation is normally suppressed, but a *multiline lambda* body that lives
//!   inside an open bracket (e.g. `arr.sort_custom(func(a, b):\n\treturn a < b\n)`)
//!   must still be a block. We mirror Godot's stack-of-stacks: a line that ends with
//!   `:` while inside brackets opens a fresh indentation context for the lambda body,
//!   which closes (restoring the bracket-suppressed context) once a later line dedents
//!   back to the header's column.

use text_size::{TextRange, TextSize};

use crate::SyntaxKind;
use crate::lexer::RawToken;

/// Godot's default indentation width for a tab character.
const TAB_SIZE: u32 = 4;

/// A saved indentation context for a lambda body opened inside brackets. When the
/// lambda's `:` is reached we stash the surrounding indent stack and start a fresh one
/// based at the header line's column; the body closes once indentation returns to
/// `base`, restoring `saved_indent_stack`.
#[derive(Debug, Clone)]
struct LambdaCtx {
    saved_indent_stack: Vec<u32>,
    base: u32,
    /// The `bracket_depth` the lambda body lives at (the depth inside its enclosing
    /// bracket). When a closing bracket drops below this, the body ends — even mid-line,
    /// when the closer trails the last body statement (`call(func(): … last())`).
    open_bracket_depth: u32,
}

/// An indentation diagnostic produced while injecting block-structure markers.
/// Byte-ranged; mapped into a `gdscript-base` `Diagnostic` by the IDE layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndentDiagnostic {
    /// The offending leading-whitespace range.
    pub range: TextRange,
    /// A human-readable message (mirrors Godot's wording).
    pub message: String,
}

/// Which character a line used for its leading indentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentChar {
    Tab,
    Space,
}

/// Inject `Newline`/`Indent`/`Dedent` markers into the lexer token stream.
///
/// Returns the augmented token stream plus any indentation diagnostics. The output is
/// still lossless: the injected markers are zero-width, and every input token is
/// preserved in order.
#[must_use]
pub fn run(tokens: &[RawToken], src: &str) -> (Vec<RawToken>, Vec<IndentDiagnostic>) {
    let mut p = PrePass {
        src,
        out: Vec::with_capacity(tokens.len() + 16),
        diags: Vec::new(),
        indent_stack: vec![0],
        bracket_depth: 0,
        indent_char: None,
        lambda_stack: Vec::new(),
    };
    p.run_lines(tokens);
    (p.out, p.diags)
}

struct PrePass<'s> {
    src: &'s str,
    out: Vec<RawToken>,
    diags: Vec<IndentDiagnostic>,
    indent_stack: Vec<u32>,
    bracket_depth: u32,
    indent_char: Option<IndentChar>,
    /// Active lambda-body indentation contexts (innermost last). Non-empty means
    /// indentation is significant *despite* being inside brackets.
    lambda_stack: Vec<LambdaCtx>,
}

impl PrePass<'_> {
    fn run_lines(&mut self, tokens: &[RawToken]) {
        // Split the stream into physical lines (each ends at a `NewlinePhys`, or at
        // EOF). `\`-continuations are already absorbed into `LineContinuation` tokens,
        // so a continued logical line is naturally one slice here.
        let mut start = 0usize;
        let mut i = 0usize;
        while i < tokens.len() {
            if tokens[i].kind == SyntaxKind::NewlinePhys {
                self.line(&tokens[start..=i]);
                start = i + 1;
            }
            i += 1;
        }
        if start < tokens.len() {
            self.line(&tokens[start..]); // trailing line without a final newline
        }
        self.finish(src_end(self.src));
    }

    /// Process one physical line (the slice may end with a `NewlinePhys`).
    ///
    /// Indentation is significant when we are outside all brackets **or** inside a
    /// lambda body opened within brackets. The logical `Newline` is emitted at the
    /// terminator when we are at bracket depth 0, inside a lambda body, or the line is
    /// itself a lambda header (its `:` opens a body block).
    fn line(&mut self, line: &[RawToken]) {
        // Blank / comment-only lines keep indentation state — copy verbatim, no
        // markers (this is what stops a column-0 comment from closing a scope). A line
        // whose only non-trivia content is the newline is blank too, since
        // `NewlinePhys` is trivia.
        let Some(first) = line.iter().find(|t| !t.kind.is_trivia()) else {
            self.copy_verbatim(line);
            return;
        };
        let col = self.column(line);
        let at = first.range.start();

        // A line whose first meaningful token is a closing bracket that closes a lambda's enclosing
        // bracket is a *bracket continuation* — the `)` of `call(func(): … )` on its own dedented
        // line, which real code often indents BETWEEN the lambda header and its body. Close that body
        // now by BRACKET DEPTH (not column) so the line is treated as indentation-suppressed and no
        // spurious INDENT is emitted for where the closer happens to sit. (The column-based
        // `close_lambdas` below only fires when the line dedents to at-or-below the header column.)
        if matches!(
            first.kind,
            SyntaxKind::RParen | SyntaxKind::RBrace | SyntaxKind::RBrack
        ) && self
            .lambda_stack
            .last()
            .is_some_and(|ctx| ctx.open_bracket_depth >= self.bracket_depth)
        {
            self.close_lambdas_on_bracket(self.bracket_depth.saturating_sub(1), at);
        }

        // Close any lambda bodies this line has dedented back out of.
        self.close_lambdas(col, at);

        let in_lambda = !self.lambda_stack.is_empty();
        let suppressed = !in_lambda && self.bracket_depth > 0;
        // Whether this physical line, when it ends with `:` inside brackets, is a *lambda header*
        // (`… func(params) [-> Type]:`) rather than a dict entry whose value sits on the next line
        // (`"key":\n value`). Both end with `:` inside brackets, but only the lambda opens a body
        // block; a dict-entry colon must keep its newline suppressed so the value continues the entry.
        let is_lambda_header = line_is_lambda_header(line);

        // Indentation markers only where indentation is significant.
        if !suppressed {
            self.diagnose_indent(line);
            self.emit_indent_dedent(col, at);
        }

        // Copy the line's tokens, tracking brackets, and emit a logical Newline at the terminator
        // where appropriate.
        let mut has_terminator = false;
        for tok in line {
            if tok.kind == SyntaxKind::NewlinePhys {
                has_terminator = true;
                let opens_lambda = self.bracket_depth > 0 && is_lambda_header;
                // Use the *current* lambda state, not the line-start `in_lambda`: a lambda body that
                // closed mid-line (`func(): … return X,` / `… last())`) is no longer significant, so
                // its trailing physical newline must stay suppressed inside the enclosing bracket —
                // otherwise a stray `Newline` lands between the call's argument `,` and the next arg.
                let in_lambda_now = !self.lambda_stack.is_empty();
                if self.bracket_depth == 0 || in_lambda_now || opens_lambda {
                    self.push_marker(SyntaxKind::Newline, tok.range.start());
                }
                self.out.push(*tok);
            } else {
                // A closing bracket that drops below a lambda body's enclosing depth
                // ends that body here, even mid-line — emit its `Dedent`s before the
                // bracket so the parser closes the block at the right place.
                if matches!(
                    tok.kind,
                    SyntaxKind::RParen | SyntaxKind::RBrace | SyntaxKind::RBrack
                ) && !self.lambda_stack.is_empty()
                {
                    let new_depth = self.bracket_depth.saturating_sub(1);
                    self.close_lambdas_on_bracket(new_depth, tok.range.start());
                }
                // A `,` at a lambda body's OWN enclosing bracket depth is the enclosing call's
                // argument separator (`call(func(): body, next_arg)` — a bare comma can't be valid
                // lambda-body syntax at that depth), so it ends the body mid-line too. Close by depth,
                // not column, before the comma.
                else if tok.kind == SyntaxKind::Comma
                    && self
                        .lambda_stack
                        .last()
                        .is_some_and(|ctx| ctx.open_bracket_depth == self.bracket_depth)
                {
                    self.close_lambdas_on_bracket(
                        self.bracket_depth.saturating_sub(1),
                        tok.range.start(),
                    );
                }
                self.out.push(*tok);
                self.track_bracket(tok.kind);
            }
        }
        // A final line with content but no trailing newline still terminates a statement.
        if !has_terminator && (self.bracket_depth == 0 || in_lambda) {
            self.push_marker(SyntaxKind::Newline, src_end(self.src));
        }

        // A lambda header inside brackets opens a fresh indentation context for its body, based at
        // this line's column. (A dict-entry colon with the value on the next line is *not* a header.)
        if self.bracket_depth > 0 && is_lambda_header {
            let saved = std::mem::replace(&mut self.indent_stack, vec![col]);
            self.lambda_stack.push(LambdaCtx {
                saved_indent_stack: saved,
                base: col,
                open_bracket_depth: self.bracket_depth,
            });
        }
    }

    /// Close every lambda body whose base column is `>= col` (i.e. that this line has
    /// dedented out of), emitting the `Dedent`s for its body and restoring the
    /// surrounding indentation context.
    fn close_lambdas(&mut self, col: u32, at: TextSize) {
        while self.lambda_stack.last().is_some_and(|ctx| col <= ctx.base) {
            let base = self.lambda_stack.last().expect("checked").base;
            while *self.indent_stack.last().expect("lambda base present") > base {
                self.indent_stack.pop();
                self.push_marker(SyntaxKind::Dedent, at);
            }
            let ctx = self.lambda_stack.pop().expect("checked");
            self.indent_stack = ctx.saved_indent_stack;
        }
    }

    /// Close lambda bodies whose enclosing bracket has just closed — a `)`/`]`/`}` that
    /// drops `bracket_depth` to `new_depth` *mid-line*. Mirrors [`Self::close_lambdas`]
    /// but is keyed on bracket depth instead of column, for the case where the closer
    /// trails the last body statement on one line (`call(func(): … last())`). The
    /// column-based path already handles a closer that sits on its own dedented line; a
    /// lambda is only ever popped once, so the two paths never double-close.
    fn close_lambdas_on_bracket(&mut self, new_depth: u32, at: TextSize) {
        while self
            .lambda_stack
            .last()
            .is_some_and(|ctx| ctx.open_bracket_depth > new_depth)
        {
            let base = self.lambda_stack.last().expect("checked").base;
            while *self.indent_stack.last().expect("lambda base present") > base {
                self.indent_stack.pop();
                self.push_marker(SyntaxKind::Dedent, at);
            }
            let ctx = self.lambda_stack.pop().expect("checked");
            self.indent_stack = ctx.saved_indent_stack;
        }
    }

    /// Copy a blank / comment-only line's tokens unchanged (no structural markers),
    /// only updating bracket depth so an open multiline literal stays open across it.
    fn copy_verbatim(&mut self, line: &[RawToken]) {
        for tok in line {
            self.out.push(*tok);
            if tok.kind != SyntaxKind::NewlinePhys {
                self.track_bracket(tok.kind);
            }
        }
    }

    /// Compare `col` to the indent stack and push `Indent` / `Dedent` markers.
    fn emit_indent_dedent(&mut self, col: u32, at: TextSize) {
        let top = *self.indent_stack.last().expect("indent stack has a base 0");
        if col > top {
            self.indent_stack.push(col);
            self.push_marker(SyntaxKind::Indent, at);
        } else if col < top {
            while *self.indent_stack.last().expect("base 0 guards the loop") > col {
                self.indent_stack.pop();
                self.push_marker(SyntaxKind::Dedent, at);
            }
            if *self.indent_stack.last().expect("non-empty") != col {
                self.diags.push(IndentDiagnostic {
                    range: TextRange::empty(at),
                    message: "Unindent does not match any outer indentation level.".to_owned(),
                });
                self.indent_stack.push(col); // resync and keep going
            }
        }
    }

    /// The leading-whitespace column of a line (Godot's flat `tab_size` per tab, `+1`
    /// per space). Pure — used for the lambda-context bookkeeping before deciding
    /// whether to diagnose.
    fn column(&self, line: &[RawToken]) -> u32 {
        let Some(ws) = line.first().filter(|t| t.kind == SyntaxKind::Whitespace) else {
            return 0;
        };
        self.src[ws.range]
            .bytes()
            .fold(0u32, |col, b| col + if b == b'\t' { TAB_SIZE } else { 1 })
    }

    /// Record any tab/space indentation diagnostics for a line (same-line mix;
    /// cross-line inconsistency with the file's first indent character).
    fn diagnose_indent(&mut self, line: &[RawToken]) {
        let Some(ws) = line.first().filter(|t| t.kind == SyntaxKind::Whitespace) else {
            return;
        };
        let text = &self.src[ws.range];
        let mut saw_tab = false;
        let mut saw_space = false;
        for b in text.bytes() {
            saw_tab |= b == b'\t';
            saw_space |= b == b' ';
        }
        if saw_tab && saw_space {
            self.diags.push(IndentDiagnostic {
                range: ws.range,
                message: "Mixed use of tabs and spaces for indentation.".to_owned(),
            });
        } else if let Some(first) = text.bytes().next() {
            let this = if first == b'\t' {
                IndentChar::Tab
            } else {
                IndentChar::Space
            };
            match self.indent_char {
                None => self.indent_char = Some(this),
                Some(file) if file != this => {
                    let (used, before) = match this {
                        IndentChar::Tab => ("tab", "space"),
                        IndentChar::Space => ("space", "tab"),
                    };
                    self.diags.push(IndentDiagnostic {
                        range: ws.range,
                        message: format!(
                            "Used {used} character for indentation instead of {before} as used before in the file."
                        ),
                    });
                }
                Some(_) => {}
            }
        }
    }

    /// At end of input, close any still-open lambda bodies, then terminate any open
    /// block by popping the indent stack to 0.
    fn finish(&mut self, at: TextSize) {
        self.close_lambdas(0, at); // col 0 <= every base, so all lambdas close
        while *self.indent_stack.last().expect("base 0") > 0 {
            self.indent_stack.pop();
            self.push_marker(SyntaxKind::Dedent, at);
        }
    }

    fn track_bracket(&mut self, kind: SyntaxKind) {
        match kind {
            SyntaxKind::LParen | SyntaxKind::LBrack | SyntaxKind::LBrace => {
                self.bracket_depth += 1;
            }
            SyntaxKind::RParen | SyntaxKind::RBrack | SyntaxKind::RBrace => {
                self.bracket_depth = self.bracket_depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn push_marker(&mut self, kind: SyntaxKind, at: TextSize) {
        self.out.push(RawToken {
            kind,
            range: TextRange::empty(at),
        });
    }
}

/// The end-of-source offset as a `TextSize`.
fn src_end(src: &str) -> TextSize {
    TextSize::of(src)
}

/// Whether a physical line is a **lambda header** — `… func(params) [-> Type]:` ending in `:`.
///
/// Used to distinguish a lambda whose body follows on the next line (its `:` opens an indented
/// block) from a dict entry whose value sits on the next line (`"key":\n value`), since both end
/// with `:` inside brackets. We find the last `func` keyword and require that what follows is a
/// balanced parameter list, then only an optional `-> Type` return annotation, then the line's
/// terminal `:` — i.e. no further `:` (which would mean an inline lambda body or a dict colon owns
/// the terminal one).
fn line_is_lambda_header(line: &[RawToken]) -> bool {
    use SyntaxKind as S;
    let toks: Vec<S> = line
        .iter()
        .map(|t| t.kind)
        .filter(|k| !k.is_trivia())
        .collect();
    if toks.last() != Some(&S::Colon) {
        return false;
    }
    let Some(func_pos) = toks.iter().rposition(|&k| k == S::FuncKw) else {
        return false;
    };
    // `func` is followed by an optional name (named lambda) then the parameter list `(...)`.
    let mut i = func_pos + 1;
    if toks.get(i) == Some(&S::Ident) {
        i += 1;
    }
    if toks.get(i) != Some(&S::LParen) {
        return false;
    }
    let mut depth = 0u32;
    while i < toks.len() {
        match toks[i] {
            S::LParen => depth += 1,
            S::RParen => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        return false;
    }
    // Between the params' `)` and the terminal `:` only a `-> Type` may appear — no other colon.
    let last = toks.len() - 1;
    !toks[i..last].contains(&S::Colon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize;

    fn prepass(src: &str) -> Vec<RawToken> {
        run(&tokenize(src), src).0
    }

    /// Non-trivia kind sequence — shows the synthetic markers + real tokens, hiding
    /// whitespace/comment noise.
    fn structure(src: &str) -> Vec<SyntaxKind> {
        prepass(src)
            .into_iter()
            .filter(|t| !t.kind.is_trivia())
            .map(|t| t.kind)
            .collect()
    }

    fn diagnostics(src: &str) -> Vec<IndentDiagnostic> {
        run(&tokenize(src), src).1
    }

    /// The pre-pass must remain byte-exact: zero-width markers contribute nothing and
    /// every original token is preserved.
    fn assert_lossless(src: &str) {
        let rebuilt: String = prepass(src).iter().map(|t| &src[t.range]).collect();
        assert_eq!(rebuilt, src, "prepass not lossless for {src:?}");
    }

    fn count(src: &str, kind: SyntaxKind) -> usize {
        structure(src).into_iter().filter(|&k| k == kind).count()
    }

    #[test]
    fn nested_func_if_drives_indent_dedent() {
        use SyntaxKind as S;
        let src = "func f():\n\tif x:\n\t\treturn\n";
        assert_lossless(src);
        assert_eq!(
            structure(src),
            vec![
                S::FuncKw,
                S::Ident,
                S::LParen,
                S::RParen,
                S::Colon,
                S::Newline,
                S::Indent,
                S::IfKw,
                S::Ident,
                S::Colon,
                S::Newline,
                S::Indent,
                S::ReturnKw,
                S::Newline,
                S::Dedent,
                S::Dedent,
            ]
        );
    }

    #[test]
    fn line_continuation_does_not_indent() {
        // Case 1: a `\`-continued line never produces Newline/Indent mid-expression.
        let src = "a = 1 + \\\n  2\n";
        assert_lossless(src);
        assert_eq!(count(src, SyntaxKind::Indent), 0);
        assert_eq!(count(src, SyntaxKind::Newline), 1); // exactly one logical line
    }

    #[test]
    fn multiline_brackets_suppress_indentation() {
        // Case 2: newlines inside [] are not significant.
        let src = "var a = [\n\t1,\n\t2,\n]\n";
        assert_lossless(src);
        assert_eq!(count(src, SyntaxKind::Indent), 0);
        assert_eq!(count(src, SyntaxKind::Dedent), 0);
        assert_eq!(count(src, SyntaxKind::Newline), 1); // one logical statement
    }

    #[test]
    fn top_level_lambda_body_indents() {
        // Case 4: a multiline lambda body at statement level indents normally.
        use SyntaxKind as S;
        let src = "var f = func():\n\tprint()\nx = 1\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 1);
        assert_eq!(count(src, S::Dedent), 1);
    }

    #[test]
    fn blank_and_comment_only_lines_keep_state() {
        // Cases 7 & 8: blank lines and a column-0 comment must not close the block.
        let src = "func f():\n\tx = 1\n\n# top-level comment\n\ty = 2\n";
        assert_lossless(src);
        // Only one Indent (into the body) and one Dedent (at EOF) — the blank and the
        // column-0 comment do not emit a Dedent.
        assert_eq!(count(src, SyntaxKind::Indent), 1);
        assert_eq!(count(src, SyntaxKind::Dedent), 1);
    }

    #[test]
    fn inline_block_has_no_indent() {
        // Case 9: `func f(): return 1` on one line never produces an Indent.
        let src = "func f(): return 1\n";
        assert_lossless(src);
        assert_eq!(count(src, SyntaxKind::Indent), 0);
        assert_eq!(count(src, SyntaxKind::Newline), 1);
    }

    #[test]
    fn dedent_to_eof_without_trailing_newline() {
        // Case 11: file ends mid-nest with no trailing newline.
        use SyntaxKind as S;
        let src = "func f():\n\tpass";
        assert_lossless(src);
        let s = structure(src);
        assert_eq!(s.last(), Some(&S::Dedent));
        assert_eq!(count(src, S::Indent), 1);
        assert_eq!(count(src, S::Dedent), 1);
        // The final unterminated line still gets a logical Newline.
        assert!(s.contains(&S::Newline));
    }

    #[test]
    fn empty_and_comment_only_files() {
        // Case 12.
        assert_lossless("");
        assert_eq!(structure(""), Vec::<SyntaxKind>::new());
        assert_lossless("# just a comment\n");
        assert_eq!(count("# just a comment\n", SyntaxKind::Indent), 0);
    }

    #[test]
    fn mixed_tabs_and_spaces_diagnoses_but_recovers() {
        // Case 6: a tab+space mix on one indentation run is flagged, not fatal.
        let src = "func f():\n \tpass\n";
        assert_lossless(src);
        let diags = diagnostics(src);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("Mixed use of tabs and spaces")),
            "expected a mixed-indent diagnostic, got {diags:?}"
        );
    }

    #[test]
    fn inconsistent_indent_char_across_lines_is_flagged() {
        // First indented line uses a tab; a later one uses spaces → file-consistency
        // diagnostic (first char wins).
        let src = "func f():\n\ta = 1\nfunc g():\n    b = 2\n";
        let diags = diagnostics(src);
        assert!(
            diags.iter().any(|d| d.message.contains("instead of")),
            "expected an inconsistent-indent diagnostic, got {diags:?}"
        );
    }

    #[test]
    fn match_block_nests() {
        use SyntaxKind as S;
        let src = "match x:\n\t1:\n\t\tpass\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 2);
        assert_eq!(count(src, S::Dedent), 2);
        assert_eq!(structure(src)[0], S::MatchKw);
    }

    #[test]
    fn multiline_lambda_inside_brackets_indents() {
        // A multiline lambda body inside an open `(` re-enables indentation.
        use SyntaxKind as S;
        let src = "arr.sort_custom(func(a, b):\n\treturn a < b\n)\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 1, "lambda body should Indent once");
        assert_eq!(count(src, S::Dedent), 1, "lambda body should Dedent once");
        // One logical statement (the call), terminated after the closing `)`.
        let s = structure(src);
        // The Indent comes right after the lambda's `:` + Newline.
        let colon = s.iter().position(|&k| k == S::Colon).unwrap();
        assert_eq!(s[colon + 1], S::Newline);
        assert_eq!(s[colon + 2], S::Indent);
        // The Dedent comes before the closing `)`.
        let rparen = s.iter().rposition(|&k| k == S::RParen).unwrap();
        assert_eq!(s[rparen - 1], S::Dedent);
    }

    #[test]
    fn lambda_inside_multiline_array() {
        // A lambda living inside a multiline `[ ]` literal.
        use SyntaxKind as S;
        let src = "var a = [\n\tfunc():\n\t\tprint()\n]\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 1);
        assert_eq!(count(src, S::Dedent), 1);
    }

    #[test]
    fn nested_lambdas_inside_brackets() {
        use SyntaxKind as S;
        let src = "outer(func():\n\tinner(func():\n\t\tbody\n\t)\n)\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 2, "two nested lambda bodies");
        assert_eq!(count(src, S::Dedent), 2);
    }

    #[test]
    fn single_line_lambda_inside_brackets_has_no_indent() {
        // The body is on the header line → no Indent/Dedent, one statement.
        use SyntaxKind as S;
        let src = "arr.map(func(x): x * 2)\n";
        assert_lossless(src);
        assert_eq!(count(src, S::Indent), 0);
        assert_eq!(count(src, S::Dedent), 0);
        assert_eq!(count(src, S::Newline), 1);
    }
}
