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
/// local `var`/`const` is offered ONLY when the cursor sits inside its owning function. Enum
/// variants stay class-level (an anonymous-enum variant is a class-level `int` constant).
///
/// The enclosing function is found by an **indentation scan**, not the CST `FuncDecl` range: that
/// range stops at the last body token, so when you type on a fresh (still-empty) line at the end of
/// a body the cursor is *past* it — a range test would then wrongly hide the function's own
/// params/locals (worse than over-offering). See `TECH_DEBT.md`.
fn visible_symbols(db: &dyn Db, file: FileText, offset: u32) -> Vec<CompletionItem> {
    let text = file.text(db);
    let parsed = parse(db, file);
    let root = parsed.syntax_node();
    // The FuncDecl whose body the cursor sits in (the `func`/`static` token's parent), if any.
    let enc_range = enclosing_func_offset(text, offset)
        .and_then(|o| ast::token_at(&root, u32::try_from(o).unwrap_or(0).into()))
        .map(|t| t.parent().clone())
        .filter(|n| n.kind() == SyntaxKind::FuncDecl)
        .map(|n| n.text_range());
    // A local/param is in scope iff it lies within the enclosing function's range.
    let in_scope = |node: &GdNode| enc_range.is_some_and(|r| r.contains_range(node.text_range()));

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for node in ast::descendants(&root) {
        let (name, kind) = if let Some(decl) = Decl::cast(node.clone()) {
            let Some(name) = decl.name() else { continue };
            // A `var`/`const` in a func / accessor / lambda body is a LOCAL — scope it; otherwise
            // it is a class member, always visible.
            if is_in_callable_body(&node) && !in_scope(&node) {
                continue;
            }
            (name, decl_completion_kind(&decl))
        } else if node.kind() == SyntaxKind::Param {
            // A parameter is visible only inside its own function.
            if !in_scope(&node) {
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

/// Whether `node` is nested inside a callable body (a `func`, a `get`/`set` accessor, or a lambda)
/// — i.e. a declaration here is a LOCAL, not a class member.
fn is_in_callable_body(node: &GdNode) -> bool {
    let mut cur = node.parent().cloned();
    while let Some(n) = cur {
        if matches!(
            n.kind(),
            SyntaxKind::FuncDecl | SyntaxKind::Getter | SyntaxKind::Setter | SyntaxKind::LambdaExpr
        ) {
            return true;
        }
        cur = n.parent().cloned();
    }
    false
}

/// The byte offset of the `func`/`static` keyword of the function whose body contains `offset`,
/// found by an indentation scan. Walk up from the cursor's line; the first shallower non-blank line
/// that begins with `func`/`static func` owns the cursor's block — unless a shallower top-level
/// (indent-0) non-`func` line is reached first, meaning the cursor is at class level. `None` then.
fn enclosing_func_offset(text: &str, offset: u32) -> Option<usize> {
    let bytes = text.as_bytes();
    let cursor = (offset as usize).min(bytes.len());
    let mut line_start = cursor;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let cur_indent = leading_ws(bytes, line_start);
    let mut min_indent = cur_indent;
    let mut i = line_start;
    loop {
        if i == 0 {
            return None; // reached the top with no enclosing function
        }
        let prev_nl = i - 1; // the '\n' ending the previous line
        let mut prev_start = prev_nl;
        while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
            prev_start -= 1;
        }
        let line = &text[prev_start..prev_nl];
        i = prev_start;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue; // blank line — does not change block nesting
        }
        let li = line.len() - trimmed.len(); // leading-whitespace width
        if li < min_indent {
            if starts_a_func(trimmed) {
                return Some(prev_start + li);
            }
            if li == 0 {
                return None; // a top-level non-`func` header → cursor is at class level
            }
            min_indent = li; // a control-flow header (if/for/while/match/…) — keep ascending
        }
    }
}

/// Leading-whitespace (space/tab) width of the line starting at `line_start`.
fn leading_ws(bytes: &[u8], line_start: usize) -> usize {
    let mut i = line_start;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i - line_start
}

/// Whether a trimmed line begins a function declaration (`func …` or `static func …`).
fn starts_a_func(trimmed: &str) -> bool {
    let is_func_kw = |s: &str| {
        s.strip_prefix("func")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '(']))
    };
    is_func_kw(trimmed)
        || trimmed
            .strip_prefix("static")
            .is_some_and(|rest| rest.starts_with([' ', '\t']) && is_func_kw(rest.trim_start()))
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
        let inside = u32::try_from("var health = 100\nfunc take_damage(amount):\n\t".len()).unwrap();
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
        assert!(items.iter().any(|i| i.label == "amount"), "param visible inside its body");

        // At class level (end of file, indent 0) the param is NOT visible; members still are.
        let at_eof = completions(&db, ft, u32::try_from(src.len()).unwrap());
        assert!(at_eof.iter().any(|i| i.label == "health"));
        assert!(
            !at_eof.iter().any(|i| i.label == "amount"),
            "a function's param must not leak to class level",
        );
    }
}
