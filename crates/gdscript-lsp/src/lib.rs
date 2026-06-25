//! `gdscript-lsp` — a standalone, spec-compliant GDScript Language Server (Phase 5, Workstream 1).
//!
//! The only crate that knows LSP / JSON-RPC. It is a thin synchronous protocol shell over
//! [`gdscript_ide`]'s `AnalysisHost`/`Analysis` API (salsa-incremental, cancellable, returning POD
//! with `u32` UTF-8 byte offsets). Unlike Godot's built-in editor LSP it needs no running editor and
//! will add semantic tokens, inlay hints, workspace symbols, and cross-file rename.
//!
//! Built on rust-analyzer's own low-level stack (`lsp-server` + `lsp-types` + a hand-rolled event
//! loop) rather than an async framework — our engine is synchronous + cancellation-via-unwind, so
//! async is pure impedance. See `plans/PHASE-5-LSP-PLAYBOOK.md`.
//!
//! **M0 (the spine):** lifecycle + position-encoding negotiation + the [`line_index`] converter +
//! incremental text sync + the URI↔`FileId` [`vfs`] + push diagnostics. Read requests (hover,
//! completion, …) and the read thread-pool arrive in M1+.

pub mod convert;
pub mod line_index;
pub mod vfs;

use anyhow::Result;
use gdscript_ide::{AnalysisHost, Change};
use lsp_server::{Connection, Message, Notification, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, InitializeResult, PositionEncodingKind, PublishDiagnosticsParams,
    ServerCapabilities, ServerInfo, TextDocumentContentChangeEvent, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri,
};

use crate::convert::diagnostic_to_lsp;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::vfs::Vfs;

/// LSP error code for a read superseded by a concurrent edit — the client re-requests. Used once
/// read requests land (M1); defined here so the cancellation path is in place.
pub const CONTENT_MODIFIED: i32 = -32801;
/// LSP error code for an unimplemented request method.
pub const METHOD_NOT_FOUND: i32 = -32601;

/// Choose the position encoding: prefer UTF-8 (our native byte columns), then UTF-32, else the
/// mandatory UTF-16 fallback — driven by what the client advertised in `general.positionEncodings`.
#[must_use]
pub fn negotiate_encoding(params: &InitializeParams) -> PositionEncoding {
    let offered = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref());
    match offered {
        Some(encs) if encs.contains(&PositionEncodingKind::UTF8) => PositionEncoding::Utf8,
        Some(encs) if encs.contains(&PositionEncodingKind::UTF32) => PositionEncoding::Utf32,
        _ => PositionEncoding::Utf16,
    }
}

/// The capabilities we advertise. **Data-driven by what's wired**: M0 ships only the negotiated
/// position encoding + incremental text sync (diagnostics are pushed, needing no capability). Read
/// features are added to this set as their milestones land.
#[must_use]
pub fn server_capabilities(encoding: PositionEncoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.to_lsp()),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        ..Default::default()
    }
}

/// Drive the full server lifecycle over `connection`: the `initialize` handshake (negotiating the
/// position encoding from the client's capabilities) then the event loop. Returns when the client
/// completes the `shutdown`→`exit` sequence.
///
/// # Errors
/// Propagates a protocol/transport error, or a JSON (de)serialization failure.
pub fn run(connection: &Connection) -> Result<()> {
    let (id, init_value) = connection.initialize_start()?;
    let init_params: InitializeParams = serde_json::from_value(init_value)?;
    let encoding = negotiate_encoding(&init_params);
    let result = InitializeResult {
        capabilities: server_capabilities(encoding),
        server_info: Some(ServerInfo {
            name: "gdscript-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;
    main_loop(connection, encoding)
}

/// The minimal correct loop: writes (text sync) mutate the single `AnalysisHost` on this thread;
/// `shutdown`/`exit` end it. (Read requests + their thread-pool dispatch land in M1.)
fn main_loop(conn: &Connection, encoding: PositionEncoding) -> Result<()> {
    let mut state = GlobalState::new(encoding);
    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
                    return Ok(());
                }
                // No read requests implemented yet (M1) → MethodNotFound, so clients don't hang.
                let resp = Response::new_err(
                    req.id,
                    METHOD_NOT_FOUND,
                    format!("gdscript-lsp: '{}' not implemented yet", req.method),
                );
                conn.sender.send(Message::Response(resp))?;
            }
            Message::Notification(note) => state.on_notification(conn, &note)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// The single mutable owner of server state (main-thread only): the analysis host, the document
/// VFS, and the negotiated encoding.
struct GlobalState {
    host: AnalysisHost,
    vfs: Vfs,
    encoding: PositionEncoding,
}

impl GlobalState {
    fn new(encoding: PositionEncoding) -> Self {
        Self {
            host: AnalysisHost::new(),
            vfs: Vfs::default(),
            encoding,
        }
    }

    fn on_notification(&mut self, conn: &Connection, note: &Notification) -> Result<()> {
        match note.method.as_str() {
            "textDocument/didOpen" => {
                let p: DidOpenTextDocumentParams = serde_json::from_value(note.params.clone())?;
                let td = p.text_document;
                let id = self.vfs.upsert(&td.uri, td.text, td.version);
                self.commit(id);
                self.publish_diagnostics(conn, id)?;
            }
            "textDocument/didChange" => {
                let p: DidChangeTextDocumentParams = serde_json::from_value(note.params.clone())?;
                let uri = p.text_document.uri;
                let Some(id) = self.vfs.id(&uri) else {
                    return Ok(());
                };
                let new_text = self.apply_content_changes(id, &p.content_changes);
                self.vfs.upsert(&uri, new_text, p.text_document.version);
                self.commit(id);
                self.publish_diagnostics(conn, id)?;
            }
            "textDocument/didClose" => {
                let p: DidCloseTextDocumentParams = serde_json::from_value(note.params.clone())?;
                if let Some(id) = self.vfs.id(&p.text_document.uri) {
                    self.vfs.close(id);
                    clear_diagnostics(conn, p.text_document.uri)?;
                }
            }
            // `initialized` is informational; `exit` is consumed by `handle_shutdown`.
            _ => {}
        }
        Ok(())
    }

    /// Feed the document's current text to the salsa input (the only mutation of the host).
    fn commit(&mut self, id: gdscript_base::FileId) {
        let Some(text) = self.vfs.doc(id).map(|d| d.text.clone()) else {
            return;
        };
        let mut change = Change::new();
        change.change_file(id, text);
        self.host.apply_change(change);
    }

    /// Rebuild a document's text by applying incremental content changes in order (each relative to
    /// the text after the previous), mapping LSP ranges to byte offsets through the live line index.
    fn apply_content_changes(
        &self,
        id: gdscript_base::FileId,
        changes: &[TextDocumentContentChangeEvent],
    ) -> String {
        let Some(doc) = self.vfs.doc(id) else {
            return String::new();
        };
        let mut text = doc.text.to_string();
        let mut li = doc.line_index.clone();
        for change in changes {
            if let Some(range) = change.range {
                let start = li.offset(&text, range.start, self.encoding) as usize;
                let end = li.offset(&text, range.end, self.encoding) as usize;
                text.replace_range(start..end, &change.text);
            } else {
                text.clone_from(&change.text); // full-document replacement
            }
            li = LineIndex::new(&text);
        }
        text
    }

    fn publish_diagnostics(&self, conn: &Connection, id: gdscript_base::FileId) -> Result<()> {
        let Some(doc) = self.vfs.doc(id) else {
            return Ok(());
        };
        let Some(uri) = self.vfs.uri(id).cloned() else {
            return Ok(());
        };
        // Computed synchronously right after the edit on this thread, so the snapshot can't be
        // cancelled here; `Cancelled` (an M1 concern for async reads) degrades to no diagnostics.
        let diagnostics = self
            .host
            .analysis()
            .diagnostics(id)
            .unwrap_or_default()
            .iter()
            .map(|d| diagnostic_to_lsp(&doc.line_index, &doc.text, d, self.encoding))
            .collect();
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: Some(doc.version),
        };
        send_notification(conn, "textDocument/publishDiagnostics", &params)
    }
}

/// Clear diagnostics for a closed document (a `publishDiagnostics` with an empty list).
fn clear_diagnostics(conn: &Connection, uri: Uri) -> Result<()> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics: Vec::new(),
        version: None,
    };
    send_notification(conn, "textDocument/publishDiagnostics", &params)
}

fn send_notification(
    conn: &Connection,
    method: &str,
    params: &impl serde::Serialize,
) -> Result<()> {
    let note = Notification::new(method.to_owned(), serde_json::to_value(params)?);
    conn.sender.send(Message::Notification(note))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::{Request, RequestId};
    use lsp_types::{
        InitializedParams, NumberOrString, TextDocumentItem, VersionedTextDocumentIdentifier,
    };

    fn send_req(client: &Connection, id: i32, method: &str, params: impl serde::Serialize) {
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(id),
                method.to_owned(),
                serde_json::to_value(params).unwrap(),
            )))
            .unwrap();
    }

    fn send_note(client: &Connection, method: &str, params: impl serde::Serialize) {
        client
            .sender
            .send(Message::Notification(Notification::new(
                method.to_owned(),
                serde_json::to_value(params).unwrap(),
            )))
            .unwrap();
    }

    /// Pull messages until a `Response` arrives (skipping interleaved notifications).
    fn next_response(client: &Connection) -> Response {
        loop {
            if let Message::Response(r) = client.receiver.recv().unwrap() {
                return r;
            }
        }
    }

    /// Pull messages until a `publishDiagnostics` notification arrives.
    fn next_diagnostics(client: &Connection) -> PublishDiagnosticsParams {
        loop {
            if let Message::Notification(n) = client.receiver.recv().unwrap()
                && n.method == "textDocument/publishDiagnostics"
            {
                return serde_json::from_value(n.params).unwrap();
            }
        }
    }

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn open_then_edit_publishes_diagnostics_at_correct_ranges() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));

        // initialize handshake (no positionEncodings advertised → UTF-16 fallback).
        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        assert!(init.error.is_none(), "initialize failed: {init:?}");
        send_note(&client, "initialized", InitializedParams {});

        // didOpen a doc with a known diagnostic: `1 / 2` is INTEGER_DIVISION on line 1.
        let doc_uri = uri("file:///main.gd");
        send_note(
            &client,
            "textDocument/didOpen",
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: doc_uri.clone(),
                    language_id: "gdscript".to_owned(),
                    version: 1,
                    text: "func f():\n\tvar x := 1 / 2\n".to_owned(),
                },
            },
        );
        let diags = next_diagnostics(&client);
        assert_eq!(diags.uri, doc_uri);
        let intdiv = diags
            .diagnostics
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("INTEGER_DIVISION".to_owned())))
            .expect("expected INTEGER_DIVISION diagnostic");
        assert_eq!(
            intdiv.range.start.line, 1,
            "diagnostic must be on the `1 / 2` line"
        );
        assert_eq!(intdiv.source.as_deref(), Some("gdscript"));

        // an incremental edit that removes the division → the diagnostic clears.
        send_note(
            &client,
            "textDocument/didChange",
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: doc_uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(lsp_types::Range::new(
                        lsp_types::Position::new(1, 10), // start of `1 / 2`
                        lsp_types::Position::new(1, 15), // end of `1 / 2`
                    )),
                    range_length: None,
                    text: "0".to_owned(),
                }],
            },
        );
        let after = next_diagnostics(&client);
        assert!(
            !after
                .diagnostics
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("INTEGER_DIVISION".to_owned()))),
            "edit should have cleared the integer-division diagnostic: {:?}",
            after.diagnostics,
        );

        // shutdown / exit.
        send_req(&client, 2, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn capabilities_advertise_negotiated_encoding_and_incremental_sync() {
        let caps = server_capabilities(PositionEncoding::Utf8);
        assert_eq!(caps.position_encoding, Some(PositionEncodingKind::UTF8));
        assert!(matches!(
            caps.text_document_sync,
            Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL
            ))
        ));
    }
}
