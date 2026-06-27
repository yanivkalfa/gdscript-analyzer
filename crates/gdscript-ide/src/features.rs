//! The Tier-0 IDE features: parse diagnostics, document symbols, folding ranges, and
//! by-name completion. Each is a `(db, file) -> value` function over the **memoized**
//! [`parse`](gdscript_db::parse) query, so repeated queries in a revision share one parse and a
//! body edit reuses the cached tree.

use gdscript_base::{
    CompletionItem, CompletionKind, Diagnostic, DiagnosticSource, DocumentSymbol, FoldKind,
    FoldRange, LineIndex, Severity, SymbolKind, TextRange,
};
use gdscript_db::{Db, FileText, parse};
use gdscript_syntax::ast::{self, AstNode, Decl};
use gdscript_syntax::{GdNode, SyntaxKind, tokenize};

/// Convert a `text-size` byte range to the serde POD `TextRange`.
fn to_base_range(r: text_size::TextRange) -> TextRange {
    TextRange::new(u32::from(r.start()), u32::from(r.end()))
}

/// The byte range of a declaration's `Name` child, for the selection range.
fn name_range(node: &GdNode) -> Option<text_size::TextRange> {
    node.children()
        .find(|c| c.kind() == SyntaxKind::Name)
        .map(|n| n.text_range())
}

// ---- diagnostics ----------------------------------------------------------------

/// Parse-error diagnostics (lexer + parser recovery + indentation issues).
#[must_use]
pub fn diagnostics(db: &dyn Db, file: FileText) -> Vec<Diagnostic> {
    parse(db, file)
        .errors()
        .iter()
        .map(|e| Diagnostic {
            range: to_base_range(e.range),
            severity: Severity::Error,
            code: "GDSCRIPT_SYNTAX".to_owned(),
            message: e.message.clone(),
            source: DiagnosticSource::Syntax,
            fixes: Vec::new(),
        })
        .collect()
}

// ---- document symbols -----------------------------------------------------------

/// The document outline.
#[must_use]
pub fn document_symbols(db: &dyn Db, file: FileText) -> Vec<DocumentSymbol> {
    let parsed = parse(db, file);
    match ast::SourceFile::cast(parsed.syntax_node()) {
        Some(file) => decls_to_symbols(file.decls()),
        None => Vec::new(),
    }
}

fn decls_to_symbols(decls: impl Iterator<Item = Decl>) -> Vec<DocumentSymbol> {
    decls.filter_map(|d| decl_to_symbol(&d)).collect()
}

fn decl_to_symbol(decl: &Decl) -> Option<DocumentSymbol> {
    let name = decl.name()?;
    if name.is_empty() {
        return None;
    }
    let range = to_base_range(decl.syntax().text_range());
    let selection_range = name_range(decl.syntax()).map_or(range, to_base_range);
    let children = match decl {
        Decl::Class(c) => c
            .body()
            .map(|b| decls_to_symbols(b.decls()))
            .unwrap_or_default(),
        Decl::Enum(e) => e
            .variants()
            .filter_map(|v| {
                let vname = v.text()?;
                if vname.is_empty() {
                    return None;
                }
                let vrange = to_base_range(v.syntax().text_range());
                Some(DocumentSymbol {
                    name: vname,
                    detail: None,
                    kind: SymbolKind::EnumMember,
                    range: vrange,
                    selection_range: vrange,
                    children: Vec::new(),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(DocumentSymbol {
        name,
        detail: None,
        kind: symbol_kind(decl),
        range,
        selection_range,
        children,
    })
}

fn symbol_kind(decl: &Decl) -> SymbolKind {
    match decl {
        Decl::ClassName(_) | Decl::Class(_) => SymbolKind::Class,
        Decl::Func(_) => SymbolKind::Function,
        Decl::Var(_) => SymbolKind::Variable,
        Decl::Const(_) => SymbolKind::Constant,
        Decl::Enum(_) => SymbolKind::Enum,
        Decl::Signal(_) => SymbolKind::Signal,
    }
}

// ---- folding ranges -------------------------------------------------------------

/// Foldable ranges: indented blocks, multi-line brackets, and `#region` pairs.
#[must_use]
pub fn folding_ranges(db: &dyn Db, file: FileText) -> Vec<FoldRange> {
    let parsed = parse(db, file);
    let text = file.text(db);
    let index = LineIndex::new(text);
    let mut out = Vec::new();

    for node in ast::descendants(&parsed.syntax_node()) {
        let kind = match node.kind() {
            SyntaxKind::Block | SyntaxKind::ClassBody | SyntaxKind::MatchStmt => FoldKind::Block,
            SyntaxKind::ArrayLit
            | SyntaxKind::DictLit
            | SyntaxKind::ArgList
            | SyntaxKind::ParamList
            | SyntaxKind::EnumDecl => FoldKind::Brackets,
            _ => continue,
        };
        let range = node.text_range();
        if spans_multiple_lines(&index, range) {
            out.push(FoldRange {
                range: to_base_range(range),
                kind,
            });
        }
    }

    out.extend(region_folds(text));
    out
}

fn spans_multiple_lines(index: &LineIndex, range: text_size::TextRange) -> bool {
    index.line_col(u32::from(range.start())).line != index.line_col(u32::from(range.end())).line
}

/// Pair `#region` … `#endregion` comment tokens (nesting-aware).
fn region_folds(text: &str) -> Vec<FoldRange> {
    let mut stack = Vec::new();
    let mut out = Vec::new();
    for tok in tokenize(text) {
        match tok.kind {
            SyntaxKind::RegionComment => stack.push(u32::from(tok.range.start())),
            SyntaxKind::EndRegionComment => {
                if let Some(start) = stack.pop() {
                    out.push(FoldRange {
                        range: TextRange::new(start, u32::from(tok.range.end())),
                        kind: FoldKind::Region,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

// ---- completions ----------------------------------------------------------------

/// By-name completions. Detects an `@` prefix (annotations) vs. ordinary context
/// (keywords + document-local symbol names). No member completion in Tier 0.
#[must_use]
pub fn completions(db: &dyn Db, file: FileText, offset: u32) -> Vec<CompletionItem> {
    let text = file.text(db);
    let bytes = text.as_bytes();
    let cursor = (offset as usize).min(bytes.len());
    // Walk back over the identifier being typed.
    let mut word_start = cursor;
    while word_start > 0 && is_ident_byte(bytes[word_start - 1]) {
        word_start -= 1;
    }
    // Annotation context: the identifier is immediately preceded by `@`.
    if word_start > 0 && bytes[word_start - 1] == b'@' {
        return ANNOTATIONS
            .iter()
            .map(|a| CompletionItem {
                label: (*a).to_owned(),
                kind: CompletionKind::Annotation,
                insert_text: None,
                detail: None,
            })
            .collect();
    }

    let mut items: Vec<CompletionItem> = KEYWORDS
        .iter()
        .map(|k| CompletionItem {
            label: (*k).to_owned(),
            kind: CompletionKind::Keyword,
            insert_text: None,
            detail: None,
        })
        .collect();
    items.extend(visible_symbols(db, file, offset));
    items
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Collect the symbol names **visible at `offset`**, deduped. Class-level members (funcs, vars,
/// consts, signals, enums, the `class_name`, inner classes) are always visible; a parameter or a
/// local `var`/`const` is offered ONLY when the cursor sits inside its owning callable — a `func`, a
/// lambda, or a `get`/`set` accessor. Enum variants stay class-level (an anonymous-enum variant is a
/// class-level `int` constant).
///
/// Scope is decided per-declaration by [`local_in_scope`]: the node's owning callable is the CST
/// ancestor (so lambdas / accessors / nested closures all work), and "cursor is inside that body" is
/// CST range containment OR an indentation body-extent check — the latter because the CST range
/// stops at the last body token, so a cursor typed on a fresh end-of-body line is *past* it and a
/// range test alone would wrongly hide the body's own params/locals. See `TECH_DEBT.md`.
fn visible_symbols(db: &dyn Db, file: FileText, offset: u32) -> Vec<CompletionItem> {
    let text = file.text(db);
    let parsed = parse(db, file);
    let root = parsed.syntax_node();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for node in ast::descendants(&root) {
        let (name, kind) = if let Some(decl) = Decl::cast(node.clone()) {
            let Some(name) = decl.name() else { continue };
            // A `var`/`const` in a callable body is a LOCAL — scope it to that body; otherwise it is
            // a class member, always visible.
            if owning_callable(&node).is_some() && !local_in_scope(text, &node, offset) {
                continue;
            }
            (name, decl_completion_kind(&decl))
        } else if node.kind() == SyntaxKind::Param {
            // A parameter is visible only inside its own callable's body.
            if !local_in_scope(text, &node, offset) {
                continue;
            }
            match ast::Param::cast(node.clone())
                .and_then(|p| p.name())
                .and_then(|n| n.text())
            {
                Some(name) => (name, CompletionKind::Variable),
                None => continue,
            }
        } else if node.kind() == SyntaxKind::EnumVariant {
            match ast::EnumVariant::cast(node.clone()).and_then(|v| v.text()) {
                Some(name) => (name, CompletionKind::Constant),
                None => continue,
            }
        } else {
            continue;
        };
        if !name.is_empty() && seen.insert(name.clone()) {
            out.push(CompletionItem {
                label: name,
                kind,
                insert_text: None,
                detail: None,
            });
        }
    }
    out
}

/// The nearest enclosing callable (`func`, lambda, or `get`/`set` accessor) of `node`, or `None`
/// if `node` is at class level. A declaration inside one is a LOCAL, scoped to that callable's body.
fn owning_callable(node: &GdNode) -> Option<GdNode> {
    let mut cur = node.parent().cloned();
    while let Some(n) = cur {
        if matches!(
            n.kind(),
            SyntaxKind::FuncDecl | SyntaxKind::Getter | SyntaxKind::Setter | SyntaxKind::LambdaExpr
        ) {
            return Some(n);
        }
        cur = n.parent().cloned();
    }
    None
}

/// Whether the param/local `node` is in scope at `offset`: the cursor must sit inside the body of
/// the node's owning callable. If the cursor is on the callable's own header line (at/after where it
/// starts), it is the inline-body case (`var f := func(x): |`, `set(v): |`); otherwise the cursor's
/// line must be within the header's indented body (the multi-line case, robust to the fresh
/// end-of-body line the CST range omits). Nesting works for free — a closure's body lines are
/// indented under the enclosing func's header, so the func's locals stay visible there too. The CST
/// `FuncDecl` range is deliberately NOT used for containment: it absorbs trailing newlines to EOF,
/// which would wrongly scope a class-level cursor at end of file.
fn local_in_scope(text: &str, node: &GdNode, offset: u32) -> bool {
    let Some(callable) = owning_callable(node) else {
        return false;
    };
    let bytes = text.as_bytes();
    // The `func`/`get`/`set` keyword offset — NOT the node's `text_range().start()`, which absorbs
    // leading trivia (the preceding newline) and would put the header on the previous line.
    let start = callable_keyword_offset(&callable);
    let header_ls = line_start(bytes, start as usize);
    let cursor_ls = line_start(bytes, (offset as usize).min(bytes.len()));
    if cursor_ls == header_ls {
        // Inline body: the cursor is on the callable's own header line, at or after where the
        // callable begins (so `var f := func(x): |` is in the lambda, but a cursor on `|var f` isn't).
        return offset >= start;
    }
    cursor_in_indented_body(text, start, offset)
}

/// The byte offset of a callable's leading keyword (`func`/`static`/`get`/`set`) — the first
/// non-trivia token of the node. Used instead of `text_range().start()`, which includes the leading
/// trivia (e.g. the preceding newline) attached to the node by the tree sink.
fn callable_keyword_offset(node: &GdNode) -> u32 {
    node.descendants_with_tokens()
        .filter_map(cstree::util::NodeOrToken::into_token)
        .find(|t| !t.kind().is_trivia())
        .map_or_else(
            || u32::from(node.text_range().start()),
            |t| u32::from(t.text_range().start()),
        )
}

/// Whether `cursor` sits on a line below the callable header at `header_off` that is still part of
/// the header's indented body — i.e. the cursor's line, and every non-blank line between, is
/// indented deeper than the header. Robust to the fresh end-of-body line the CST range omits.
fn cursor_in_indented_body(text: &str, header_off: u32, cursor: u32) -> bool {
    let bytes = text.as_bytes();
    let header_ls = line_start(bytes, header_off as usize);
    let header_indent = leading_ws(bytes, header_ls);
    let cursor = (cursor as usize).min(bytes.len());
    let cursor_ls = line_start(bytes, cursor);
    if cursor_ls <= header_ls {
        return false; // the cursor is on the header line or above (handled by range containment)
    }
    // The cursor's own line must be indented deeper than the header (its leading whitespace — what
    // the user has typed so far on a fresh line).
    if leading_ws(bytes, cursor_ls) <= header_indent {
        return false;
    }
    // No non-blank line strictly between the header and the cursor may dedent back to the header.
    let mut pos = line_end(bytes, header_ls);
    while pos < cursor_ls {
        pos += 1; // step past the '\n' onto the next line's start
        if pos >= cursor_ls {
            break;
        }
        let eol = line_end(bytes, pos);
        if !text[pos..eol].trim().is_empty() && leading_ws(bytes, pos) <= header_indent {
            return false;
        }
        pos = eol;
    }
    true
}

/// The start offset of the line containing `off` (just after the preceding `\n`, or 0).
fn line_start(bytes: &[u8], off: usize) -> usize {
    let mut s = off.min(bytes.len());
    while s > 0 && bytes[s - 1] != b'\n' {
        s -= 1;
    }
    s
}

/// The end offset of the line starting at `ls` (the next `\n`, or end of text).
fn line_end(bytes: &[u8], ls: usize) -> usize {
    let mut e = ls;
    while e < bytes.len() && bytes[e] != b'\n' {
        e += 1;
    }
    e
}

/// Leading-whitespace (space/tab) width of the line starting at `line_start`.
fn leading_ws(bytes: &[u8], line_start: usize) -> usize {
    let mut i = line_start;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i - line_start
}

fn decl_completion_kind(decl: &Decl) -> CompletionKind {
    match decl {
        Decl::ClassName(_) | Decl::Class(_) => CompletionKind::Class,
        Decl::Func(_) => CompletionKind::Function,
        Decl::Var(_) => CompletionKind::Variable,
        Decl::Const(_) => CompletionKind::Constant,
        Decl::Enum(_) => CompletionKind::Enum,
        Decl::Signal(_) => CompletionKind::Signal,
    }
}

/// The GDScript keyword + literal-keyword table (by-name completion).
const KEYWORDS: &[&str] = &[
    "if",
    "elif",
    "else",
    "for",
    "while",
    "match",
    "when",
    "break",
    "continue",
    "pass",
    "return",
    "var",
    "const",
    "enum",
    "func",
    "static",
    "signal",
    "class",
    "class_name",
    "extends",
    "is",
    "in",
    "as",
    "self",
    "super",
    "void",
    "await",
    "preload",
    "assert",
    "breakpoint",
    "not",
    "and",
    "or",
    "true",
    "false",
    "null",
];

/// The 36 GDScript annotations (names without the leading `@`), Godot 4.5
/// (`plans/research/04-gdscript-semantics-and-features.md` §1.10).
const ANNOTATIONS: &[&str] = &[
    "abstract",
    "export",
    "export_category",
    "export_color_no_alpha",
    "export_custom",
    "export_dir",
    "export_enum",
    "export_exp_easing",
    "export_file",
    "export_file_path",
    "export_flags",
    "export_flags_2d_navigation",
    "export_flags_2d_physics",
    "export_flags_2d_render",
    "export_flags_3d_navigation",
    "export_flags_3d_physics",
    "export_flags_3d_render",
    "export_flags_avoidance",
    "export_global_dir",
    "export_global_file",
    "export_group",
    "export_multiline",
    "export_node_path",
    "export_placeholder",
    "export_range",
    "export_storage",
    "export_subgroup",
    "export_tool_button",
    "icon",
    "onready",
    "rpc",
    "static_unload",
    "tool",
    "warning_ignore",
    "warning_ignore_restore",
    "warning_ignore_start",
];

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_base::FileId;
    use gdscript_db::RootDatabase;
    use salsa::Durability;

    fn db_ft(src: &str) -> (RootDatabase, FileText) {
        let mut db = RootDatabase::default();
        db.set_file_text(FileId(0), src, Durability::LOW);
        let ft = db.file_text(FileId(0)).unwrap();
        (db, ft)
    }

    #[test]
    fn diagnostics_report_parse_errors() {
        let (db, ft) = db_ft("func f(:\n\tpass\n");
        let diags = diagnostics(&db, ft);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].code, "GDSCRIPT_SYNTAX");
        assert_eq!(diags[0].severity, Severity::Error);
        // Valid code → no diagnostics.
        let (db2, ft2) = db_ft("func f():\n\tpass\n");
        assert!(diagnostics(&db2, ft2).is_empty());
    }

    #[test]
    fn document_symbols_nest_class_and_enum() {
        let src =
            "class_name Foo\nenum E { A, B }\nclass Inner:\n\tvar y = 1\n\tfunc m():\n\t\tpass\n";
        let (db, ft) = db_ft(src);
        let syms = document_symbols(&db, ft);
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Foo", "E", "Inner"]);

        let en = syms.iter().find(|s| s.name == "E").unwrap();
        assert_eq!(en.kind, SymbolKind::Enum);
        let variants: Vec<_> = en.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(variants, vec!["A", "B"]);

        let inner = syms.iter().find(|s| s.name == "Inner").unwrap();
        assert_eq!(inner.kind, SymbolKind::Class);
        let members: Vec<_> = inner.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(members, vec!["y", "m"]);
    }

    #[test]
    fn folding_includes_blocks_and_regions() {
        let src = "#region Setup\nfunc f():\n\tvar a = 1\n\tvar b = 2\n#endregion\n";
        let (db, ft) = db_ft(src);
        let folds = folding_ranges(&db, ft);
        assert!(folds.iter().any(|f| f.kind == FoldKind::Region));
        assert!(folds.iter().any(|f| f.kind == FoldKind::Block));
    }

    #[test]
    fn completion_after_at_offers_annotations() {
        let src = "@e\n";
        let (db, ft) = db_ft(src);
        let items = completions(&db, ft, 2); // cursor right after "@e"
        assert!(items.iter().all(|i| i.kind == CompletionKind::Annotation));
        assert!(items.iter().any(|i| i.label == "export"));
    }

    #[test]
    fn completion_offers_keywords_and_locals() {
        let src = "var health = 100\nfunc take_damage(amount):\n\tpass\n";
        let (db, ft) = db_ft(src);
        // Cursor INSIDE take_damage's body (right after the body tab) — its param `amount` is in
        // scope here. Completion is scope-aware, so the param is offered only inside its function.
        let inside =
            u32::try_from("var health = 100\nfunc take_damage(amount):\n\t".len()).unwrap();
        let items = completions(&db, ft, inside);
        assert!(
            items
                .iter()
                .any(|i| i.label == "func" && i.kind == CompletionKind::Keyword)
        );
        assert!(items.iter().any(|i| i.label == "health"));
        assert!(
            items
                .iter()
                .any(|i| i.label == "take_damage" && i.kind == CompletionKind::Function)
        );
        assert!(
            items.iter().any(|i| i.label == "amount"),
            "param visible inside its body"
        );

        // At class level (end of file, indent 0) the param is NOT visible; members still are.
        let at_eof = completions(&db, ft, u32::try_from(src.len()).unwrap());
        assert!(at_eof.iter().any(|i| i.label == "health"));
        assert!(
            !at_eof.iter().any(|i| i.label == "amount"),
            "a function's param must not leak to class level",
        );
    }
}
