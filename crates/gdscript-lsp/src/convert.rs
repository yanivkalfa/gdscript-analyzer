//! POD (byte ranges) → `lsp-types` (encoded positions). Together with [`crate::line_index`] this is
//! the only code that touches the protocol's coordinate system, so every range we emit is encoded
//! consistently with the negotiated [`PositionEncoding`](crate::line_index::PositionEncoding).

use gdscript_base::{Diagnostic, Severity, TextRange};
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
