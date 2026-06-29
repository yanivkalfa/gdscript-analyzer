//! CST-driven line wrapping — a faithful port of gdformat 4.5's expression formatter
//! (`gdtoolkit/formatter/expression.py` + `expression_to_str.py`).
//!
//! The reflow pass owns the layout of every *logical statement*. When a statement does not fit on one
//! line, gdformat does **not** use ad-hoc bracket heuristics: it walks the parse tree and recursively
//! renders each (sub-)expression *single-line if it fits, otherwise exploded* — a call/array/dict
//! explodes its comma-separated elements (each itself rendered the same way), an operator chain breaks
//! operator-leading inside injected parens, and a method (dot-)chain either wraps its final call's
//! arguments (bottom-up) or, failing that, explodes at each `.` (leading-dot). This module reproduces
//! that algorithm against our own CST so the output is byte-identical, not merely close.
//!
//! It is invoked from [`crate::render_statement`] with the *flattened* (single physical line, correctly
//! spaced) statement text. We re-parse that text, locate the statement, set up gdformat's
//! `(prefix, suffix)` expression context, and format. Anything we do not (yet) model returns `None`,
//! and the caller falls back to the previous heuristic — so this can only improve parity, never break
//! the safety guarantee (the meaning-equivalence net still gates the whole formatter).

use cstree::util::NodeOrToken;
use gdscript_syntax::{GdNode, SyntaxKind as S};

use crate::{FmtConfig, canonical_string, infix_prec};

const TAB_W: usize = 4; // gdformat's TAB_INDENT_SIZE — tabs count as 4 columns for length

/// A significant child of a node: either a child node or a non-trivia token (kind + text).
enum El {
    Node(GdNode),
    Tok(S, String),
}

/// The significant children of `node` in source order (trivia + synthetic layout markers dropped).
fn sig(node: &GdNode) -> Vec<El> {
    let mut v = Vec::new();
    for c in node.children_with_tokens() {
        match c {
            NodeOrToken::Node(n) => v.push(El::Node(n.clone())),
            NodeOrToken::Token(t) => {
                let k = t.kind();
                if !k.is_trivia() && !k.is_synthetic_layout() {
                    v.push(El::Tok(k, t.text().to_owned()));
                }
            }
        }
    }
    v
}

/// The significant child *nodes* of `node`.
fn child_nodes(node: &GdNode) -> Vec<GdNode> {
    node.children().map(Clone::clone).collect()
}

/// Display width of a rendered line: tabs count as [`TAB_W`], everything else as one column.
fn width(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { TAB_W } else { 1 }).sum()
}

/// The whole significant text of a node concatenated with no spacing — for atoms whose spelling is
/// verbatim (node paths `$A/B`, `%Unique`, type refs `Array[int]`). Commas get a following space.
fn node_text(node: &GdNode) -> String {
    let mut out = String::new();
    for c in node.children_with_tokens() {
        match c {
            NodeOrToken::Node(n) => out.push_str(&node_text(n)),
            NodeOrToken::Token(t) => {
                let k = t.kind();
                if k.is_trivia() || k.is_synthetic_layout() {
                    continue;
                }
                out.push_str(t.text());
                if k == S::Comma {
                    out.push(' ');
                }
            }
        }
    }
    out
}

/// Shared wrapping state for one statement.
struct W<'a> {
    cfg: &'a FmtConfig,
    unit: String,
    max: usize,
    /// The exact text that was parsed to build the tree being formatted — lets comment tokens be
    /// located by byte offset and source line so they can be re-emitted in the reshaped output.
    src: String,
}

impl W<'_> {
    fn indent(&self, level: usize) -> String {
        self.unit.repeat(level)
    }
}

/// The first significant child node of `node` with the given kind (owned).
fn find_child(node: &GdNode, kind: S) -> Option<GdNode> {
    node.children().find(|c| c.kind() == kind).cloned()
}

/// The first type-reference child node of `node` (`TypeRef`/`TypedArray`/`TypedDict`), owned.
fn find_child_ty(node: &GdNode) -> Option<GdNode> {
    node.children()
        .find(|c| matches!(c.kind(), S::TypeRef | S::TypedArray | S::TypedDict))
        .cloned()
}

/// The declared name of `node` — the identifier token inside its `Name` child (also accepts the soft
/// keywords `match`/`when`, which the grammar permits as identifiers).
fn name_of(node: &GdNode) -> Option<String> {
    let name = node.children().find(|c| c.kind() == S::Name)?;
    name.children_with_tokens().find_map(|c| match c {
        NodeOrToken::Token(t) if matches!(t.kind(), S::Ident | S::MatchKw | S::WhenKw) => {
            Some(t.text().to_owned())
        }
        _ => None,
    })
}

/// Format a flattened logical-statement `body` (no leading indent) at `indent` (in levels). Returns
/// the rendered lines joined by `\n` (each carrying its own indentation), or `None` if the construct
/// is not modelled — in which case the caller keeps its previous behaviour.
pub(crate) fn render(body: &str, indent: usize, cfg: &FmtConfig) -> Option<String> {
    if !cfg.reflow {
        return None;
    }
    // A class-level declaration (`var`/`const`/`func`/`signal`/`enum`/`@…`) parses at file scope; a
    // function-body statement (`return …`, `x = …`, a call, `if`/`for`/`while`/`match`) is invalid
    // there, so it is wrapped in a throwaway `func`. A genuine block header (ending in `:`) needs a
    // throwaway `pass` body too — but never a *property* header (`var x := v:`), which would build a
    // malformed property body the parser cannot recover (and var headers are class-level anyway).
    let class_level = starts_with_class_keyword(body);
    let block_header = body.trim_end().ends_with(':')
        && matches!(
            leading_keyword(body),
            "if" | "elif" | "else" | "for" | "while" | "match" | "func" | "class"
        );
    // `body` is at indent 0 (single physical line, or a multi-line statement already dedented by the
    // caller). For a function-body statement we re-indent *every* line one level into the throwaway
    // `func` (so a multi-line lambda body keeps its relative nesting and parses). The parsed text is
    // kept in `w.src` so comment tokens can be located by byte offset / source line.
    let parsed_src = if class_level {
        if block_header {
            format!("{body}\n\tpass\n")
        } else {
            format!("{body}\n")
        }
    } else {
        let indented = reindent_to_skipping_strings(body, 1);
        if block_header {
            format!("func __():\n{indented}\n\t\tpass\n")
        } else {
            format!("func __():\n{indented}\n")
        }
    };
    let parse = gdscript_syntax::parse(&parsed_src);
    let w = W {
        cfg,
        unit: if cfg.use_tabs {
            "\t".to_owned()
        } else {
            " ".repeat(cfg.indent_size.max(1))
        },
        max: cfg.line_width,
        src: parsed_src,
    };
    let root = parse.syntax_node();
    let container = if class_level {
        root.clone()
    } else {
        let func = root.children().find(|c| c.kind() == S::FuncDecl)?;
        func.children().find(|c| c.kind() == S::Block)?.clone()
    };
    // Collect any leading prefix annotations (`@onready var x = …`) — they prepend to the underlying
    // statement's first line, exactly as gdformat does.
    let (anns, stmt) = find_statement_with_anns(&container)?;
    let mut lines = format_statement(&w, &stmt, indent)?;
    if !anns.is_empty() {
        let prefix = anns.iter().map(node_text).collect::<Vec<_>>().join(" ");
        let first = &lines[0];
        let lead = &first[..first.len() - first.trim_start().len()];
        let prepended = format!("{lead}{prefix} {}", &first[lead.len()..]);
        // gdformat moves the annotation to its own line when prepending it would overflow. Its check
        // (`indent + len(annotations) + len(stripped_line) <= max`) does NOT count the joining space,
        // so the threshold is one past the rendered width — hence `> max + 1`.
        if width(&prepended) > w.max + 1 {
            lines.insert(0, format!("{lead}{prefix}"));
        } else {
            lines[0] = prepended;
        }
    }
    // The CST path owns only *multi-line* layout. A single-line result means the statement fits as-is;
    // we delegate it to the caller's flat path, which re-emits the body's already-correct spacing
    // verbatim (our `expr_to_str` is for the inline parts of a wrap, not a spacing oracle).
    if lines.len() == 1 {
        return None;
    }
    // A comment trailing the whole statement (`const X := {…}  # note`) sits on the rendered last line.
    if let Some(c) = statement_trailing_comment(&container, &stmt, &w.src) {
        let last = lines.last_mut()?;
        last.push_str("  ");
        last.push_str(&c);
    }
    let out = lines.join("\n");
    // Self-validate: the output must be meaning-equivalent to the body. We re-parse both in the *same*
    // context (a bare function-body statement is invalid at file scope), neutralising indentation so
    // only the token structure is compared — `meaning_preserved` then allows exactly gdformat's
    // legitimate rewrites (redundant grouping parens, trailing commas, string quotes).
    let out_norm = reindent_to(&dedent(&out), 1);
    let body_norm = reindent_to(&dedent(body), 1);
    let (mine, orig) = if class_level {
        (dedent(&out), dedent(body))
    } else {
        (
            format!("func __():\n{out_norm}\n"),
            format!("func __():\n{body_norm}\n"),
        )
    };
    if !crate::meaning_preserved(&mine, &orig) {
        return None;
    }
    // The reshape must carry *every* comment through unchanged — the meaning net treats comments as
    // trivia, so this is the dedicated guard: if any comment was dropped or duplicated, fall back to
    // the verbatim statement (which keeps all comments exactly where they were).
    if comment_multiset(&out) != comment_multiset(body) {
        return None;
    }
    Some(out)
}

/// The sorted multiset of comment texts in `s` (trailing whitespace trimmed).
fn comment_multiset(s: &str) -> Vec<String> {
    let mut v: Vec<String> = gdscript_syntax::tokenize(s)
        .into_iter()
        .filter(|t| is_comment_kind(t.kind))
        .map(|t| s[t.range].trim_end().to_owned())
        .collect();
    v.sort();
    v
}

/// A comment that trails `stmt` on the same source line, just past its code (a sibling token in
/// `container`, after the statement's last significant token).
fn statement_trailing_comment(container: &GdNode, stmt: &GdNode, src: &str) -> Option<String> {
    let (_, ce) = code_span(stmt)?;
    let end_line = line_of(src, ce.saturating_sub(1));
    let mut comments = Vec::new();
    comment_tokens(container, src, &mut comments);
    comments
        .into_iter()
        .find(|&(off, line, _)| off >= ce && line == end_line)
        .map(|(_, _, text)| text)
}

/// Strip the common leading-whitespace prefix from every non-blank line (preserves relative indent).
pub(crate) fn dedent(s: &str) -> String {
    let min = s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.bytes().take_while(|&b| b == b'\t' || b == b' ').count())
        .min()
        .unwrap_or(0);
    s.lines()
        .map(|l| if l.len() >= min { &l[min..] } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Like [`reindent_to`], but never re-indents a line that lies *inside* a multi-line string token
/// (its content is literal — adding indentation would change the string's bytes). The opening line of
/// the string still gets indented, since the prefix there precedes the `"""`.
fn reindent_to_skipping_strings(s: &str, levels: usize) -> String {
    let raw = gdscript_syntax::tokenize(s);
    let mut inside: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for t in &raw {
        if t.kind == S::String {
            let (sl, el) = (
                line_of(s, usize::from(t.range.start())),
                line_of(s, usize::from(t.range.end()).saturating_sub(1)),
            );
            inside.extend((sl + 1)..=el); // interior + closing line — never the opening line
        }
    }
    let pad = "\t".repeat(levels);
    s.lines()
        .enumerate()
        .map(|(i, l)| {
            if l.trim().is_empty() || inside.contains(&i) {
                l.to_owned()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prefix every non-blank line of `s` with `levels` tabs (relative indentation preserved).
fn reindent_to(s: &str, levels: usize) -> String {
    let pad = "\t".repeat(levels);
    s.lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first word (after an optional `static `) of `body` — its leading keyword, if any.
fn leading_keyword(body: &str) -> &str {
    let t = body.trim_start();
    let t = t.strip_prefix("static ").unwrap_or(t);
    t.split([' ', '(', ':']).next().unwrap_or("")
}

/// Whether `body` is a class-scope declaration (parses at file scope) rather than a function-body
/// statement (which must be wrapped in a `func` to parse).
fn starts_with_class_keyword(body: &str) -> bool {
    let t = body.trim_start();
    t.starts_with('@')
        || matches!(
            leading_keyword(body),
            "func" | "var" | "const" | "signal" | "enum" | "class" | "class_name" | "extends"
        )
}

/// Within `container` (the source file or a function's block), collect any leading *prefix*
/// annotations (`@onready`, `@export(…)` — which prepend to the statement they decorate) and return
/// them with the underlying statement. If only annotations are present (a standalone `@annotation`
/// line), the last one is returned as the statement itself.
fn find_statement_with_anns(container: &GdNode) -> Option<(Vec<GdNode>, GdNode)> {
    let mut anns = Vec::new();
    for c in container.children() {
        if c.kind() == S::Annotation {
            anns.push(c.clone());
        } else if is_statement_kind(c.kind()) {
            return Some((anns, c.clone()));
        }
    }
    anns.pop().map(|a| (Vec::new(), a))
}

fn is_statement_kind(k: S) -> bool {
    matches!(
        k,
        S::VarDecl
            | S::ConstDecl
            | S::FuncDecl
            | S::SignalDecl
            | S::EnumDecl
            | S::Annotation
            | S::IfStmt
            | S::ElifClause
            | S::WhileStmt
            | S::ForStmt
            | S::MatchStmt
            | S::ReturnStmt
            | S::ExprStmt
    )
}

/// `true` if any descendant is an error node — we refuse to format an un-parseable fragment.
fn has_error(node: &GdNode) -> bool {
    if node.kind() == S::ErrorNode {
        return true;
    }
    node.children().any(has_error)
}

// ---- statement dispatch ---------------------------------------------------------

fn format_statement(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    match stmt.kind() {
        S::VarDecl | S::ConstDecl => format_var_like(w, stmt, indent),
        S::FuncDecl => format_func_header(w, stmt, indent),
        S::SignalDecl => format_signal(w, stmt, indent),
        S::EnumDecl => format_enum(w, stmt, indent),
        S::Annotation => format_annotation(w, stmt, indent),
        S::IfStmt | S::ElifClause => format_branch(w, stmt, indent),
        S::WhileStmt => format_keyword_expr(w, stmt, indent, "while ", ":", S::WhileKw),
        S::MatchStmt => format_keyword_expr(w, stmt, indent, "match ", ":", S::MatchKw),
        S::ForStmt => format_for(w, stmt, indent),
        S::ReturnStmt => format_return(w, stmt, indent),
        S::ExprStmt => {
            let expr = first_expr_child(stmt)?;
            if has_error(&expr) {
                return None;
            }
            format_expression(w, &expr, indent, "", "")
        }
        _ => None,
    }
}

/// Format a statement INCLUDING its nested block bodies — used inside a lambda body, where the
/// wrapper owns the whole construct (unlike a top-level statement, whose body is a sequence of
/// separate logical lines that reflow handles). Returns `None` for a compound we cannot fully render,
/// so the enclosing lambda render falls back to verbatim.
fn format_full_statement(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    match stmt.kind() {
        S::IfStmt => {
            let mut out = format_branch(w, stmt, indent)?;
            out.extend(format_block_body(
                w,
                &find_child(stmt, S::Block)?,
                indent + 1,
            )?);
            for clause in stmt.children() {
                match clause.kind() {
                    S::ElifClause => {
                        out.extend(format_branch(w, clause, indent)?);
                        out.extend(format_block_body(
                            w,
                            &find_child(clause, S::Block)?,
                            indent + 1,
                        )?);
                    }
                    S::ElseClause => {
                        out.push(format!("{}else:", w.indent(indent)));
                        out.extend(format_block_body(
                            w,
                            &find_child(clause, S::Block)?,
                            indent + 1,
                        )?);
                    }
                    _ => {}
                }
            }
            Some(out)
        }
        S::ForStmt | S::WhileStmt => {
            let mut out = format_statement(w, stmt, indent)?;
            out.extend(format_block_body(
                w,
                &find_child(stmt, S::Block)?,
                indent + 1,
            )?);
            Some(out)
        }
        // `match` arms are not modelled here — bail so the lambda render falls back to verbatim.
        S::MatchStmt => None,
        _ => format_statement(w, stmt, indent),
    }
}

// ---- comments ------------------------------------------------------------------

/// Whether `k` is a comment token (line / doc / `#region` marker) that must survive a reshape.
fn is_comment_kind(k: S) -> bool {
    matches!(
        k,
        S::LineComment | S::DocComment | S::RegionComment | S::EndRegionComment
    )
}

/// 0-based source line of byte offset `off`.
fn line_of(src: &str, off: usize) -> usize {
    src[..off.min(src.len())].matches('\n').count()
}

/// Every comment token in `node`'s subtree, in source order: `(offset, line, text)` (text trimmed).
fn comment_tokens(node: &GdNode, src: &str, out: &mut Vec<(usize, usize, String)>) {
    for c in node.children_with_tokens() {
        match c {
            NodeOrToken::Node(n) => comment_tokens(n, src, out),
            NodeOrToken::Token(t) if is_comment_kind(t.kind()) => {
                let off = usize::from(t.text_range().start());
                out.push((off, line_of(src, off), t.text().trim_end().to_owned()));
            }
            NodeOrToken::Token(_) => {}
        }
    }
}

/// The first..last significant (non-trivia, non-synthetic) byte offsets of `node` — its code span,
/// excluding leading / trailing comment or whitespace trivia.
fn code_span(node: &GdNode) -> Option<(usize, usize)> {
    fn rec(node: &GdNode, first: &mut Option<usize>, last: &mut usize) {
        for c in node.children_with_tokens() {
            match c {
                NodeOrToken::Node(n) => rec(n, first, last),
                NodeOrToken::Token(t) => {
                    if t.kind().is_trivia() || t.kind().is_synthetic_layout() {
                        continue;
                    }
                    let r = t.text_range();
                    if first.is_none() {
                        *first = Some(usize::from(r.start()));
                    }
                    *last = usize::from(r.end());
                }
            }
        }
    }
    let mut first = None;
    let mut last = 0usize;
    rec(node, &mut first, &mut last);
    first.map(|f| (f, last))
}

/// The comments inside a block, partitioned for re-emission around its statements.
struct BlockComments {
    /// `before[i]` — standalone comment lines that precede statement `i` (each at the block indent).
    before: Vec<Vec<String>>,
    /// `trailing[i]` — an inline comment that trails statement `i` on the same source line.
    trailing: Vec<Option<String>>,
    /// Standalone comment lines after the last statement.
    tail: Vec<String>,
}

/// Partition a block's comments into per-statement leading / trailing / tail buckets. A comment that
/// falls *inside* a statement's own code span is left to that statement's recursive formatting (a
/// compound body) or to the meaning net (an in-expression comment → verbatim fallback).
fn collect_block_comments(block: &GdNode, src: &str, stmts: &[GdNode]) -> BlockComments {
    let n = stmts.len();
    let spans: Vec<Option<(usize, usize)>> = stmts.iter().map(code_span).collect();
    let mut bc = BlockComments {
        before: vec![Vec::new(); n],
        trailing: vec![None; n],
        tail: Vec::new(),
    };
    let mut comments = Vec::new();
    comment_tokens(block, src, &mut comments);
    for (off, line, text) in comments {
        if spans
            .iter()
            .flatten()
            .any(|&(cs, ce)| cs <= off && off < ce)
        {
            continue; // inside a statement — handled elsewhere
        }
        // The last statement whose code ends before this comment, if on the same source line → trail.
        let prev = spans
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, s)| s.filter(|&(_, ce)| ce <= off).map(|(_, ce)| (i, ce)));
        if let Some((i, _)) = prev.filter(|&(_, ce)| line_of(src, ce.saturating_sub(1)) == line) {
            bc.trailing[i] = Some(text);
            continue;
        }
        // Otherwise standalone: it precedes the next statement, or trails the whole block.
        match spans.iter().position(|s| s.is_some_and(|(cs, _)| cs > off)) {
            Some(nx) => bc.before[nx].push(text),
            None => bc.tail.push(text),
        }
    }
    bc
}

/// Format every statement of a block (a lambda / compound body) fully, in order, re-emitting the
/// block's comments: a standalone comment on its own indented line, an inline comment appended to its
/// statement's last line with gdformat's two-space offset.
fn format_block_body(w: &W, block: &GdNode, indent: usize) -> Option<Vec<String>> {
    let stmts: Vec<GdNode> = block.children().cloned().collect();
    let bc = collect_block_comments(block, &w.src, &stmts);
    let mut out = Vec::new();
    for (i, stmt) in stmts.iter().enumerate() {
        for c in &bc.before[i] {
            out.push(format!("{}{c}", w.indent(indent)));
        }
        let mut lines = format_full_statement(w, stmt, indent)?;
        if let Some(c) = &bc.trailing[i] {
            let last = lines.last_mut()?;
            last.push_str("  ");
            last.push_str(c);
        }
        out.append(&mut lines);
    }
    for c in &bc.tail {
        out.push(format!("{}{c}", w.indent(indent)));
    }
    Some(out)
}

/// The first child node that is an expression kind.
fn first_expr_child(node: &GdNode) -> Option<GdNode> {
    node.children().find(|c| is_expr_kind(c.kind())).cloned()
}

fn is_expr_kind(k: S) -> bool {
    matches!(
        k,
        S::BinExpr
            | S::UnaryExpr
            | S::TernaryExpr
            | S::CastExpr
            | S::IsExpr
            | S::InExpr
            | S::CallExpr
            | S::IndexExpr
            | S::FieldExpr
            | S::AwaitExpr
            | S::LambdaExpr
            | S::ParenExpr
            | S::ArrayLit
            | S::DictLit
            | S::NameRef
            | S::Literal
            | S::GetNodeExpr
            | S::UniqueNodeExpr
            | S::PreloadExpr
    )
}

fn format_var_like(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    // Property bodies (get/set) are not modelled — let the caller handle them.
    if stmt.children().any(|c| c.kind() == S::PropertyBody) {
        return None;
    }
    let els = sig(stmt);
    // Find the initializer expression and the assignment operator that precedes it.
    let mut assign: Option<&str> = None;
    let mut ty: Option<String> = None;
    let mut is_static = false;
    let mut expr: Option<GdNode> = None;
    let kw = if stmt.kind() == S::ConstDecl {
        "const"
    } else {
        "var"
    };
    for el in &els {
        match el {
            El::Tok(S::StaticKw, _) => is_static = true,
            El::Tok(S::Eq, _) => assign = Some("="),
            El::Tok(S::ColonEq, _) => assign = Some(":="),
            El::Node(n) if matches!(n.kind(), S::TypeRef | S::TypedArray | S::TypedDict) => {
                ty = Some(node_text(n));
            }
            El::Node(n) if is_expr_kind(n.kind()) => expr = Some(n.clone()),
            _ => {}
        }
    }
    let name = name_of(stmt)?;
    let (Some(expr), Some(assign)) = (expr, assign) else {
        // No initializer to fold — a plain single line.
        return None;
    };
    if has_error(&expr) {
        return None;
    }
    let mut prefix = String::new();
    if is_static {
        prefix.push_str("static ");
    }
    prefix.push_str(kw);
    prefix.push(' ');
    prefix.push_str(&name);
    if let Some(ty) = ty {
        prefix.push_str(": ");
        prefix.push_str(&ty);
    }
    prefix.push(' ');
    prefix.push_str(assign);
    prefix.push(' ');
    format_expression(w, &expr, indent, &prefix, "")
}

fn format_func_header(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    let mut is_static = false;
    let mut params: Option<GdNode> = None;
    let mut ret: Option<String> = None;
    for el in &sig(stmt) {
        match el {
            El::Tok(S::StaticKw, _) => is_static = true,
            El::Node(n) if n.kind() == S::ParamList => params = Some(n.clone()),
            El::Node(n) if matches!(n.kind(), S::TypeRef | S::TypedArray | S::TypedDict) => {
                ret = Some(node_text(n));
            }
            _ => {}
        }
    }
    let name = name_of(stmt)?;
    let params = params?;
    let mut prefix = String::new();
    if is_static {
        prefix.push_str("static ");
    }
    prefix.push_str("func ");
    prefix.push_str(&name);
    let suffix = match ret {
        Some(t) => format!(" -> {t}:"),
        None => ":".to_owned(),
    };
    // The parameter list folds as a `(`-delimited comma list.
    format_paren_list(w, &params, indent, &prefix, &suffix, "(", ")")
}

fn format_signal(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    let name = name_of(stmt)?;
    let params = find_child(stmt, S::ParamList)?;
    let prefix = format!("signal {name}");
    format_paren_list(w, &params, indent, &prefix, "", "(", ")")
}

fn format_annotation(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    let name = sig(stmt).iter().find_map(|el| match el {
        El::Tok(S::Ident, t) => Some(t.clone()),
        _ => None,
    })?;
    let args = find_child(stmt, S::AnnotationArgList)?;
    let prefix = format!("@{name}");
    format_paren_list(w, &args, indent, &prefix, "", "(", ")")
}

fn format_branch(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    let kw = if stmt.kind() == S::ElifClause {
        "elif "
    } else {
        "if "
    };
    let expr = first_expr_child(stmt)?;
    if has_error(&expr) {
        return None;
    }
    format_expression(w, &expr, indent, kw, ":")
}

fn format_keyword_expr(
    w: &W,
    stmt: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
    _kw: S,
) -> Option<Vec<String>> {
    let expr = first_expr_child(stmt)?;
    if has_error(&expr) {
        return None;
    }
    format_expression(w, &expr, indent, prefix, suffix)
}

fn format_for(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    // `for NAME (: TYPE)? in EXPR:` — the iterable is the expression after `in`.
    let name = name_of(stmt)?;
    let ty = find_child_ty(stmt).map(|n| node_text(&n));
    let expr = first_expr_child(stmt)?;
    if has_error(&expr) {
        return None;
    }
    let prefix = match ty {
        Some(t) => format!("for {name}: {t} in "),
        None => format!("for {name} in "),
    };
    format_expression(w, &expr, indent, &prefix, ":")
}

fn format_return(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    let Some(expr) = first_expr_child(stmt) else {
        return None; // bare `return` — single line
    };
    if has_error(&expr) {
        return None;
    }
    format_expression(w, &expr, indent, "return ", "")
}

fn format_enum(w: &W, stmt: &GdNode, indent: usize) -> Option<Vec<String>> {
    // `enum (NAME)? { VARIANTS }` — variants fold as a `{ `-delimited comma list (enum-brace spacing).
    let name = name_of(stmt);
    let prefix = match name {
        Some(n) => format!("enum {n} "),
        None => "enum ".to_owned(),
    };
    let variants: Vec<GdNode> = stmt
        .children()
        .filter(|c| c.kind() == S::EnumVariant)
        .cloned()
        .collect();
    let elements: Vec<String> = variants
        .iter()
        .map(|v| enum_variant_str(w, v))
        .collect::<Option<Vec<_>>>()?;
    let magic = has_trailing_comma(stmt);
    let flat = format!(
        "{}{}{{ {} }}",
        w.indent(indent),
        prefix,
        elements.join(", ")
    );
    if !magic && width(&flat) <= w.max {
        return Some(vec![flat]);
    }
    // Exploded: one variant per line, with a trailing comma (gdformat adds it for enum bodies).
    let mut out = vec![format!("{}{}{{", w.indent(indent), prefix)];
    let inner = w.indent(indent + 1);
    for e in &elements {
        out.push(format!("{inner}{e},"));
    }
    out.push(format!("{}}}", w.indent(indent)));
    Some(out)
}

/// A single enum variant: `NAME` or `NAME = value` (the `=` is spaced, unlike a tight `node_text`).
fn enum_variant_str(w: &W, variant: &GdNode) -> Option<String> {
    let name = variant.children_with_tokens().find_map(|c| match c {
        NodeOrToken::Token(t) if matches!(t.kind(), S::Ident | S::MatchKw | S::WhenKw) => {
            Some(t.text().to_owned())
        }
        _ => None,
    })?;
    match first_expr_child(variant) {
        Some(val) => Some(format!("{name} = {}", expr_to_str(w, &val)?)),
        None => Some(name),
    }
}

// ---- expression formatting (gdformat's format_expression) -----------------------

/// `_format_standalone_expression` → strip redundant outer parens, then format concrete.
fn format_expression(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let node = remove_outer_parens(node);
    format_concrete(w, &node, indent, prefix, suffix)
}

fn remove_outer_parens(node: &GdNode) -> GdNode {
    let mut n = node.clone();
    while n.kind() == S::ParenExpr {
        match child_nodes(&n).into_iter().find(|c| is_expr_kind(c.kind())) {
            Some(inner) => n = inner,
            None => break,
        }
    }
    n
}

fn format_concrete(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    if is_multiline_string(node) {
        return multiline_string_lines(w, node, indent, prefix, suffix);
    }
    if is_foldable(node) {
        format_foldable(w, node, indent, prefix, suffix)
    } else {
        Some(vec![single_line(w, node, indent, prefix, suffix)?])
    }
}

/// The verbatim text of a `Literal`'s string token, if it is a multi-line (`"""…"""`) string.
fn multiline_string_text(node: &GdNode) -> Option<String> {
    if node.kind() != S::Literal {
        return None;
    }
    sig(node).into_iter().find_map(|el| match el {
        El::Tok(S::String, t) if t.contains('\n') => Some(t),
        _ => None,
    })
}

fn is_multiline_string(node: &GdNode) -> bool {
    multiline_string_text(node).is_some()
}

/// Render a multi-line string as gdformat's `_format_string_to_multiple_lines`: the first physical
/// line carries the indent and prefix, the interior lines are verbatim (the literal content), and the
/// closing line carries the suffix — keeping the string's bytes exactly.
fn multiline_string_lines(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let text = multiline_string_text(node)?;
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() < 2 {
        return None;
    }
    let mut out = vec![format!("{}{}{}", w.indent(indent), prefix, lines[0])];
    out.extend(lines[1..lines.len() - 1].iter().map(|l| (*l).to_owned()));
    out.push(format!("{}{}", lines[lines.len() - 1], suffix));
    Some(out)
}

fn single_line(w: &W, node: &GdNode, indent: usize, prefix: &str, suffix: &str) -> Option<String> {
    Some(format!(
        "{}{}{}{}",
        w.indent(indent),
        prefix,
        expr_to_str(w, node)?,
        suffix
    ))
}

fn format_foldable(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    // A *standalone* comment inside `node` forces it (and every enclosing construct) multi-line — a
    // single-line render would have nowhere to put it (gdformat's `_has_standalone_comments`). An
    // inline comment that merely trails the construct does not force it.
    if forcing_multiline(node) || node_has_standalone_comment(node, &w.src) {
        return explode(w, node, indent, prefix, suffix);
    }
    let line = single_line(w, node, indent, prefix, suffix)?;
    if width(&line) <= w.max {
        return Some(vec![line]);
    }
    explode(w, node, indent, prefix, suffix)
}

/// A node is *foldable* (splittable across lines) unless it is an atomic leaf.
fn is_foldable(node: &GdNode) -> bool {
    !matches!(
        node.kind(),
        S::Literal | S::NameRef | S::GetNodeExpr | S::UniqueNodeExpr
    )
}

/// gdformat forces multiple lines when a magic trailing comma is present anywhere, or for a lambda
/// whose body is multi-statement or a single *compound* statement (`_is_multistatement_lambda` /
/// `_is_unistatement_lambda_with_compound_statement`).
fn forcing_multiline(node: &GdNode) -> bool {
    if is_multiline_string(node) {
        return true; // gdformat's `_is_multiline_string` — a `"""…"""` forces every enclosing construct
    }
    if node.kind() == S::LambdaExpr && lambda_forces_multiline(node) {
        return true;
    }
    has_trailing_comma(node) || node.children().any(forcing_multiline)
}

/// Whether `node`'s subtree contains a lambda (gdformat's `expression_contains_lambda`).
fn node_contains_lambda(node: &GdNode) -> bool {
    node.kind() == S::LambdaExpr || node.children().any(node_contains_lambda)
}

/// Whether `node` has a *standalone* comment strictly *inside* its code span — one that begins its
/// source line (only whitespace precedes it) and sits between the node's first and last significant
/// tokens. gdformat forces a construct multi-line for such an interior comment, but a comment that
/// merely leads the node (a sibling's standalone comment) or trails it does not force *this* node.
fn node_has_standalone_comment(node: &GdNode, src: &str) -> bool {
    let Some((cs, ce)) = code_span(node) else {
        return false;
    };
    let mut comments = Vec::new();
    comment_tokens(node, src, &mut comments);
    comments.iter().any(|&(off, _, _)| {
        cs < off && off < ce && {
            let line_start = src[..off].rfind('\n').map_or(0, |i| i + 1);
            src[line_start..off].trim().is_empty()
        }
    })
}

/// A lambda body forces multiple lines: more than one statement, or a single compound statement.
fn lambda_forces_multiline(node: &GdNode) -> bool {
    let Some(block) = find_child(node, S::Block) else {
        return false;
    };
    let stmts: Vec<GdNode> = block.children().cloned().collect();
    match stmts.as_slice() {
        [one] => matches!(
            one.kind(),
            S::IfStmt | S::ElifClause | S::ElseClause | S::ForStmt | S::WhileStmt | S::MatchStmt
        ),
        _ => true,
    }
}

/// `true` if the bracket list `node` ends with a trailing comma (magic comma).
fn has_trailing_comma(node: &GdNode) -> bool {
    if !matches!(
        node.kind(),
        S::ArgList | S::ArrayLit | S::DictLit | S::ParamList | S::AnnotationArgList | S::EnumDecl
    ) {
        return false;
    }
    let mut last_significant: Option<S> = None;
    for c in node.children_with_tokens() {
        match c {
            NodeOrToken::Node(_) => last_significant = Some(S::Tombstone), // a real element
            NodeOrToken::Token(t) => {
                let k = t.kind();
                if k.is_trivia() || k.is_synthetic_layout() {
                    continue;
                }
                if is_close_bracket(k) {
                    return last_significant == Some(S::Comma);
                }
                last_significant = Some(k);
            }
        }
    }
    false
}

fn is_close_bracket(k: S) -> bool {
    matches!(k, S::RParen | S::RBrack | S::RBrace)
}

// ---- explode (gdformat's _format_foldable_to_multiple_lines dispatch) -----------

fn explode(w: &W, node: &GdNode, indent: usize, prefix: &str, suffix: &str) -> Option<Vec<String>> {
    match node.kind() {
        S::BinExpr => {
            let op = bin_op(node)?;
            if is_assign_op(op) {
                format_assignment(w, node, indent, prefix, suffix)
            } else {
                format_operator_chain(w, node, indent, prefix, suffix)
            }
        }
        S::TernaryExpr | S::IsExpr | S::CastExpr | S::InExpr => {
            format_operator_chain(w, node, indent, prefix, suffix)
        }
        S::UnaryExpr => format_unary(w, node, indent, prefix, suffix),
        S::AwaitExpr => format_await(w, node, indent, prefix, suffix),
        S::CallExpr => format_call(w, node, indent, prefix, suffix),
        S::PreloadExpr => format_preload(w, node, indent, prefix, suffix),
        S::IndexExpr => format_index(w, node, indent, prefix, suffix),
        S::FieldExpr => format_dot_chain(w, node, indent, prefix, suffix),
        S::ArrayLit => format_paren_list(w, node, indent, prefix, suffix, "[", "]"),
        S::DictLit => format_paren_list(w, node, indent, prefix, suffix, "{", "}"),
        S::ParenExpr => {
            let inner = child_nodes(node)
                .into_iter()
                .find(|c| is_expr_kind(c.kind()))?;
            format_expression(
                w,
                &inner,
                indent,
                &format!("{prefix}("),
                &format!("){suffix}"),
            )
        }
        S::DictEntry => format_dict_entry(w, node, indent, prefix, suffix),
        S::LambdaExpr => format_lambda(w, node, indent, prefix, suffix),
        _ => None,
    }
}

/// gdformat's `_format_lambda_to_multiple_lines`: the header on the first line, the body block
/// indented one level below, and the `suffix` appended to the body's last line.
fn format_lambda(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let header = lambda_header_str(w, node)?;
    let mut out = vec![format!("{}{prefix}{header}", w.indent(indent))];
    let block = find_child(node, S::Block)?;
    out.extend(format_block_body(w, &block, indent + 1)?);
    out.last_mut()?.push_str(suffix);
    Some(out)
}

/// gdformat's `_format_kv_pair_to_multiple_lines`: the key keeps its `:`/` =` on its own line(s);
/// the value is formatted below it at the same indent (so a multi-line value's open bracket drops to
/// the next line — `player =` / `{ … }`).
/// Decompose a dict entry into `(key, value, is_colon)`. A lua-style `key = value` is parsed with the
/// `=` absorbed into a single assignment `BinExpr` child (since `=` is an infix operator), so that is
/// unwrapped here; a colon entry `key: value` has the key and value as separate children.
fn dict_entry_kv(node: &GdNode) -> Option<(GdNode, GdNode, bool)> {
    let nodes = child_nodes(node);
    if nodes.len() == 1 && nodes[0].kind() == S::BinExpr && bin_op(&nodes[0]) == Some(S::Eq) {
        let bn = child_nodes(&nodes[0]);
        return Some((bn.first()?.clone(), bn.get(1)?.clone(), false));
    }
    let colon = sig(node).iter().any(|e| matches!(e, El::Tok(S::Colon, _)));
    Some((nodes.first()?.clone(), nodes.get(1)?.clone(), colon))
}

fn format_dict_entry(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let (key, val, colon) = dict_entry_kv(node)?;
    let key_suffix = if colon { ":" } else { " =" };
    let mut out = format_expression(w, &key, indent, prefix, key_suffix)?;
    out.extend(format_expression(w, &val, indent, "", suffix)?);
    Some(out)
}

fn bin_op(node: &GdNode) -> Option<S> {
    sig(node).into_iter().find_map(|el| match el {
        El::Tok(k, _) if infix_prec(k).is_some() || is_assign_op(k) => Some(k),
        _ => None,
    })
}

fn is_assign_op(k: S) -> bool {
    matches!(
        k,
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

/// `lhs OP rhs` where OP assigns — prefix becomes `…lhs OP ` and the RHS is formatted.
fn format_assignment(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let nodes = child_nodes(node);
    let lhs = nodes.first()?;
    let rhs = nodes.get(1)?;
    let op = bin_op(node)?;
    let new_prefix = format!("{prefix}{} {} ", expr_to_str(w, lhs)?, op_str(op));
    format_concrete(w, rhs, indent, &new_prefix, suffix)
}

fn op_str(k: S) -> &'static str {
    match k {
        S::Eq => "=",
        S::PlusEq => "+=",
        S::MinusEq => "-=",
        S::StarEq => "*=",
        S::SlashEq => "/=",
        S::StarStarEq => "**=",
        S::PercentEq => "%=",
        S::AmpEq => "&=",
        S::PipeEq => "|=",
        S::CaretEq => "^=",
        S::ShlEq => "<<=",
        S::ShrEq => ">>=",
        S::ColonEq => ":=",
        _ => "",
    }
}

/// An operand of an operator chain: a node, or pre-rendered text (a type ref for `is`/`as`).
enum Operand {
    Node(GdNode),
    Text(String),
}

/// Decompose a chain expression into its operand sequence and the operator string before each
/// non-first operand. Same-precedence binary operators are flattened into one n-ary chain (matching
/// gdformat's grammar, which keeps `a + b + c` as a single node).
fn chain_parts(node: &GdNode) -> Option<(Vec<Operand>, Vec<String>)> {
    match node.kind() {
        S::BinExpr => {
            let op = bin_op(node)?;
            let prec = infix_prec(op)?;
            let nodes = child_nodes(node);
            let lhs = nodes.first()?.clone();
            let rhs = nodes.get(1)?.clone();
            let (mut operands, mut ops) =
                if lhs.kind() == S::BinExpr && bin_op(&lhs).and_then(infix_prec) == Some(prec) {
                    chain_parts(&lhs)?
                } else {
                    (vec![Operand::Node(lhs)], Vec::new())
                };
            ops.push(op_str_bin(op).to_owned());
            operands.push(Operand::Node(rhs));
            Some((operands, ops))
        }
        S::TernaryExpr => {
            // `a if c else b`
            let nodes = child_nodes(node);
            let a = nodes.first()?.clone();
            let c = nodes.get(1)?.clone();
            let b = nodes.get(2)?.clone();
            Some((
                vec![Operand::Node(a), Operand::Node(c), Operand::Node(b)],
                vec!["if".to_owned(), "else".to_owned()],
            ))
        }
        S::InExpr => {
            let nodes = child_nodes(node);
            let a = nodes.first()?.clone();
            let b = nodes.get(1)?.clone();
            let not = sig(node).iter().any(|e| matches!(e, El::Tok(S::NotKw, _)));
            let op = if not { "not in" } else { "in" };
            Some((
                vec![Operand::Node(a), Operand::Node(b)],
                vec![op.to_owned()],
            ))
        }
        S::IsExpr => {
            let nodes = child_nodes(node);
            let a = nodes.first()?.clone();
            let ty = node
                .children()
                .find(|c| matches!(c.kind(), S::TypeRef | S::TypedArray | S::TypedDict))?;
            let not = sig(node).iter().any(|e| matches!(e, El::Tok(S::NotKw, _)));
            let op = if not { "is not" } else { "is" };
            Some((
                vec![Operand::Node(a), Operand::Text(node_text(ty))],
                vec![op.to_owned()],
            ))
        }
        S::CastExpr => {
            let nodes = child_nodes(node);
            let a = nodes.first()?.clone();
            let ty = node
                .children()
                .find(|c| matches!(c.kind(), S::TypeRef | S::TypedArray | S::TypedDict))?;
            Some((
                vec![Operand::Node(a), Operand::Text(node_text(ty))],
                vec!["as".to_owned()],
            ))
        }
        _ => None,
    }
}

fn op_str_bin(k: S) -> &'static str {
    match k {
        S::Plus => "+",
        S::Minus => "-",
        S::Star => "*",
        S::Slash => "/",
        S::Percent => "%",
        S::StarStar => "**",
        S::EqEq => "==",
        S::Neq => "!=",
        S::Lt => "<",
        S::Gt => ">",
        S::Le => "<=",
        S::Ge => ">=",
        S::AndKw => "and",
        S::OrKw => "or",
        S::AmpAmp => "&&",
        S::PipePipe => "||",
        S::Pipe => "|",
        S::Caret => "^",
        S::Amp => "&",
        S::Shl => "<<",
        S::Shr => ">>",
        S::InKw => "in",
        _ => "",
    }
}

/// gdformat's `_format_operator_chain_based_expression_to_multiple_lines`: wrap in injected parens
/// (unless already directly inside a bracket), render the chain operator-leading one per line.
fn format_operator_chain(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let (operands, ops) = chain_parts(node)?;
    let inside_par = (prefix.ends_with('(') && suffix.starts_with(')'))
        || (prefix.ends_with('[') && suffix.starts_with(']'));
    let (lpar, rpar) = if inside_par { ("", "") } else { ("(", ")") };
    let mut out = vec![format!("{}{}{}", w.indent(indent), prefix, lpar)];
    let child = indent + 1;
    // Compact first: the whole chain on one continuation line, if it fits (gdformat renders the
    // bracketed chain via `_format_foldable`, which tries single-line before exploding per operator).
    if let Some(flat) = expr_to_str(w, node) {
        let compact = format!("{}{}", w.indent(child), flat);
        if !forcing_multiline(node) && width(&compact) <= w.max {
            out.push(compact);
            out.push(format!("{}{}{}", w.indent(indent), rpar, suffix));
            return Some(out);
        }
    }
    // Otherwise explode operator-leading, one operand per line.
    out.extend(format_operand(w, &operands[0], child, "", "")?);
    for (i, op) in ops.iter().enumerate() {
        let pfx = format!("{op} ");
        out.extend(format_operand(w, &operands[i + 1], child, &pfx, "")?);
    }
    out.push(format!("{}{}{}", w.indent(indent), rpar, suffix));
    Some(out)
}

fn format_operand(
    w: &W,
    operand: &Operand,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    match operand {
        Operand::Node(n) => format_concrete(w, n, indent, prefix, suffix),
        Operand::Text(t) => Some(vec![format!(
            "{}{}{}{}",
            w.indent(indent),
            prefix,
            t,
            suffix
        )]),
    }
}

fn format_unary(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let op = sig(node).into_iter().find_map(|el| match el {
        El::Tok(k @ (S::NotKw | S::Bang | S::Minus | S::Plus | S::Tilde), _) => Some(k),
        _ => None,
    })?;
    let operand = first_expr_child(node)?;
    let spacing = if op == S::NotKw { " " } else { "" };
    let op_txt = match op {
        S::NotKw => "not",
        S::Bang => "!",
        S::Minus => "-",
        S::Plus => "+",
        S::Tilde => "~",
        _ => return None,
    };
    format_concrete(
        w,
        &operand,
        indent,
        &format!("{prefix}{op_txt}{spacing}"),
        suffix,
    )
}

fn format_await(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let operand = first_expr_child(node)?;
    format_concrete(w, &operand, indent, &format!("{prefix}await "), suffix)
}

/// gdformat's `_format_call_expression_to_multiple_lines`: `callee(` … `)`, args as a comma list. If
/// the callee is itself a chain (method call), defer to the dot-chain handler.
fn format_call(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let nodes = child_nodes(node);
    let callee = nodes.first()?;
    if matches!(callee.kind(), S::FieldExpr | S::IndexExpr | S::CallExpr) {
        return format_dot_chain(w, node, indent, prefix, suffix);
    }
    let arglist = find_child(node, S::ArgList)?;
    let callee_str = expr_to_str(w, callee)?;
    let elems = list_elements(&arglist);
    if elems.is_empty() {
        return Some(vec![format!(
            "{}{}{}(){}",
            w.indent(indent),
            prefix,
            callee_str,
            suffix
        )]);
    }
    let new_prefix = format!("{prefix}{callee_str}(");
    let new_suffix = format!("){suffix}");
    format_comma_list(w, &arglist, indent, &new_prefix, &new_suffix)
}

fn format_preload(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let arglist = find_child(node, S::ArgList)?;
    let new_prefix = format!("{prefix}preload(");
    let new_suffix = format!("){suffix}");
    format_comma_list(w, &arglist, indent, &new_prefix, &new_suffix)
}

fn format_index(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    // `obj[idx]` — render the subscriptee flat, fold the subscript.
    let nodes = child_nodes(node);
    let obj = nodes.first()?;
    let idx = nodes.get(1)?;
    // A subscript on a chain (`a.b(…)["k"]`) is part of that dot-chain: gdformat collapses it and
    // wraps bottom-up, or leading-dot when the flat subscriptee would overflow. Delegate so we get the
    // same choice (mirrors `format_call`'s delegation for a chained callee).
    if matches!(obj.kind(), S::FieldExpr | S::CallExpr | S::IndexExpr) {
        return format_dot_chain(w, node, indent, prefix, suffix);
    }
    let obj_str = expr_to_str(w, obj)?;
    format_expression(
        w,
        idx,
        indent,
        &format!("{prefix}{obj_str}["),
        &format!("]{suffix}"),
    )
}

/// A `(`/`[`/`{`-delimited comma list whose open token is supplied (param lists, annotation args).
fn format_paren_list(
    w: &W,
    list_node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
    open: &str,
    close: &str,
) -> Option<Vec<String>> {
    let elems = list_elements(list_node);
    let magic = has_trailing_comma(list_node);
    // Single line when the whole construct fits and no magic comma forces it open (gdformat's
    // `_format_foldable`: try single-line first, explode only on overflow).
    if !magic && !elems.iter().any(forcing_multiline) {
        let flat_elems = elems
            .iter()
            .map(|e| expr_to_str(w, e))
            .collect::<Option<Vec<_>>>()?;
        let flat = format!(
            "{}{}{}{}{}{}",
            w.indent(indent),
            prefix,
            open,
            flat_elems.join(", "),
            close,
            suffix
        );
        if width(&flat) <= w.max {
            return Some(vec![flat]);
        }
    }
    if elems.is_empty() {
        return Some(vec![format!(
            "{}{}{}{}{}",
            w.indent(indent),
            prefix,
            open,
            close,
            suffix
        )]);
    }
    format_comma_list(
        w,
        list_node,
        indent,
        &format!("{prefix}{open}"),
        &format!("{close}{suffix}"),
    )
}

/// gdformat's `_format_comma_separated_list_to_multiple_lines`: open line, then the elements as a
/// `contextless_comma_separated_list` one indent deeper (compact when they fit, one-per-line when not),
/// then the close line.
fn format_comma_list(
    w: &W,
    list_node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let elems = list_elements(list_node);
    let magic = has_trailing_comma(list_node);
    let lc = collect_list_comments(list_node, &w.src, &elems);
    let mut out = vec![format!("{}{}", w.indent(indent), prefix)];
    let child = indent + 1;
    // Compact: all elements on one continuation line, if they fit, no magic comma, and no comment to
    // place (a comment cannot sit on a compact line). When an element cannot render inline (a
    // multi-line lambda body), the compact form is unavailable — fall through to the exploded form.
    let compact = (!magic && !lc.any() && !elems.iter().any(forcing_multiline))
        .then(|| {
            elems
                .iter()
                .map(|e| expr_to_str(w, e))
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.join(", "))
        })
        .flatten()
        .map(|flat| format!("{}{}", w.indent(child), flat))
        .filter(|line| width(line) <= w.max);
    if let Some(compact) = compact {
        out.push(compact);
        out.push(format!("{}{}", w.indent(indent), suffix));
        return Some(out);
    }
    // A comment trailing the open bracket (`[  # …`) sits on the open line.
    if let Some(c) = &lc.after_open {
        let last = out.last_mut()?;
        last.push_str("  ");
        last.push_str(c);
    }
    // Exploded: one element per line, with its surrounding comments.
    let n = elems.len();
    for (i, e) in elems.iter().enumerate() {
        for c in &lc.before[i] {
            out.push(format!("{}{c}", w.indent(child)));
        }
        let elem_suffix = if i + 1 < n || magic { "," } else { "" };
        let mut lines = format_expression(w, e, child, "", elem_suffix)?;
        if let Some(c) = &lc.trailing[i] {
            let last = lines.last_mut()?;
            last.push_str("  ");
            last.push_str(c);
        }
        out.append(&mut lines);
    }
    for c in &lc.tail {
        out.push(format!("{}{c}", w.indent(child)));
    }
    out.push(format!("{}{}", w.indent(indent), suffix));
    Some(out)
}

/// The comments inside a bracket list, partitioned for re-emission around its elements.
struct ListComments {
    /// A comment trailing the open bracket (`[  # …`).
    after_open: Option<String>,
    /// `before[i]` — standalone comment lines that precede element `i`.
    before: Vec<Vec<String>>,
    /// `trailing[i]` — an inline comment that trails element `i` on the same source line.
    trailing: Vec<Option<String>>,
    /// Standalone comment lines after the last element, before the close bracket.
    tail: Vec<String>,
}

impl ListComments {
    fn any(&self) -> bool {
        self.after_open.is_some()
            || self.before.iter().any(|v| !v.is_empty())
            || self.trailing.iter().any(Option::is_some)
            || !self.tail.is_empty()
    }
}

/// Partition a bracket list's comments into open-bracket / per-element leading / trailing / tail
/// buckets. A comment inside an element's own code span is left to that element's recursive
/// formatting (a nested list / lambda) or to the meaning net (an in-leaf comment → verbatim).
fn collect_list_comments(list_node: &GdNode, src: &str, elems: &[GdNode]) -> ListComments {
    let n = elems.len();
    let mut lc = ListComments {
        after_open: None,
        before: vec![Vec::new(); n],
        trailing: vec![None; n],
        tail: Vec::new(),
    };
    let Some((open_off, _)) = code_span(list_node) else {
        return lc;
    };
    let open_line = line_of(src, open_off);
    let spans: Vec<Option<(usize, usize)>> = elems.iter().map(code_span).collect();
    let first_start = spans.iter().flatten().map(|&(cs, _)| cs).min();
    let mut comments = Vec::new();
    comment_tokens(list_node, src, &mut comments);
    for (off, line, text) in comments {
        if spans
            .iter()
            .flatten()
            .any(|&(cs, ce)| cs <= off && off < ce)
        {
            continue; // inside an element — handled by its own recursion
        }
        // A comment on the open-bracket line, before the first element, trails the open bracket.
        if line == open_line && first_start.is_none_or(|fs| off < fs) {
            lc.after_open = Some(text);
            continue;
        }
        // The last element whose code ends before this comment, if on the same source line → trail.
        let prev = spans
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, s)| s.filter(|&(_, ce)| ce <= off).map(|(_, ce)| (i, ce)));
        if let Some((i, _)) = prev.filter(|&(_, ce)| line_of(src, ce.saturating_sub(1)) == line) {
            lc.trailing[i] = Some(text);
            continue;
        }
        // A standalone comment after a lambda argument belongs to the lambda *body* (gdformat indents
        // it at the body, not the arg list). Rather than mis-place it, omit it — the comment-multiset
        // net then falls back to the verbatim statement, which has it exactly right.
        if prev.is_some_and(|(i, _)| node_contains_lambda(&elems[i])) {
            continue;
        }
        match spans.iter().position(|s| s.is_some_and(|(cs, _)| cs > off)) {
            Some(nx) => lc.before[nx].push(text),
            None => lc.tail.push(text),
        }
    }
    lc
}

/// The element expression nodes of a bracket list (args / array / dict entries / params).
fn list_elements(list_node: &GdNode) -> Vec<GdNode> {
    list_node
        .children()
        .filter(|c| is_expr_kind(c.kind()) || matches!(c.kind(), S::DictEntry | S::Param))
        .cloned()
        .collect()
}

// ---- dot chains -----------------------------------------------------------------

/// One link of a (collapsed) postfix chain.
enum Seg {
    Field(String), // `.name`
    Call(GdNode),  // `(args)` applied to the current segment
    Index(GdNode), // `[idx]`
}

/// Collapse a left-nested postfix chain (`a.b(x).c[y]`) into a base node plus ordered segments.
fn collapse_chain(node: &GdNode) -> Option<(GdNode, Vec<Seg>)> {
    let mut segs = Vec::new();
    let mut cur = node.clone();
    loop {
        match cur.kind() {
            S::CallExpr => {
                let nodes = child_nodes(&cur);
                let callee = nodes.first()?.clone();
                let args = find_child(&cur, S::ArgList)?;
                segs.push(Seg::Call(args));
                cur = callee;
            }
            S::IndexExpr => {
                let nodes = child_nodes(&cur);
                let obj = nodes.first()?.clone();
                let idx = nodes.get(1)?.clone();
                segs.push(Seg::Index(idx));
                cur = obj;
            }
            S::FieldExpr => {
                let nodes = child_nodes(&cur);
                if nodes.len() < 2 {
                    return None;
                }
                let obj = nodes.first()?.clone();
                let name = node_text(nodes.last()?);
                segs.push(Seg::Field(name));
                cur = obj;
            }
            _ => break,
        }
    }
    segs.reverse();
    Some((cur, segs))
}

/// gdformat's `_format_dot_chain_to_multiple_lines`: try bottom-up (wrap the final call's arguments,
/// keeping the chain prefix flat); if every line fits use it, otherwise explode at each `.`
/// (leading-dot), wrapping each over-long segment's own arguments recursively.
fn format_dot_chain(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    // gdformat's `_format_dot_chain_to_multiple_lines` decision order:
    //   1. a chain *containing a lambda* always wraps bottom-up (a Godot-parser-bug workaround) —
    //      regardless of a magic comma deep inside it;
    //   2. otherwise a chain forced multi-line by a magic comma goes straight to leading-dot;
    //   3. otherwise try bottom-up, and keep it only if every line fits, else leading-dot.
    // Bottom-up needs args to wrap, so it applies only when the chain ends in a call or subscript.
    let endable = matches!(node.kind(), S::CallExpr | S::IndexExpr);
    let bottom_up = if endable && node_contains_lambda(node) {
        format_chain_bottom_up(w, node, indent, prefix, suffix)
    } else if endable && !forcing_multiline(node) {
        format_chain_bottom_up(w, node, indent, prefix, suffix)
            .filter(|b| b.iter().all(|l| width(l) <= w.max))
    } else {
        None
    };
    if let Some(bottom_up) = bottom_up {
        return Some(bottom_up);
    }
    format_chain_leading_dot(w, node, indent, prefix, suffix)
}

/// Wrap only the final segment's arguments; the chain prefix stays on the first line.
fn format_chain_bottom_up(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    match node.kind() {
        S::CallExpr => {
            let nodes = child_nodes(node);
            let callee = nodes.first()?;
            // gdformat's `_format_dot_chain_to_multiple_lines_bottom_up` falls back to leading-dot when
            // the lambda lives in an *earlier* chain segment (the callee) rather than the final call's
            // own arguments — bottom-up keeps the prefix flat, which a lambda there cannot.
            if node_contains_lambda(callee) {
                return None;
            }
            let arglist = find_child(node, S::ArgList)?;
            let callee_str = expr_to_str(w, callee)?;
            let elems = list_elements(&arglist);
            if elems.is_empty() {
                return None; // nothing to wrap
            }
            format_comma_list(
                w,
                &arglist,
                indent,
                &format!("{prefix}{callee_str}("),
                &format!("){suffix}"),
            )
        }
        S::IndexExpr => {
            let nodes = child_nodes(node);
            let obj = nodes.first()?;
            let idx = nodes.get(1)?;
            if node_contains_lambda(obj) {
                return None;
            }
            let obj_str = expr_to_str(w, obj)?;
            format_expression(
                w,
                idx,
                indent,
                &format!("{prefix}{obj_str}["),
                &format!("]{suffix}"),
            )
        }
        _ => None,
    }
}

/// One rendered link of a leading-dot chain explosion: the base or a `.field`, plus any call/index
/// applied to it.
struct Unit {
    lead_dot: bool,
    head: String,              // `name` for a field unit, or base text
    base_node: Option<GdNode>, // for the base unit (may itself fold)
    call: Option<GdNode>,
    index: Option<GdNode>,
}

/// Explode the chain at each `.`, leading-dot style, inside injected parens.
fn format_chain_leading_dot(
    w: &W,
    node: &GdNode,
    indent: usize,
    prefix: &str,
    suffix: &str,
) -> Option<Vec<String>> {
    let (base, segs) = collapse_chain(node)?;
    // Group segments into render-units: the base, then each `.field` (carrying any immediately
    // following call/index).
    let mut units: Vec<Unit> = vec![Unit {
        lead_dot: false,
        head: String::new(),
        base_node: Some(base.clone()),
        call: None,
        index: None,
    }];
    for seg in segs {
        match seg {
            Seg::Field(name) => units.push(Unit {
                lead_dot: true,
                head: name,
                base_node: None,
                call: None,
                index: None,
            }),
            Seg::Call(args) => {
                let u = units.last_mut()?;
                if u.call.is_some() || u.index.is_some() {
                    // chained `(...)(...)` / `[...]( ...)` — too unusual; bail.
                    return None;
                }
                u.call = Some(args);
            }
            Seg::Index(idx) => {
                let u = units.last_mut()?;
                // A `.field(args)[idx]` unit is fine (the index trails the call); only a second index
                // on the same unit (`a[i][j]`) is too unusual for this path.
                if u.index.is_some() {
                    return None;
                }
                u.index = Some(idx);
            }
        }
    }
    let inside_par = (prefix.ends_with('(') && suffix.starts_with(')'))
        || (prefix.ends_with('[') && suffix.starts_with(']'));
    let (lpar, rpar) = if inside_par { ("", "") } else { ("(", ")") };
    let mut out = vec![format!("{}{}{}", w.indent(indent), prefix, lpar)];
    let child = indent + 1;
    // Compact first: the whole chain on one continuation line, if it fits — gdformat explodes at each
    // `.` only when even that overflows.
    if let Some(flat) = expr_to_str(w, node) {
        let compact = format!("{}{}", w.indent(child), flat);
        if !forcing_multiline(node) && width(&compact) <= w.max {
            out.push(compact);
            out.push(format!("{}{}{}", w.indent(indent), rpar, suffix));
            return Some(out);
        }
    }
    for u in &units {
        let lead = if u.lead_dot { ". " } else { "" };
        let head = if let Some(bn) = &u.base_node {
            expr_to_str(w, bn)?
        } else {
            u.head.clone()
        };
        // A trailing `[idx]` on this unit (`.field(args)[idx]`) is appended after the call's `)`.
        let idx_suffix = match &u.index {
            Some(idx) if u.call.is_some() => format!("[{}]", expr_to_str(w, idx)?),
            _ => String::new(),
        };
        // Each segment is itself rendered single-line-if-it-fits, else exploded (gdformat's recursion).
        if let Some(args) = &u.call {
            let flat = format!(
                "{}{}{}({}){idx_suffix}",
                w.indent(child),
                lead,
                head,
                args_to_str(w, args)?
            );
            if !forcing_multiline(args) && width(&flat) <= w.max {
                out.push(flat);
            } else if list_elements(args).is_empty() {
                out.push(format!("{}{}{}(){idx_suffix}", w.indent(child), lead, head));
            } else {
                out.extend(format_comma_list(
                    w,
                    args,
                    child,
                    &format!("{lead}{head}("),
                    &format!("){idx_suffix}"),
                )?);
            }
        } else if let Some(idx) = &u.index {
            out.push(format!(
                "{}{}{}[{}]",
                w.indent(child),
                lead,
                head,
                expr_to_str(w, idx)?
            ));
        } else {
            out.push(format!("{}{}{}", w.indent(child), lead, head));
        }
    }
    out.push(format!("{}{}{}", w.indent(indent), rpar, suffix));
    Some(out)
}

// ---- single-line rendering (gdformat's expression_to_str) -----------------------

/// Render `node` on one line with gdformat's spacing. Returns `None` for anything not modelled.
#[allow(
    clippy::too_many_lines,
    reason = "one dispatch table mirroring gdformat's expression_to_str — splitting it would obscure the 1:1 mapping"
)]
fn expr_to_str(w: &W, node: &GdNode) -> Option<String> {
    match node.kind() {
        S::Literal => {
            let els = sig(node);
            let El::Tok(k, t) = els.first()? else {
                return None;
            };
            if *k == S::String && w.cfg.normalize_strings {
                Some(canonical_string(t))
            } else {
                Some(t.clone())
            }
        }
        S::NameRef | S::GetNodeExpr | S::UniqueNodeExpr => Some(node_text(node)),
        S::FieldExpr => {
            let nodes = child_nodes(node);
            if nodes.len() < 2 {
                return None;
            }
            let obj = expr_to_str(w, nodes.first()?)?;
            // The member name is the node *after* the object (both are `NameRef`s — `a.b`).
            let name = node_text(nodes.last()?);
            Some(format!("{obj}.{name}"))
        }
        S::IndexExpr => {
            let nodes = child_nodes(node);
            Some(format!(
                "{}[{}]",
                expr_to_str(w, nodes.first()?)?,
                expr_to_str(w, nodes.get(1)?)?
            ))
        }
        S::CallExpr => {
            let nodes = child_nodes(node);
            let callee = expr_to_str(w, nodes.first()?)?;
            let arglist = find_child(node, S::ArgList)?;
            Some(format!("{callee}({})", args_to_str(w, &arglist)?))
        }
        S::PreloadExpr => {
            let arglist = find_child(node, S::ArgList)?;
            Some(format!("preload({})", args_to_str(w, &arglist)?))
        }
        S::BinExpr => {
            let nodes = child_nodes(node);
            let op = bin_op(node)?;
            let lhs = expr_to_str(w, nodes.first()?)?;
            let rhs = expr_to_str(w, nodes.get(1)?)?;
            let op_txt = if is_assign_op(op) {
                op_str(op)
            } else {
                op_str_bin(op)
            };
            Some(format!("{lhs} {op_txt} {rhs}"))
        }
        S::UnaryExpr => {
            let op = sig(node).into_iter().find_map(|el| match el {
                El::Tok(k @ (S::NotKw | S::Bang | S::Minus | S::Plus | S::Tilde), _) => Some(k),
                _ => None,
            })?;
            let operand = expr_to_str(w, &first_expr_child(node)?)?;
            let (op_txt, sp) = match op {
                S::NotKw => ("not", " "),
                S::Bang => ("!", ""),
                S::Minus => ("-", ""),
                S::Plus => ("+", ""),
                S::Tilde => ("~", ""),
                _ => return None,
            };
            Some(format!("{op_txt}{sp}{operand}"))
        }
        S::AwaitExpr => Some(format!(
            "await {}",
            expr_to_str(w, &first_expr_child(node)?)?
        )),
        S::TernaryExpr => {
            let nodes = child_nodes(node);
            Some(format!(
                "{} if {} else {}",
                expr_to_str(w, nodes.first()?)?,
                expr_to_str(w, nodes.get(1)?)?,
                expr_to_str(w, nodes.get(2)?)?
            ))
        }
        S::IsExpr => {
            let a = expr_to_str(w, &first_expr_child(node)?)?;
            let ty = node
                .children()
                .find(|c| matches!(c.kind(), S::TypeRef | S::TypedArray | S::TypedDict))?;
            let not = sig(node).iter().any(|e| matches!(e, El::Tok(S::NotKw, _)));
            Some(format!(
                "{a} {} {}",
                if not { "is not" } else { "is" },
                node_text(ty)
            ))
        }
        S::CastExpr => {
            let a = expr_to_str(w, &first_expr_child(node)?)?;
            let ty = node
                .children()
                .find(|c| matches!(c.kind(), S::TypeRef | S::TypedArray | S::TypedDict))?;
            Some(format!("{a} as {}", node_text(ty)))
        }
        S::InExpr => {
            let nodes = child_nodes(node);
            let a = expr_to_str(w, nodes.first()?)?;
            let b = expr_to_str(w, nodes.get(1)?)?;
            let not = sig(node).iter().any(|e| matches!(e, El::Tok(S::NotKw, _)));
            Some(format!("{a} {} {b}", if not { "not in" } else { "in" }))
        }
        S::ParenExpr => {
            let inner = child_nodes(node)
                .into_iter()
                .find(|c| is_expr_kind(c.kind()))?;
            Some(format!(
                "({})",
                expr_to_str(w, &remove_outer_parens(&inner))?
            ))
        }
        S::ArrayLit => {
            let elems = list_elements(node)
                .iter()
                .map(|e| expr_to_str(w, e))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("[{}]", elems.join(", ")))
        }
        S::DictLit => {
            let elems = list_elements(node)
                .iter()
                .map(|e| expr_to_str(w, e))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("{{{}}}", elems.join(", ")))
        }
        S::DictEntry => {
            let (key, val, colon) = dict_entry_kv(node)?;
            let key = expr_to_str(w, &key)?;
            let val = expr_to_str(w, &val)?;
            Some(if colon {
                format!("{key}: {val}")
            } else {
                format!("{key} = {val}")
            })
        }
        S::Param => {
            let name = name_of(node)?;
            let ty = find_child_ty(node).map(|t| node_text(&t));
            let default = first_expr_child(node);
            let inferred = sig(node)
                .iter()
                .any(|e| matches!(e, El::Tok(S::ColonEq, _)));
            let mut s = name;
            if let Some(ty) = ty {
                s.push_str(": ");
                s.push_str(&ty);
            }
            if let Some(def) = default {
                s.push_str(if inferred { " := " } else { " = " });
                s.push_str(&expr_to_str(w, &def)?);
            }
            Some(s)
        }
        S::LambdaExpr => {
            // Inline form (`func(params) [-> Type]: body`); only valid when the body is a single
            // simple statement — otherwise `None` defers to the multi-line `explode` path.
            let header = lambda_header_str(w, node)?;
            let block = find_child(node, S::Block)?;
            let stmts: Vec<GdNode> = block.children().cloned().collect();
            let [stmt] = stmts.as_slice() else {
                return None;
            };
            Some(format!("{header} {}", stmt_to_str(w, stmt)?))
        }
        _ => None,
    }
}

/// The single-line lambda header `func{ name}(params)[ -> Type]:` (gdformat's `_lambda_header_to_str`).
fn lambda_header_str(w: &W, node: &GdNode) -> Option<String> {
    let name = name_of(node).map(|n| format!(" {n}")).unwrap_or_default();
    let params = find_child(node, S::ParamList)?;
    let ty = find_child_ty(node)
        .map(|t| format!(" -> {}", node_text(&t)))
        .unwrap_or_default();
    Some(format!("func{name}({}){ty}:", args_to_str(w, &params)?))
}

/// Render a *simple* (non-compound, single-line) statement inline — used for an inline lambda body.
/// Returns `None` for a compound or unmodelled statement so the caller falls back to multi-line.
fn stmt_to_str(w: &W, stmt: &GdNode) -> Option<String> {
    match stmt.kind() {
        S::ExprStmt => expr_to_str(w, &first_expr_child(stmt)?),
        S::ReturnStmt => match first_expr_child(stmt) {
            Some(e) => Some(format!("return {}", expr_to_str(w, &e)?)),
            None => Some("return".to_owned()),
        },
        S::PassStmt => Some("pass".to_owned()),
        S::BreakStmt => Some("break".to_owned()),
        S::ContinueStmt => Some("continue".to_owned()),
        _ => None,
    }
}

fn args_to_str(w: &W, arglist: &GdNode) -> Option<String> {
    Some(
        list_elements(arglist)
            .iter()
            .map(|e| expr_to_str(w, e))
            .collect::<Option<Vec<_>>>()?
            .join(", "),
    )
}
