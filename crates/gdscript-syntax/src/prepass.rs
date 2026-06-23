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
//! **Known Tier-0 limitation:** a multiline lambda body that lives *inside* an open
//! bracket (e.g. `arr.sort_custom(func(a, b):\n\treturn a < b\n)`) is indentation-
//! suppressed by the surrounding bracket, so its body is not given `Indent`/`Dedent`
//! markers. Godot re-enables indentation there via a stack-of-stacks; wiring that
//! (a `saved_stacks: Vec<Vec<u32>>` on the state) is deferred and the divergence is
//! allowlisted. Top-level and ordinary nested lambda bodies indent correctly through
//! the normal mechanism.

use text_size::{TextRange, TextSize};

use crate::SyntaxKind;
use crate::lexer::RawToken;

/// Godot's default indentation width for a tab character.
const TAB_SIZE: u32 = 4;

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
    /// Indentation markers are decided by the bracket depth at the line's **start**
    /// (inside an open bracket, indentation is not significant); the logical `Newline`
    /// is decided by the bracket depth at the **terminator** (so the line that finally
    /// closes a multiline bracket still terminates the statement).
    fn line(&mut self, line: &[RawToken]) {
        let start_suppressed = self.bracket_depth > 0;

        // Blank / comment-only lines keep indentation state — copy verbatim, no
        // markers (this is what stops a column-0 comment from closing a scope). A line
        // whose only non-trivia content is the newline is blank too, since
        // `NewlinePhys` is trivia.
        let Some(first) = line.iter().find(|t| !t.kind.is_trivia()) else {
            self.copy_verbatim(line);
            return;
        };

        // Compute indentation only for lines that begin outside a bracket.
        if !start_suppressed {
            let col = self.measure_indent(line);
            self.emit_indent_dedent(col, first.range.start());
        }

        // Copy the line, tracking brackets, and emit a logical Newline before the
        // terminating physical newline when we have returned to bracket depth 0.
        let mut has_terminator = false;
        for tok in line {
            if tok.kind == SyntaxKind::NewlinePhys {
                has_terminator = true;
                if self.bracket_depth == 0 {
                    self.push_marker(SyntaxKind::Newline, tok.range.start());
                }
                self.out.push(*tok);
            } else {
                self.out.push(*tok);
                self.track_bracket(tok.kind);
            }
        }
        // A final line with content but no trailing newline still terminates a
        // statement.
        if !has_terminator && self.bracket_depth == 0 {
            self.push_marker(SyntaxKind::Newline, src_end(self.src));
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

    /// Measure the leading-whitespace column of a line and record any tab/space
    /// diagnostics (same-line mix; cross-line inconsistency with the file's first
    /// indent character).
    fn measure_indent(&mut self, line: &[RawToken]) -> u32 {
        let Some(ws) = line.first().filter(|t| t.kind == SyntaxKind::Whitespace) else {
            return 0; // no leading whitespace
        };
        let text = &self.src[ws.range];
        let mut col = 0u32;
        let mut saw_tab = false;
        let mut saw_space = false;
        for b in text.bytes() {
            if b == b'\t' {
                col += TAB_SIZE;
                saw_tab = true;
            } else {
                col += 1;
                saw_space |= b == b' ';
            }
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
        col
    }

    /// At end of input, terminate any open block by popping the indent stack to 0.
    fn finish(&mut self, at: TextSize) {
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
}
