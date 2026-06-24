//! `gdscript-base` — foundational POD types shared across the gdscript-analyzer.
//!
//! The lowest layer of the crate stack (`plans/01-ARCHITECTURE.md` §1). It holds the
//! engine-/protocol-neutral, `serde`-serializable result structs every client maps to
//! its own protocol, plus byte-offset position types and a [`LineIndex`] for the
//! byte↔(line, column) and byte↔UTF-16 conversions LSP clients need.
//!
//! All offsets are **byte** offsets into a file's UTF-8 source. No logic beyond the
//! conversions lives here. The crate is `wasm32`-safe (no `std::fs`, clocks, threads).
#![cfg_attr(docsrs, feature(doc_cfg))]

use serde::{Deserialize, Serialize};

/// An opaque file handle. The host owns the `FileId` → text mapping; the library never
/// reads paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

/// A half-open byte range `[start, end)` into a file's UTF-8 source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

impl TextRange {
    /// A new range from `start` to `end` (bytes).
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

/// A `(file, byte offset)` cursor position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePosition {
    /// The file.
    pub file: FileId,
    /// The byte offset within the file.
    pub offset: u32,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A hard error.
    Error,
    /// A warning.
    Warning,
    /// Informational.
    Info,
    /// A hint.
    Hint,
}

/// What analysis layer produced a diagnostic — lets clients group/filter parse vs. type
/// diagnostics without parsing the `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSource {
    /// A lexer / parser / indentation diagnostic (Phase 1).
    #[default]
    Syntax,
    /// A type / semantic diagnostic from inference (Phase 2).
    Type,
}

/// A diagnostic with a byte range, a stable machine code, a severity, and a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The byte range the diagnostic applies to.
    pub range: TextRange,
    /// Severity.
    pub severity: Severity,
    /// A stable code, e.g. `GDSCRIPT_SYNTAX` or `INTEGER_DIVISION`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Which analysis layer produced it. Defaults to [`DiagnosticSource::Syntax`] so older
    /// serialized diagnostics and Phase-1 call sites round-trip unchanged.
    #[serde(default)]
    pub source: DiagnosticSource,
    /// Quick-fixes offered for this diagnostic (e.g. "add type annotation"). Empty when none.
    #[serde(default)]
    pub fixes: Vec<CodeAction>,
}

/// The kind of a document symbol (a subset of LSP `SymbolKind`, named for GDScript).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// A `class_name` / inner `class`.
    Class,
    /// A `func`.
    Function,
    /// A `func` that is a class member (currently same as `Function`).
    Method,
    /// A `var`.
    Variable,
    /// A `const`.
    Constant,
    /// An `enum`.
    Enum,
    /// An enum variant.
    EnumMember,
    /// A `signal`.
    Signal,
}

/// A (possibly nested) symbol in a document's outline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSymbol {
    /// The symbol name.
    pub name: String,
    /// Optional detail (e.g. a signature).
    pub detail: Option<String>,
    /// The symbol kind.
    pub kind: SymbolKind,
    /// The full range of the symbol (its whole declaration).
    pub range: TextRange,
    /// The range of the name/selection within `range`.
    pub selection_range: TextRange,
    /// Nested symbols (members of a class, variants of an enum).
    pub children: Vec<DocumentSymbol>,
}

/// What a fold range corresponds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FoldKind {
    /// An indented block body.
    Block,
    /// A `#region`…`#endregion` pair.
    Region,
    /// A multi-line bracketed span.
    Brackets,
}

/// A foldable range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldRange {
    /// The foldable byte range.
    pub range: TextRange,
    /// What kind of fold it is.
    pub kind: FoldKind,
}

/// The kind of a completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// A language keyword.
    Keyword,
    /// An annotation (`@export`, …).
    Annotation,
    /// A function/method name.
    Function,
    /// A variable / parameter / local.
    Variable,
    /// A constant.
    Constant,
    /// A class / type name.
    Class,
    /// An enum.
    Enum,
    /// A signal.
    Signal,
}

/// A by-name completion suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// The label shown / inserted.
    pub label: String,
    /// The kind of suggestion.
    pub kind: CompletionKind,
    /// Optional text to insert (defaults to `label`).
    pub insert_text: Option<String>,
    /// Optional secondary text shown after the label — a type or signature, e.g. `: int`
    /// or `(node: Node) -> void`. Phase 2 fills this for typed members; `None` keeps the
    /// Phase-1 by-name items unchanged.
    #[serde(default)]
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Phase 2 PODs — hover, signature help, inlay hints, code actions, navigation.
// Each is an engine-/protocol-neutral result struct (byte offsets, serde). A feature
// returning one of these maps it to its own protocol at the client edge. See
// `plans/PHASE-2-IMPLEMENTATION-PLAYBOOK.md` §1.1.
// ---------------------------------------------------------------------------

/// Documentation rendered as Markdown (engine `BBCode` already converted at codegen time).
pub type Markdown = String;

/// The result of a hover query: an inferred type / signature label plus engine docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverResult {
    /// The inferred type / signature rendered for display, e.g. `Node` or
    /// `add_child(node: Node) -> void`. `None` when the type is `Unknown` (elided — the
    /// Phase-3 cross-file seam) so we never show a placeholder type.
    pub ty_label: Option<String>,
    /// Engine documentation as Markdown. Empty when no doc XML is available.
    pub doc: Markdown,
    /// The source range the hover applies to (the hovered token / expression).
    pub range: TextRange,
}

/// One parameter within a [`SignatureInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamInfo {
    /// The parameter label, e.g. `node: Node` or `force_readable_name: bool = false`.
    pub label: String,
    /// Optional documentation (Markdown).
    pub doc: Markdown,
}

/// One signature shown in signature help (GDScript has no overloads, so usually one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureInfo {
    /// The full signature label, e.g.
    /// `add_child(node: Node, force_readable_name: bool = false) -> void`.
    pub label: String,
    /// Optional documentation (Markdown).
    pub doc: Markdown,
    /// The parameters, in order.
    pub params: Vec<ParamInfo>,
}

/// The result of a signature-help query at a call site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureHelp {
    /// The candidate signatures.
    pub signatures: Vec<SignatureInfo>,
    /// Index into `signatures` of the active one.
    pub active_signature: u32,
    /// Index of the active parameter within the active signature. A vararg call keeps the
    /// last parameter active once the fixed parameters are exhausted.
    pub active_parameter: u32,
}

/// What an [`InlayHint`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InlayHintKind {
    /// An inferred type, e.g. `: int` after a `:=` declaration or an unannotated parameter.
    Type,
    /// An inferred parameter name shown at a call site.
    Parameter,
}

/// An inline hint rendered at a byte offset (e.g. the `: int` the engine LSP omits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlayHint {
    /// The byte offset at which to render the hint.
    pub offset: u32,
    /// The hint text, e.g. `: int`.
    pub label: String,
    /// What kind of hint it is.
    pub kind: InlayHintKind,
}

/// A single text edit: replace `range` with `new_text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// The byte range to replace.
    pub range: TextRange,
    /// The replacement text.
    pub new_text: String,
}

/// A set of edits to apply. Phase 2 is single-file, so every edit targets `file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChange {
    /// The file the edits apply to.
    pub file: FileId,
    /// The edits (non-overlapping; the client sorts and applies them).
    pub edits: Vec<TextEdit>,
}

/// A code action / quick-fix: a titled, optionally-kinded [`SourceChange`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAction {
    /// Human-readable title, e.g. `Add type annotation`.
    pub title: String,
    /// An LSP-style kind such as `quickfix` or `refactor.rewrite`; `None` if unspecified.
    pub kind: Option<String>,
    /// The edit this action performs.
    pub edit: SourceChange,
}

/// A navigation target (goto-definition / -declaration). Phase 2 only ever points within
/// the same file; cross-file targets arrive in Phase 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavTarget {
    /// The file the target lives in.
    pub file: FileId,
    /// The full range of the target's declaration.
    pub full_range: TextRange,
    /// The name / selection range to focus within `full_range`.
    pub focus_range: TextRange,
    /// The target symbol's name.
    pub name: String,
    /// The target symbol's kind.
    pub kind: SymbolKind,
}

/// A read query was cancelled by a concurrent change. (Phase 1 never actually cancels,
/// but the type is on the API surface so the Phase 3 salsa swap is source-compatible.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// The result of a cancellable read query.
pub type Cancellable<T> = Result<T, Cancelled>;

/// Maps byte offsets to/from `(line, column)` and UTF-16 columns.
///
/// Lines and columns are 0-based. The core emits byte offsets; LSP/JS clients convert
/// to UTF-16 via this (the documented position-encoding footgun —
/// `plans/01-ARCHITECTURE.md` §4).
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the start of each line (line 0 starts at 0).
    line_starts: Vec<u32>,
    /// Total source length in bytes.
    len: u32,
}

/// A 0-based `(line, column)` position. `col` is a byte offset within the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 0-based line.
    pub line: u32,
    /// 0-based byte column within the line.
    pub col: u32,
}

impl LineIndex {
    /// Build a line index for `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                // `i` fits in u32 for any file we accept (< 4 GiB).
                #[allow(clippy::cast_possible_truncation)]
                line_starts.push(i as u32 + 1);
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let len = text.len() as u32;
        Self { line_starts, len }
    }

    /// The `(line, byte-column)` of a byte offset (clamped to the end of input).
    #[must_use]
    pub fn line_col(&self, offset: u32) -> LineCol {
        let offset = offset.min(self.len);
        // The line is the last line-start <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        };
        #[allow(clippy::cast_possible_truncation)]
        let line = line as u32;
        LineCol {
            line,
            col: offset - self.line_starts[line as usize],
        }
    }

    /// The UTF-16 column of a byte offset on its line (LSP's default encoding).
    #[must_use]
    pub fn utf16_col(&self, text: &str, offset: u32) -> u32 {
        let lc = self.line_col(offset);
        let line_start = self.line_starts[lc.line as usize] as usize;
        let col_end = (line_start + lc.col as usize).min(text.len());
        let units: usize = text[line_start..col_end].chars().map(char::len_utf16).sum();
        u32::try_from(units).unwrap_or(u32::MAX)
    }

    /// The number of lines.
    #[must_use]
    pub fn line_count(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        {
            self.line_starts.len() as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_basics() {
        let src = "ab\ncde\n\nx";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_count(), 4);
        assert_eq!(idx.line_col(0), LineCol { line: 0, col: 0 });
        assert_eq!(idx.line_col(1), LineCol { line: 0, col: 1 });
        assert_eq!(idx.line_col(3), LineCol { line: 1, col: 0 }); // 'c'
        assert_eq!(idx.line_col(7), LineCol { line: 2, col: 0 }); // blank line
        assert_eq!(idx.line_col(8), LineCol { line: 3, col: 0 }); // 'x'
    }

    #[test]
    fn utf16_columns_account_for_astral_chars() {
        // "a😀b": 'a' is 1 UTF-8 byte / 1 UTF-16 unit; '😀' is 4 bytes / 2 units.
        let src = "a😀b";
        let idx = LineIndex::new(src);
        assert_eq!(idx.utf16_col(src, 0), 0); // before 'a'
        assert_eq!(idx.utf16_col(src, 1), 1); // before '😀'
        assert_eq!(idx.utf16_col(src, 5), 3); // before 'b' (1 + 2 units)
    }
}
