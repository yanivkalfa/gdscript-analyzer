//! Read-request handlers. Each takes a cloned [`Analysis`] snapshot + a [`DocCtx`] (the target
//! document's text, line index, and encoding) and returns a [`Cancellable`] LSP result. They run on
//! the read thread-pool; a concurrent edit unwinds the salsa query to `Err(Cancelled)`, which the
//! dispatcher maps to LSP `ContentModified` so the client re-requests.

use std::sync::Arc;

use gdscript_base::{Cancellable, FileId, FilePosition};
use gdscript_ide::Analysis;
use lsp_types as lsp;

use crate::convert;
use crate::line_index::{LineIndex, PositionEncoding};

/// The per-document context a read handler needs to convert positions/ranges. Cloned cheaply (the
/// text is an `Arc<str>`) and moved onto the worker thread.
#[derive(Debug)]
pub struct DocCtx {
    /// The document's `FileId`.
    pub file: FileId,
    /// The document text (shared with the salsa input).
    pub text: Arc<str>,
    /// Line index for `text`.
    pub line_index: LineIndex,
    /// The negotiated position encoding.
    pub encoding: PositionEncoding,
}

impl DocCtx {
    fn at(&self, offset: u32) -> FilePosition {
        FilePosition {
            file: self.file,
            offset,
        }
    }
}

pub fn hover(a: &Analysis, ctx: &DocCtx, offset: u32) -> Cancellable<Option<lsp::Hover>> {
    Ok(a.hover(ctx.at(offset))?
        .map(|h| convert::hover_to_lsp(&ctx.line_index, &ctx.text, &h, ctx.encoding)))
}

pub fn completion(a: &Analysis, ctx: &DocCtx, offset: u32) -> Cancellable<lsp::CompletionResponse> {
    let items = a
        .completions(ctx.at(offset))?
        .iter()
        .map(convert::completion_to_lsp)
        .collect();
    Ok(lsp::CompletionResponse::Array(items))
}

pub fn signature_help(
    a: &Analysis,
    ctx: &DocCtx,
    offset: u32,
) -> Cancellable<Option<lsp::SignatureHelp>> {
    Ok(a.signature_help(ctx.at(offset))?
        .map(|s| convert::signature_help_to_lsp(&s)))
}

pub fn document_symbols(a: &Analysis, ctx: &DocCtx) -> Cancellable<lsp::DocumentSymbolResponse> {
    let symbols = a
        .document_symbols(ctx.file)?
        .iter()
        .map(|s| convert::document_symbol_to_lsp(&ctx.line_index, &ctx.text, s, ctx.encoding))
        .collect();
    Ok(lsp::DocumentSymbolResponse::Nested(symbols))
}

pub fn folding_ranges(a: &Analysis, ctx: &DocCtx) -> Cancellable<Vec<lsp::FoldingRange>> {
    Ok(a.folding_ranges(ctx.file)?
        .iter()
        .map(|f| convert::folding_range_to_lsp(&ctx.line_index, &ctx.text, f, ctx.encoding))
        .collect())
}
