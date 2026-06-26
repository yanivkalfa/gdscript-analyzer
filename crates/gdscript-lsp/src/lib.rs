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
//! incremental text sync + the URI↔`FileId` [`vfs`] + push diagnostics.
//! **M1 (read features):** hover, completion, signature help, document symbols, folding ranges —
//! each snapshotted and run on a worker thread ([`handlers`]); a concurrent edit unwinds the salsa
//! query to `Cancelled`, mapped to LSP `ContentModified`. Semantic tokens + inlay hints are M2.

pub mod convert;
pub mod line_index;
pub mod vfs;

mod handlers;

use anyhow::Result;
use crossbeam_channel::Sender;
use gdscript_base::Cancellable;
use gdscript_ide::{Analysis, AnalysisHost, Change};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, FoldingRangeProviderCapability, HoverProviderCapability,
    InitializeParams, InitializeResult, OneOf, PositionEncodingKind, PublishDiagnosticsParams,
    ServerCapabilities, ServerInfo, SignatureHelpOptions, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentPositionParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri,
};

use crate::convert::diagnostic_to_lsp;
use crate::handlers::DocCtx;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::vfs::Vfs;

/// LSP error code for a read superseded by a concurrent edit — the client re-requests.
pub const CONTENT_MODIFIED: i32 = -32801;
/// LSP error code for an unimplemented request method.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// LSP error code for malformed request params.
pub const INVALID_PARAMS: i32 = -32602;
/// LSP error code for an internal failure (e.g. a query panicked) — the request still gets a reply.
pub const INTERNAL_ERROR: i32 = -32603;
/// LSP error code for a request received after `shutdown`.
pub const INVALID_REQUEST: i32 = -32600;

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

/// The capabilities we advertise. **Data-driven by what's wired**: the negotiated position encoding
/// and incremental text sync (M0; diagnostics are pushed, needing no capability), plus the M1 read
/// features. Later features (semantic tokens, inlay hints, rename, …) join as their milestones land.
#[must_use]
pub fn server_capabilities(encoding: PositionEncoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.to_lsp()),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        // M1 read features.
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(
                [".", "$", "%", "/", "\""]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
            ),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            ..Default::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
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

/// The event loop. Edits (text sync) and request *dispatch* run on this thread (the single writer);
/// read requests are snapshotted and computed on a worker thread, their `Response`s arriving back on
/// `task_rx`. A `select!` multiplexes the two so a slow read never blocks edits or other requests.
fn main_loop(conn: &Connection, encoding: PositionEncoding) -> Result<()> {
    let (task_tx, task_rx) = crossbeam_channel::unbounded::<Message>();
    let mut state = GlobalState::new(encoding, task_tx);
    // `shutdown` is acked manually (not via `Connection::handle_shutdown`, which blocks) so the loop
    // keeps draining in-flight read responses to the client until `exit` — never leaving a read
    // request unanswered (the client would otherwise hang).
    let mut shutting_down = false;
    loop {
        crossbeam_channel::select! {
            recv(conn.receiver) -> msg => {
                let Ok(msg) = msg else { return Ok(()) }; // client disconnected — nothing to flush to
                match msg {
                    Message::Request(req) => {
                        if req.method == "shutdown" {
                            conn.sender.send(Message::Response(Response::new_ok(req.id, ())))?;
                            shutting_down = true;
                        } else if shutting_down {
                            // Per spec, requests after `shutdown` are rejected.
                            conn.sender.send(Message::Response(Response::new_err(
                                req.id,
                                INVALID_REQUEST,
                                "server is shutting down".to_owned(),
                            )))?;
                        } else {
                            state.handle_request(req);
                        }
                    }
                    Message::Notification(note) => {
                        if note.method == "exit" {
                            return Ok(());
                        }
                        state.on_notification(conn, &note)?;
                    }
                    Message::Response(_) => {}
                }
            }
            recv(task_rx) -> done => {
                if let Ok(msg) = done {
                    conn.sender.send(msg)?; // a finished read's Response → the client (incl. during shutdown→exit)
                }
            }
        }
    }
}

/// The single mutable owner of server state (main-thread only): the analysis host, the document
/// VFS, the negotiated encoding, and the channel finished reads post their `Response`s to.
struct GlobalState {
    host: AnalysisHost,
    vfs: Vfs,
    encoding: PositionEncoding,
    task_tx: Sender<Message>,
}

impl GlobalState {
    fn new(encoding: PositionEncoding, task_tx: Sender<Message>) -> Self {
        Self {
            host: AnalysisHost::new(),
            vfs: Vfs::default(),
            encoding,
            task_tx,
        }
    }

    /// Dispatch a request: read features snapshot the analysis and run on a worker thread; unknown
    /// methods get `MethodNotFound` (so clients never hang).
    fn handle_request(&self, req: Request) {
        let method = req.method.clone();
        match method.as_str() {
            "textDocument/hover" => self.spawn_pos(req, handlers::hover),
            "textDocument/completion" => self.spawn_pos(req, handlers::completion),
            "textDocument/signatureHelp" => self.spawn_pos(req, handlers::signature_help),
            "textDocument/documentSymbol" => self.spawn_file(req, handlers::document_symbols),
            "textDocument/foldingRange" => self.spawn_file(req, handlers::folding_ranges),
            other => self.send(Response::new_err(
                req.id,
                METHOD_NOT_FOUND,
                format!("gdscript-lsp: '{other}' not implemented"),
            )),
        }
    }

    /// Queue a `Response` to the client (via the task channel, so it interleaves with worker
    /// results on the one writer thread).
    fn send(&self, resp: Response) {
        let _ = self.task_tx.send(Message::Response(resp));
    }

    /// The read context for `uri`, or `None` if the document isn't open.
    fn doc_ctx(&self, uri: &Uri) -> Option<DocCtx> {
        let id = self.vfs.id(uri)?;
        let doc = self.vfs.doc(id)?;
        Some(DocCtx {
            file: id,
            text: doc.text.clone(),
            line_index: doc.line_index.clone(),
            encoding: self.encoding,
        })
    }

    /// Dispatch a position-based read (`hover`/`completion`/`signatureHelp`): map the LSP position to
    /// a byte offset on this thread, then compute on a worker.
    fn spawn_pos<F, R>(&self, req: Request, handler: F)
    where
        F: FnOnce(&Analysis, &DocCtx, u32) -> Cancellable<R> + Send + 'static,
        R: serde::Serialize,
    {
        let id = req.id.clone();
        let params: TextDocumentPositionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return self.send(Response::new_err(id, INVALID_PARAMS, e.to_string())),
        };
        let Some(ctx) = self.doc_ctx(&params.text_document.uri) else {
            return self.respond_null(id);
        };
        let offset = ctx
            .line_index
            .offset(&ctx.text, params.position, ctx.encoding);
        self.spawn(id, move |a| handler(a, &ctx, offset));
    }

    /// Dispatch a whole-file read (`documentSymbol`/`foldingRange`).
    fn spawn_file<F, R>(&self, req: Request, handler: F)
    where
        F: FnOnce(&Analysis, &DocCtx) -> Cancellable<R> + Send + 'static,
        R: serde::Serialize,
    {
        let id = req.id.clone();
        let params: FileParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return self.send(Response::new_err(id, INVALID_PARAMS, e.to_string())),
        };
        let Some(ctx) = self.doc_ctx(&params.text_document.uri) else {
            return self.respond_null(id);
        };
        self.spawn(id, move |a| handler(a, &ctx));
    }

    /// Run `compute` against a fresh analysis snapshot on a worker thread; post its `Response`. A
    /// salsa `Cancelled` (a concurrent edit invalidated the snapshot) → LSP `ContentModified`.
    fn spawn<F, R>(&self, id: RequestId, compute: F)
    where
        F: FnOnce(&Analysis) -> Cancellable<R> + Send + 'static,
        R: serde::Serialize,
    {
        let analysis = self.host.analysis();
        let tx = self.task_tx.clone();
        std::thread::spawn(move || {
            // Catch a query panic (a real bug — `Cancelled` is an `Err`, not a panic) so the client
            // always gets a reply instead of hanging forever on this request id. Salsa uses
            // non-poisoning locks, so discarding the panicked snapshot leaves the host intact.
            let computed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compute(&analysis)));
            let resp = match computed {
                Ok(Ok(value)) => Response::new_ok(id, value),
                Ok(Err(_cancelled)) => {
                    Response::new_err(id, CONTENT_MODIFIED, "content modified".to_owned())
                }
                Err(_panic) => Response::new_err(id, INTERNAL_ERROR, "internal error".to_owned()),
            };
            let _ = tx.send(Message::Response(resp));
        });
    }

    /// Respond with `null` (a valid "nothing here" answer for the read requests).
    fn respond_null(&self, id: RequestId) {
        self.send(Response {
            id,
            result: Some(serde_json::Value::Null),
            error: None,
        });
    }

    fn on_notification(&mut self, conn: &Connection, note: &Notification) -> Result<()> {
        match note.method.as_str() {
            "textDocument/didOpen" => {
                let Ok(p) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(note.params.clone())
                else {
                    return Ok(()); // ignore a malformed notification (LSP: notifications get no reply)
                };
                let td = p.text_document;
                let id = self.vfs.upsert(&td.uri, td.text, td.version);
                self.commit(id);
                self.publish_diagnostics(conn, id)?;
            }
            "textDocument/didChange" => {
                let Ok(p) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(note.params.clone())
                else {
                    return Ok(());
                };
                let uri = p.text_document.uri;
                // Ignore a `didChange` for an un-opened / already-closed document (a malformed
                // client) — never rebuild text from an empty buffer.
                let Some(id) = self.vfs.id(&uri).filter(|&id| self.vfs.doc(id).is_some()) else {
                    return Ok(());
                };
                let new_text = self.apply_content_changes(id, &p.content_changes);
                self.vfs.upsert(&uri, new_text, p.text_document.version);
                self.commit(id);
                self.publish_diagnostics(conn, id)?;
            }
            "textDocument/didClose" => {
                let Ok(p) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(note.params.clone())
                else {
                    return Ok(());
                };
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
                let a = li.offset(&text, range.start, self.encoding) as usize;
                let b = li.offset(&text, range.end, self.encoding) as usize;
                // Tolerate a malformed reversed range (`start > end`) instead of panicking the
                // server thread via `replace_range`. Both offsets are already char boundaries.
                let (start, end) = (a.min(b), a.max(b));
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

/// The shared shape of whole-file request params (`documentSymbol`/`foldingRange`): the target doc.
#[derive(serde::Deserialize)]
struct FileParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
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

    /// A `GlobalState` with a throwaway task channel (for unit tests that don't drive the loop).
    fn test_state() -> GlobalState {
        GlobalState::new(PositionEncoding::Utf16, crossbeam_channel::unbounded().0)
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
    fn reversed_range_didchange_is_tolerated_not_a_panic() {
        // A malformed reversed range (`start > end`) must normalize, not crash the server thread
        // via `String::replace_range`.
        let mut state = test_state();
        let u = uri("file:///a.gd");
        state.vfs.upsert(&u, "abcdef\n".to_owned(), 1);
        let id = state.vfs.id(&u).unwrap();
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(
                lsp_types::Position::new(0, 5),
                lsp_types::Position::new(0, 2),
            )),
            range_length: None,
            text: "X".to_owned(),
        }];
        // start=5 > end=2 → normalized to (2,5): "cde" → "X" ⇒ "abXf\n".
        assert_eq!(state.apply_content_changes(id, &changes), "abXf\n");
    }

    #[test]
    fn didchange_for_a_closed_document_is_ignored() {
        // After didClose the id stays interned but the overlay is gone; a stray didChange must not
        // resurrect the doc from an empty buffer (the handler guards on `doc(id).is_some()`).
        let mut state = test_state();
        let u = uri("file:///a.gd");
        let id = state.vfs.upsert(&u, "extends Node\n".to_owned(), 1);
        state.vfs.close(id);
        assert_eq!(
            state.vfs.id(&u),
            Some(id),
            "the id stays interned after close"
        );
        assert!(
            state.vfs.doc(id).is_none(),
            "the overlay is dropped, so the didChange guard (doc(id).is_some()) rejects it",
        );
    }

    fn has_symbol(syms: &[lsp_types::DocumentSymbol], name: &str) -> bool {
        syms.iter()
            .any(|s| s.name == name || s.children.as_deref().is_some_and(|c| has_symbol(c, name)))
    }

    #[test]
    fn read_features_respond_over_the_public_api() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));

        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        let init: InitializeResult = serde_json::from_value(init.result.unwrap()).unwrap();
        let caps = &init.capabilities;
        assert!(caps.hover_provider.is_some(), "hover advertised");
        assert!(caps.completion_provider.is_some(), "completion advertised");
        assert!(
            caps.document_symbol_provider.is_some(),
            "documentSymbol advertised"
        );
        assert!(
            caps.folding_range_provider.is_some(),
            "foldingRange advertised"
        );
        send_note(&client, "initialized", InitializedParams {});

        let doc_uri = uri("file:///main.gd");
        send_note(
            &client,
            "textDocument/didOpen",
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: doc_uri.clone(),
                    language_id: "gdscript".to_owned(),
                    version: 1,
                    text: "extends Node\nfunc greet() -> int:\n\treturn 1\n".to_owned(),
                },
            },
        );
        let _ = next_diagnostics(&client);
        let u = doc_uri.as_str();

        // documentSymbol → the `greet` function symbol is present.
        send_req(
            &client,
            2,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": u } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "documentSymbol errored: {resp:?}");
        let symbols: lsp_types::DocumentSymbolResponse =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        let lsp_types::DocumentSymbolResponse::Nested(symbols) = symbols else {
            panic!("expected nested document symbols");
        };
        assert!(
            has_symbol(&symbols, "greet"),
            "expected a `greet` symbol: {symbols:?}"
        );

        // hover on `Node` (line 0, char 9) → a well-formed (deserializable) response, no error.
        send_req(
            &client,
            3,
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 0, "character": 9 } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "hover errored: {resp:?}");
        let _: Option<lsp_types::Hover> = serde_json::from_value(resp.result.unwrap()).unwrap();

        // completion at the start of a line → an Array response, no error.
        send_req(
            &client,
            4,
            "textDocument/completion",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 2, "character": 1 } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "completion errored: {resp:?}");
        let completions: lsp_types::CompletionResponse =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(matches!(
            completions,
            lsp_types::CompletionResponse::Array(_)
        ));

        // an unknown request method → MethodNotFound (clients never hang).
        send_req(
            &client,
            5,
            "textDocument/notARealMethod",
            serde_json::json!({}),
        );
        let resp = next_response(&client);
        assert_eq!(resp.error.map(|e| e.code), Some(METHOD_NOT_FOUND));

        send_req(&client, 9, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn malformed_notification_does_not_crash_the_server() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let _ = next_response(&client);
        send_note(&client, "initialized", InitializedParams {});

        // A garbage didOpen (missing textDocument) must be IGNORED, not crash the loop.
        send_note(
            &client,
            "textDocument/didOpen",
            serde_json::json!({ "garbage": true }),
        );
        // The server is still alive: a request for an unopened doc returns null (no hang/crash).
        send_req(
            &client,
            2,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": "file:///x.gd" } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "server should still answer: {resp:?}");
        assert_eq!(
            resp.result,
            Some(serde_json::Value::Null),
            "unopened doc → null"
        );

        send_req(&client, 3, "shutdown", ());
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
