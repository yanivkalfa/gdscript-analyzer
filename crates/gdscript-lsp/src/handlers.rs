//! Read-request handlers. Each takes a cloned [`Analysis`] snapshot + a [`DocCtx`] (the target
//! document's text, line index, and encoding) and returns a [`Cancellable`] LSP result. They run on
//! the read thread-pool; a concurrent edit unwinds the salsa query to `Err(Cancelled)`, which the
//! dispatcher maps to LSP `ContentModified` so the client re-requests.

use std::collections::HashMap;
use std::sync::Arc;

use gdscript_base::{Cancellable, FileId, FilePosition, RenameError, SourceChange, TextRange};
use gdscript_ide::Analysis;
use lsp_types as lsp;
use lsp_types::Uri;

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

/// `textDocument/inlayHint` — all hints for the file. (The LSP request carries a visible range; we
/// return the whole file's hints and let the client filter — they're cheap.)
pub fn inlay_hints(a: &Analysis, ctx: &DocCtx) -> Cancellable<Vec<lsp::InlayHint>> {
    Ok(a.inlay_hints(ctx.file)?
        .iter()
        .map(|h| convert::inlay_hint_to_lsp(&ctx.line_index, &ctx.text, h, ctx.encoding))
        .collect())
}

/// `textDocument/semanticTokens/full` — the whole file's tokens, 5-int relative-encoded.
pub fn semantic_tokens(
    a: &Analysis,
    ctx: &DocCtx,
) -> Cancellable<Option<lsp::SemanticTokensResult>> {
    let data = convert::encode_semantic_tokens(
        &ctx.line_index,
        &ctx.text,
        &a.semantic_tokens(ctx.file)?,
        ctx.encoding,
    );
    Ok(Some(lsp::SemanticTokensResult::Tokens(
        lsp::SemanticTokens {
            result_id: None,
            data,
        },
    )))
}

// ---- M3: navigation (cross-file results need a FileId → URI/position map) ---------------------

/// One open document's URI + the data needed to turn its byte ranges into LSP `Location`s.
#[derive(Debug)]
pub struct NavDoc {
    /// The document's URI.
    pub uri: Uri,
    /// Its text.
    pub text: Arc<str>,
    /// Its line index.
    pub line_index: LineIndex,
}

/// A snapshot of every **known** file (open overlay or scanned-from-disk), so a navigation result in
/// any project file maps to a `Location`. A result in a file outside the loaded project (none, when a
/// `project.godot` root was found) is skipped. Built on the main thread, moved to the worker.
#[derive(Debug)]
pub struct NavCtx {
    /// Known project files by `FileId`.
    pub docs: HashMap<FileId, NavDoc>,
    /// The negotiated encoding.
    pub encoding: PositionEncoding,
}

impl NavCtx {
    /// A `(file, range)` → an LSP `Location`, or `None` if `file` isn't open.
    fn location(&self, file: FileId, range: TextRange) -> Option<lsp::Location> {
        let doc = self.docs.get(&file)?;
        Some(lsp::Location {
            uri: doc.uri.clone(),
            range: convert::range_to_lsp(&doc.line_index, &doc.text, range, self.encoding),
        })
    }
}

pub fn goto_definition(
    a: &Analysis,
    nav: &NavCtx,
    file: FileId,
    offset: u32,
) -> Cancellable<Option<lsp::GotoDefinitionResponse>> {
    let locations = a
        .goto_definition(FilePosition { file, offset })?
        .iter()
        .filter_map(|t| nav.location(t.file, t.focus_range))
        .collect();
    Ok(Some(lsp::GotoDefinitionResponse::Array(locations)))
}

pub fn references(
    a: &Analysis,
    nav: &NavCtx,
    file: FileId,
    offset: u32,
) -> Cancellable<Option<Vec<lsp::Location>>> {
    let locations = a
        .find_references(FilePosition { file, offset })?
        .iter()
        .filter_map(|r| nav.location(r.file, r.range))
        .collect();
    Ok(Some(locations))
}

pub fn workspace_symbols(
    a: &Analysis,
    nav: &NavCtx,
    query: &str,
) -> Cancellable<Option<Vec<lsp::WorkspaceSymbol>>> {
    let symbols = a
        .workspace_symbols(query)?
        .iter()
        .filter_map(|t| {
            Some(lsp::WorkspaceSymbol {
                name: t.name.clone(),
                kind: convert::symbol_kind_to_lsp(t.kind),
                location: lsp::OneOf::Left(nav.location(t.file, t.focus_range)?),
                container_name: None,
                tags: None,
                data: None,
            })
        })
        .collect();
    Ok(Some(symbols))
}

impl NavCtx {
    /// A POD [`SourceChange`] → an LSP [`WorkspaceEdit`](lsp::WorkspaceEdit), or `None` if any edited
    /// file isn't in the loaded project — a rename/quick-fix must be **all-or-nothing**, so we'd
    /// rather refuse than emit a partial edit that leaves a stale reference behind.
    #[allow(
        clippy::mutable_key_type,
        reason = "lsp_types::Uri's interior cache never affects Hash/Eq"
    )]
    fn workspace_edit(&self, change: &SourceChange) -> Option<lsp::WorkspaceEdit> {
        let mut changes = HashMap::new();
        for fe in &change.edits {
            let doc = self.docs.get(&fe.file)?;
            let edits = fe
                .edits
                .iter()
                .map(|e| lsp::TextEdit {
                    range: convert::range_to_lsp(
                        &doc.line_index,
                        &doc.text,
                        e.range,
                        self.encoding,
                    ),
                    new_text: e.new_text.clone(),
                })
                .collect();
            changes.insert(doc.uri.clone(), edits);
        }
        Some(lsp::WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }
}

/// A human-readable reason a rename was refused (for the LSP error message).
fn rename_error_message(error: &RenameError) -> String {
    match error {
        RenameError::InvalidIdentifier { new_name } => {
            format!("`{new_name}` is not a valid GDScript identifier")
        }
        RenameError::NotRenamable { reason } => format!("cannot rename: {reason}"),
        RenameError::WouldCollide { with, .. } => {
            format!("a symbol named `{with}` already exists in scope")
        }
        RenameError::CrossesUnsupportedBoundary { what } => {
            format!("rename would cross an unsupported boundary ({what})")
        }
    }
}

/// `textDocument/rename` → a `WorkspaceEdit`, or `Err((code, message))` if refused (an invalid name,
/// a collision, an engine symbol, an unsupported boundary, or an edit reaching an un-opened file).
pub fn rename(
    a: &Analysis,
    nav: &NavCtx,
    file: FileId,
    offset: u32,
    new_name: &str,
) -> Cancellable<Result<lsp::WorkspaceEdit, (i32, String)>> {
    Ok(match a.rename(FilePosition { file, offset }, new_name)? {
        Ok(change) => nav.workspace_edit(&change).ok_or_else(|| {
            (
                crate::REQUEST_FAILED,
                "rename affects a file outside the loaded project".to_owned(),
            )
        }),
        Err(error) => Err((crate::REQUEST_FAILED, rename_error_message(&error))),
    })
}

/// `textDocument/prepareRename` → the range of the symbol under the cursor if it's renameable (uses
/// find-references, which is non-empty only for user symbols), else `None`.
pub fn prepare_rename(
    a: &Analysis,
    ctx: &DocCtx,
    offset: u32,
) -> Cancellable<Option<lsp::PrepareRenameResponse>> {
    // `TextRange` is half-open `[start, end)`, so containment is `start <= offset < end` (matching
    // def.rs/infer.rs) — a cursor at the exclusive end byte is not inside the token.
    let range = a
        .find_references(ctx.at(offset))?
        .iter()
        .find(|r| r.file == ctx.file && r.range.start <= offset && offset < r.range.end)
        .map(|r| convert::range_to_lsp(&ctx.line_index, &ctx.text, r.range, ctx.encoding));
    Ok(range.map(lsp::PrepareRenameResponse::Range))
}

/// `textDocument/codeAction` → the quick-fixes at the position (those whose edit stays within open
/// files).
pub fn code_actions(
    a: &Analysis,
    nav: &NavCtx,
    file: FileId,
    offset: u32,
) -> Cancellable<Option<Vec<lsp::CodeActionOrCommand>>> {
    let actions = a
        .code_actions(FilePosition { file, offset })?
        .iter()
        .filter_map(|ca| {
            Some(lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                title: ca.title.clone(),
                kind: ca.kind.clone().map(lsp::CodeActionKind::from),
                edit: Some(nav.workspace_edit(&ca.edit)?),
                ..Default::default()
            }))
        })
        .collect();
    Ok(Some(actions))
}
