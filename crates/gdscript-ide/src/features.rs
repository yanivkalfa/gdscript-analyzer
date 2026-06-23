//! The Tier-0 IDE features: parse diagnostics, document symbols, folding ranges, and
//! by-name completion. Each is a pure `(text) -> value` function over a fresh parse,
//! so the Phase-3 salsa swap is a localized change.

use gdscript_base::{
    CompletionItem, CompletionKind, Diagnostic, DiagnosticSource, DocumentSymbol, FoldKind,
    FoldRange, LineIndex, Severity, SymbolKind, TextRange,
};
use gdscript_syntax::ast::{self, AstNode, Decl};
use gdscript_syntax::{GdNode, SyntaxKind, parse, tokenize};

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
pub fn diagnostics(text: &str) -> Vec<Diagnostic> {
    parse(text)
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
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let parsed = parse(text);
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
pub fn folding_ranges(text: &str) -> Vec<FoldRange> {
    let parsed = parse(text);
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
pub fn completions(text: &str, offset: u32) -> Vec<CompletionItem> {
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
    items.extend(local_symbols(text));
    items
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Collect document-local symbol names (declarations, params, enum variants), deduped.
/// Not scope-aware in Tier 0 — every name in the file is offered.
fn local_symbols(text: &str) -> Vec<CompletionItem> {
    let parsed = parse(text);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for node in ast::descendants(&parsed.syntax_node()) {
        let (name, kind) = if let Some(decl) = Decl::cast(node.clone()) {
            let Some(name) = decl.name() else { continue };
            (name, decl_completion_kind(&decl))
        } else if node.kind() == SyntaxKind::Param {
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

    #[test]
    fn diagnostics_report_parse_errors() {
        let diags = diagnostics("func f(:\n\tpass\n");
        assert!(!diags.is_empty());
        assert_eq!(diags[0].code, "GDSCRIPT_SYNTAX");
        assert_eq!(diags[0].severity, Severity::Error);
        // Valid code → no diagnostics.
        assert!(diagnostics("func f():\n\tpass\n").is_empty());
    }

    #[test]
    fn document_symbols_nest_class_and_enum() {
        let src =
            "class_name Foo\nenum E { A, B }\nclass Inner:\n\tvar y = 1\n\tfunc m():\n\t\tpass\n";
        let syms = document_symbols(src);
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
        let folds = folding_ranges(src);
        assert!(folds.iter().any(|f| f.kind == FoldKind::Region));
        assert!(folds.iter().any(|f| f.kind == FoldKind::Block));
    }

    #[test]
    fn completion_after_at_offers_annotations() {
        let src = "@e\n";
        let items = completions(src, 2); // cursor right after "@e"
        assert!(items.iter().all(|i| i.kind == CompletionKind::Annotation));
        assert!(items.iter().any(|i| i.label == "export"));
    }

    #[test]
    fn completion_offers_keywords_and_locals() {
        let src = "var health = 100\nfunc take_damage(amount):\n\tpass\n";
        let items = completions(src, u32::try_from(src.len()).unwrap());
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
        assert!(items.iter().any(|i| i.label == "amount"));
    }
}
