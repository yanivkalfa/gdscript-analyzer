//! POD (byte ranges) → `lsp-types` (encoded positions). Together with [`crate::line_index`] this is
//! the only code that touches the protocol's coordinate system, so every range we emit is encoded
//! consistently with the negotiated [`PositionEncoding`](crate::line_index::PositionEncoding).

use gdscript_base::{
    CompletionItem, CompletionKind, Diagnostic, DocumentSymbol, FoldKind, FoldRange, HoverResult,
    InlayHint, InlayHintKind, SemanticToken, SemanticTokenType as PodTokenType, Severity,
    SignatureHelp, SymbolKind, TextRange,
};
use lsp_types as lsp;

use crate::line_index::{LineIndex, PositionEncoding};

/// A byte [`TextRange`] → an LSP [`Range`](lsp::Range) in `enc`.
#[must_use]
pub fn range_to_lsp(
    li: &LineIndex,
    text: &str,
    range: TextRange,
    enc: PositionEncoding,
) -> lsp::Range {
    lsp::Range::new(
        li.position(text, range.start, enc),
        li.position(text, range.end, enc),
    )
}

/// Our [`Severity`] → LSP [`DiagnosticSeverity`](lsp::DiagnosticSeverity).
#[must_use]
pub fn severity_to_lsp(severity: Severity) -> lsp::DiagnosticSeverity {
    match severity {
        Severity::Error => lsp::DiagnosticSeverity::ERROR,
        Severity::Warning => lsp::DiagnosticSeverity::WARNING,
        Severity::Info => lsp::DiagnosticSeverity::INFORMATION,
        Severity::Hint => lsp::DiagnosticSeverity::HINT,
    }
}

/// A POD [`Diagnostic`] → an LSP [`Diagnostic`](lsp::Diagnostic). The stable `code` is preserved as a
/// string code; `source` is always `"gdscript"` so clients can group ours.
#[must_use]
pub fn diagnostic_to_lsp(
    li: &LineIndex,
    text: &str,
    diag: &Diagnostic,
    enc: PositionEncoding,
) -> lsp::Diagnostic {
    lsp::Diagnostic {
        range: range_to_lsp(li, text, diag.range, enc),
        severity: Some(severity_to_lsp(diag.severity)),
        code: Some(lsp::NumberOrString::String(diag.code.clone())),
        source: Some("gdscript".to_owned()),
        message: diag.message.clone(),
        ..Default::default()
    }
}

// ---- M1 read-feature converters --------------------------------------------------------------

/// A markdown body combining an optional type label (as a fenced `gdscript` block) and doc text.
fn markup(ty_label: Option<&str>, doc: &str) -> lsp::MarkupContent {
    let mut value = String::new();
    if let Some(ty) = ty_label {
        value.push_str("```gdscript\n");
        value.push_str(ty);
        value.push_str("\n```");
    }
    if !doc.is_empty() {
        if !value.is_empty() {
            value.push_str("\n\n");
        }
        value.push_str(doc);
    }
    lsp::MarkupContent {
        kind: lsp::MarkupKind::Markdown,
        value,
    }
}

/// Doc markdown → optional LSP [`Documentation`](lsp::Documentation) (`None` when empty).
fn doc_markup(doc: &str) -> Option<lsp::Documentation> {
    (!doc.is_empty()).then(|| {
        lsp::Documentation::MarkupContent(lsp::MarkupContent {
            kind: lsp::MarkupKind::Markdown,
            value: doc.to_owned(),
        })
    })
}

/// A [`HoverResult`] → an LSP [`Hover`](lsp::Hover) (type label + doc as markdown, with the source
/// range).
#[must_use]
pub fn hover_to_lsp(
    li: &LineIndex,
    text: &str,
    hover: &HoverResult,
    enc: PositionEncoding,
) -> lsp::Hover {
    lsp::Hover {
        contents: lsp::HoverContents::Markup(markup(hover.ty_label.as_deref(), &hover.doc)),
        range: Some(range_to_lsp(li, text, hover.range, enc)),
    }
}

/// A [`CompletionKind`] → an LSP [`CompletionItemKind`](lsp::CompletionItemKind).
#[must_use]
pub fn completion_kind_to_lsp(kind: CompletionKind) -> lsp::CompletionItemKind {
    use lsp::CompletionItemKind as K;
    match kind {
        CompletionKind::Keyword | CompletionKind::Annotation => K::KEYWORD,
        CompletionKind::Function => K::FUNCTION,
        CompletionKind::Variable => K::VARIABLE,
        CompletionKind::Constant => K::CONSTANT,
        CompletionKind::Class => K::CLASS,
        CompletionKind::Enum => K::ENUM,
        CompletionKind::Signal => K::EVENT,
    }
}

/// A POD [`CompletionItem`] → an LSP [`CompletionItem`](lsp::CompletionItem). (No range mapping —
/// items insert `insert_text`/`label`, not a positioned `textEdit`, in M1.)
#[must_use]
pub fn completion_to_lsp(item: &CompletionItem) -> lsp::CompletionItem {
    lsp::CompletionItem {
        label: item.label.clone(),
        kind: Some(completion_kind_to_lsp(item.kind)),
        detail: item.detail.clone(),
        insert_text: item.insert_text.clone(),
        ..Default::default()
    }
}

/// A [`SignatureHelp`] → an LSP [`SignatureHelp`](lsp::SignatureHelp). Encoding-independent (labels
/// only — no source ranges).
#[must_use]
pub fn signature_help_to_lsp(help: &SignatureHelp) -> lsp::SignatureHelp {
    let signatures = help
        .signatures
        .iter()
        .map(|s| lsp::SignatureInformation {
            label: s.label.clone(),
            documentation: doc_markup(&s.doc),
            parameters: Some(
                s.params
                    .iter()
                    .map(|p| lsp::ParameterInformation {
                        label: lsp::ParameterLabel::Simple(p.label.clone()),
                        documentation: doc_markup(&p.doc),
                    })
                    .collect(),
            ),
            active_parameter: None,
        })
        .collect();
    // Clamp the active indices into range (defensive — a client that doesn't itself clamp would
    // otherwise highlight nothing / panic on an out-of-bounds index).
    let n_sigs = u32::try_from(help.signatures.len()).unwrap_or(u32::MAX);
    let active_signature = help.active_signature.min(n_sigs.saturating_sub(1));
    let n_params = help
        .signatures
        .get(active_signature as usize)
        .map_or(0, |s| u32::try_from(s.params.len()).unwrap_or(u32::MAX));
    let active_parameter = help.active_parameter.min(n_params.saturating_sub(1));
    lsp::SignatureHelp {
        signatures,
        active_signature: Some(active_signature),
        active_parameter: Some(active_parameter),
    }
}

/// A [`SymbolKind`] → an LSP [`SymbolKind`](lsp::SymbolKind).
#[must_use]
pub fn symbol_kind_to_lsp(kind: SymbolKind) -> lsp::SymbolKind {
    use lsp::SymbolKind as K;
    match kind {
        SymbolKind::Class => K::CLASS,
        SymbolKind::Function => K::FUNCTION,
        SymbolKind::Method => K::METHOD,
        SymbolKind::Variable => K::VARIABLE,
        SymbolKind::Constant => K::CONSTANT,
        SymbolKind::Enum => K::ENUM,
        SymbolKind::EnumMember => K::ENUM_MEMBER,
        SymbolKind::Signal => K::EVENT,
    }
}

/// A POD [`DocumentSymbol`] → an LSP [`DocumentSymbol`](lsp::DocumentSymbol), recursively (children).
#[must_use]
pub fn document_symbol_to_lsp(
    li: &LineIndex,
    text: &str,
    sym: &DocumentSymbol,
    enc: PositionEncoding,
) -> lsp::DocumentSymbol {
    #[expect(
        deprecated,
        reason = "the `deprecated` field is required to construct the struct"
    )]
    lsp::DocumentSymbol {
        name: sym.name.clone(),
        detail: sym.detail.clone(),
        kind: symbol_kind_to_lsp(sym.kind),
        tags: None,
        deprecated: None,
        range: range_to_lsp(li, text, sym.range, enc),
        selection_range: range_to_lsp(li, text, sym.selection_range, enc),
        children: Some(
            sym.children
                .iter()
                .map(|c| document_symbol_to_lsp(li, text, c, enc))
                .collect(),
        ),
    }
}

/// A [`FoldRange`] → an LSP [`FoldingRange`](lsp::FoldingRange). `Region` maps to the LSP region
/// kind; block/bracket folds carry no kind (editors fold them generically).
#[must_use]
pub fn folding_range_to_lsp(
    li: &LineIndex,
    text: &str,
    fold: &FoldRange,
    enc: PositionEncoding,
) -> lsp::FoldingRange {
    let start = li.position(text, fold.range.start, enc);
    let end = li.position(text, fold.range.end, enc);
    let kind = match fold.kind {
        FoldKind::Region => Some(lsp::FoldingRangeKind::Region),
        FoldKind::Block | FoldKind::Brackets => None,
    };
    lsp::FoldingRange {
        start_line: start.line,
        start_character: Some(start.character),
        end_line: end.line,
        end_character: Some(end.character),
        kind,
        collapsed_text: None,
    }
}

/// An [`InlayHint`] → an LSP [`InlayHint`](lsp::InlayHint) (rendered at its byte offset → position).
#[must_use]
pub fn inlay_hint_to_lsp(
    li: &LineIndex,
    text: &str,
    hint: &InlayHint,
    enc: PositionEncoding,
) -> lsp::InlayHint {
    lsp::InlayHint {
        position: li.position(text, hint.offset, enc),
        label: lsp::InlayHintLabel::String(hint.label.clone()),
        kind: Some(match hint.kind {
            InlayHintKind::Type => lsp::InlayHintKind::TYPE,
            InlayHintKind::Parameter => lsp::InlayHintKind::PARAMETER,
        }),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    }
}

// ---- semantic tokens (M2) --------------------------------------------------------------------

/// The legend's token-type names, in the index order [`token_type_index`] returns. (Index 13 =
/// `event` for a signal; index 14 = `variable` for a `const`, distinguished by the `readonly`
/// modifier — there is no standard `constant` type.)
const LEGEND_TYPES: [&str; 15] = [
    "function",
    "method",
    "variable",
    "parameter",
    "property",
    "class",
    "enum",
    "enumMember",
    "type",
    "decorator",
    "number",
    "string",
    "comment",
    "event",
    "variable",
];

/// The legend's modifier names, in bit order — matching `gdscript_base::semantic_token_modifier`
/// (bit 0 = declaration, …), so a token's `modifiers` bitset is forwarded verbatim.
const LEGEND_MODIFIERS: [&str; 4] = ["declaration", "readonly", "static", "defaultLibrary"];

/// The legend the client needs to decode our semantic tokens. Must stay in sync with
/// [`token_type_index`] and the modifier bit constants.
#[must_use]
pub fn semantic_tokens_legend() -> lsp::SemanticTokensLegend {
    lsp::SemanticTokensLegend {
        token_types: LEGEND_TYPES
            .iter()
            .map(|s| lsp::SemanticTokenType::new(s))
            .collect(),
        token_modifiers: LEGEND_MODIFIERS
            .iter()
            .map(|s| lsp::SemanticTokenModifier::new(s))
            .collect(),
    }
}

/// A POD token type → its index in [`LEGEND_TYPES`].
fn token_type_index(ty: PodTokenType) -> u32 {
    match ty {
        PodTokenType::Function => 0,
        PodTokenType::Method => 1,
        PodTokenType::Variable => 2,
        PodTokenType::Parameter => 3,
        PodTokenType::Property => 4,
        PodTokenType::Class => 5,
        PodTokenType::Enum => 6,
        PodTokenType::EnumMember => 7,
        PodTokenType::Type => 8,
        PodTokenType::Decorator => 9,
        PodTokenType::Number => 10,
        PodTokenType::String => 11,
        PodTokenType::Comment => 12,
        PodTokenType::Signal => 13,
        PodTokenType::Constant => 14,
    }
}

/// Encode source-ordered POD tokens into the LSP **5-integer relative** form (Δline, Δstart, length,
/// typeIndex, modifierBitset). A token that spans lines is skipped (LSP tokens are single-line —
/// multi-line splitting is a follow-up); zero-length tokens are dropped.
#[must_use]
pub fn encode_semantic_tokens(
    li: &LineIndex,
    text: &str,
    tokens: &[SemanticToken],
    enc: PositionEncoding,
) -> Vec<lsp::SemanticToken> {
    let mut data = Vec::with_capacity(tokens.len());
    let (mut prev_line, mut prev_start) = (0u32, 0u32);
    for tok in tokens {
        let start = li.position(text, tok.range.start, enc);
        let end = li.position(text, tok.range.end, enc);
        if start.line != end.line || end.character <= start.character {
            continue; // multi-line or empty — skip
        }
        let delta_line = start.line - prev_line;
        let delta_start = if delta_line == 0 {
            start.character - prev_start
        } else {
            start.character
        };
        data.push(lsp::SemanticToken {
            delta_line,
            delta_start,
            length: end.character - start.character,
            token_type: token_type_index(tok.token_type),
            token_modifiers_bitset: tok.modifiers,
        });
        prev_line = start.line;
        prev_start = start.character;
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdscript_base::{ParamInfo, SignatureInfo};

    #[test]
    fn signature_help_clamps_out_of_bounds_indices() {
        // The analyzer may report `active_*` past the ends (e.g. a vararg call); we must clamp so a
        // client doesn't index out of bounds.
        let help = SignatureHelp {
            signatures: vec![SignatureInfo {
                label: "f(a)".to_owned(),
                doc: String::new(),
                params: vec![ParamInfo {
                    label: "a".to_owned(),
                    doc: String::new(),
                }],
            }],
            active_signature: 5,
            active_parameter: 9,
        };
        let out = signature_help_to_lsp(&help);
        assert_eq!(
            out.active_signature,
            Some(0),
            "clamped to the only signature"
        );
        assert_eq!(
            out.active_parameter,
            Some(0),
            "clamped to the only parameter"
        );
    }

    fn tok(start: u32, end: u32, ty: PodTokenType, modifiers: u32) -> SemanticToken {
        SemanticToken {
            range: TextRange::new(start, end),
            token_type: ty,
            modifiers,
        }
    }

    #[test]
    fn semantic_token_encoding_is_relative() {
        let text = "ab\ncde\n";
        let li = LineIndex::new(text);
        let toks = [
            tok(0, 2, PodTokenType::Variable, 1), // "ab"  → line 0, col 0, len 2, type 2, mod 1
            tok(3, 6, PodTokenType::Function, 0), // "cde" → line 1, col 0, len 3, type 0, mod 0
        ];
        let data = encode_semantic_tokens(&li, text, &toks, PositionEncoding::Utf16);
        assert_eq!(data.len(), 2);
        let d0 = &data[0];
        assert_eq!((d0.delta_line, d0.delta_start, d0.length), (0, 0, 2));
        assert_eq!((d0.token_type, d0.token_modifiers_bitset), (2, 1));
        let d1 = &data[1];
        // new line → delta_line 1, delta_start resets to the absolute column (0).
        assert_eq!((d1.delta_line, d1.delta_start, d1.length), (1, 0, 3));
        assert_eq!((d1.token_type, d1.token_modifiers_bitset), (0, 0));
    }

    #[test]
    fn multi_line_token_is_skipped() {
        // A token spanning lines can't be expressed (LSP tokens are single-line) — drop it.
        let text = "\"\"\"multi\nline\"\"\"\n";
        let li = LineIndex::new(text);
        let toks = [tok(0, 15, PodTokenType::String, 0)];
        assert!(encode_semantic_tokens(&li, text, &toks, PositionEncoding::Utf16).is_empty());
    }
}
