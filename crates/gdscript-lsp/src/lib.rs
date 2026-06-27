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
//! query to `Cancelled`, mapped to LSP `ContentModified`.
//! **M2:** semantic tokens (the 5-int relative encoding + legend) and inlay hints.
//! **M3:** navigation (definition, references, workspace symbols) and refactor (rename +
//! prepareRename, code actions) — cross-file results map through a [`NavCtx`](handlers::NavCtx)
//! snapshot of the open documents; a rename/quick-fix touching an un-opened file is refused
//! (all-or-nothing).

pub mod convert;
pub mod line_index;
pub mod project;
pub mod vfs;

mod handlers;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Sender;
use gdscript_base::{Cancellable, FileId};
use gdscript_ide::{Analysis, AnalysisHost, Change};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CodeActionParams, CodeActionProviderCapability, CompletionOptions, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, FileChangeType, FileSystemWatcher,
    FoldingRangeProviderCapability, GlobPattern, HoverProviderCapability, InitializeParams,
    InitializeResult, OneOf, PositionEncodingKind, PublishDiagnosticsParams, Registration,
    RegistrationParams, RenameOptions, RenameParams, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo,
    SignatureHelpOptions, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
    WorkspaceSymbolParams,
};

use crate::convert::diagnostic_to_lsp;
use crate::handlers::{DocCtx, NavCtx, NavDoc};
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
/// LSP error code for a request that failed for a known reason (e.g. a rename was refused).
pub const REQUEST_FAILED: i32 = -32803;

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
        // M2.
        inlay_hint_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: convert::semantic_tokens_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(false),
                ..Default::default()
            },
        )),
        // M3 navigation + refactor.
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        })),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
    let roots = workspace_roots(&init_params);
    let result = InitializeResult {
        capabilities: server_capabilities(encoding),
        server_info: Some(ServerInfo {
            name: "gdscript-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;
    main_loop(connection, encoding, roots)
}

/// The workspace roots to scan: the client's `workspace_folders` (preferred), else the deprecated
/// single `root_uri`. Empty when the client opened no folder (the server then runs per-open-file).
fn workspace_roots(params: &InitializeParams) -> Vec<Uri> {
    if let Some(folders) = &params.workspace_folders
        && !folders.is_empty()
    {
        return folders.iter().map(|f| f.uri.clone()).collect();
    }
    #[allow(deprecated)]
    params.root_uri.clone().into_iter().collect()
}

/// The event loop. Edits (text sync) and request *dispatch* run on this thread (the single writer);
/// read requests are snapshotted and computed on a worker thread, their `Response`s arriving back on
/// `task_rx`. A `select!` multiplexes the two so a slow read never blocks edits or other requests.
fn main_loop(conn: &Connection, encoding: PositionEncoding, roots: Vec<Uri>) -> Result<()> {
    let (task_tx, task_rx) = crossbeam_channel::unbounded::<Message>();
    let mut state = GlobalState::new(encoding, task_tx, roots);
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
    /// The workspace roots to scan once `initialized` arrives.
    roots: Vec<Uri>,
    /// Whether the project scan has already run (idempotent — `initialized` fires once).
    loaded: bool,
    /// Files whose `res://` path has been fed to the host (so it's set exactly once).
    with_path: HashSet<FileId>,
    /// The scanned project root (the `project.godot` dir) — for computing a `res://` path for a file
    /// created later (a `didChangeWatchedFiles` Created event).
    project_root: Option<PathBuf>,
}

impl GlobalState {
    fn new(encoding: PositionEncoding, task_tx: Sender<Message>, roots: Vec<Uri>) -> Self {
        Self {
            host: AnalysisHost::new(),
            vfs: Vfs::default(),
            encoding,
            task_tx,
            roots,
            loaded: false,
            with_path: HashSet::new(),
            project_root: None,
        }
    }

    /// Scan the workspace roots into the one host: every `.gd`/`.tscn` as a background file (with its
    /// `res://` path), plus `project.godot` — so cross-file resolution (`class_name`, autoloads,
    /// preload, scene typing) works against the whole project, not just open documents. Idempotent and
    /// a no-op when no root resolves to a `project.godot` (the per-open-file fallback).
    fn load_project(&mut self) {
        if self.loaded || self.roots.is_empty() {
            return;
        }
        self.loaded = true;
        let loaded = project::load(&self.roots);
        if loaded.is_empty() {
            return;
        }
        let mut change = Change::new();
        for f in &loaded.files {
            let id = self.vfs.set_disk(&f.uri, Arc::clone(&f.text));
            change.change_file(id, Arc::clone(&f.text));
            if let Some(res) = &f.res_path {
                self.vfs.set_res_path(id, res.clone());
                if self.with_path.insert(id) {
                    change.set_file_path(id, res.clone());
                }
            }
        }
        if let Some(cfg) = &loaded.config {
            change.set_project_config(Arc::clone(cfg));
        }
        self.host.apply_change(change);
        self.project_root = loaded.root; // moved after the borrows above end
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
            "textDocument/inlayHint" => self.spawn_file(req, handlers::inlay_hints),
            "textDocument/semanticTokens/full" => {
                self.spawn_file(req, handlers::semantic_tokens);
            }
            "textDocument/definition" => self.spawn_nav_pos(req, handlers::goto_definition),
            "textDocument/references" => self.spawn_nav_pos(req, handlers::references),
            "workspace/symbol" => self.spawn_nav_query(req, handlers::workspace_symbols),
            "textDocument/prepareRename" => self.spawn_pos(req, handlers::prepare_rename),
            "textDocument/rename" => self.handle_rename(req),
            "textDocument/codeAction" => self.handle_code_actions(req),
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

    /// A navigation snapshot of **every known file** (open overlay or scanned-from-disk), so a
    /// cross-file definition / reference / rename in any project file maps to a `Location` — not just
    /// the handful of open documents.
    fn nav_ctx(&self) -> NavCtx {
        let docs = self
            .vfs
            .known_ids()
            .into_iter()
            .filter_map(|id| {
                let uri = self.vfs.uri(id)?.clone();
                let snap = self.vfs.snapshot(id)?;
                Some((
                    id,
                    NavDoc {
                        uri,
                        text: snap.text,
                        line_index: snap.line_index,
                    },
                ))
            })
            .collect();
        NavCtx {
            docs,
            encoding: self.encoding,
        }
    }

    /// Dispatch a position-based navigation read (`definition`/`references`) with the nav snapshot.
    fn spawn_nav_pos<F, R>(&self, req: Request, handler: F)
    where
        F: FnOnce(&Analysis, &NavCtx, gdscript_base::FileId, u32) -> Cancellable<R>
            + Send
            + 'static,
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
        let (nav, file) = (self.nav_ctx(), ctx.file);
        self.spawn(id, move |a| handler(a, &nav, file, offset));
    }

    /// Dispatch a query-based navigation read (`workspace/symbol`).
    fn spawn_nav_query<F, R>(&self, req: Request, handler: F)
    where
        F: FnOnce(&Analysis, &NavCtx, &str) -> Cancellable<R> + Send + 'static,
        R: serde::Serialize,
    {
        let id = req.id.clone();
        let params: WorkspaceSymbolParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return self.send(Response::new_err(id, INVALID_PARAMS, e.to_string())),
        };
        let nav = self.nav_ctx();
        self.spawn(id, move |a| handler(a, &nav, &params.query));
    }

    /// Run a **fallible** read on the pool: the handler's inner `Err((code, message))` becomes an LSP
    /// error response (e.g. a refused rename); `Ok(value)` → an ok response. `Cancelled`/panic map as
    /// in [`Self::spawn`].
    fn spawn_fallible<F, R>(&self, id: RequestId, compute: F)
    where
        F: FnOnce(&Analysis) -> Cancellable<Result<R, (i32, String)>> + Send + 'static,
        R: serde::Serialize,
    {
        let analysis = self.host.analysis();
        let tx = self.task_tx.clone();
        std::thread::spawn(move || {
            let computed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compute(&analysis)));
            let resp = match computed {
                Ok(Ok(Ok(value))) => Response::new_ok(id, value),
                Ok(Ok(Err((code, message)))) => Response::new_err(id, code, message),
                Ok(Err(_cancelled)) => {
                    Response::new_err(id, CONTENT_MODIFIED, "content modified".to_owned())
                }
                Err(_panic) => Response::new_err(id, INTERNAL_ERROR, "internal error".to_owned()),
            };
            let _ = tx.send(Message::Response(resp));
        });
    }

    /// Dispatch `textDocument/rename` (cross-file edit → a `WorkspaceEdit`, or a refusal error).
    fn handle_rename(&self, req: Request) {
        let id = req.id.clone();
        let params: RenameParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return self.send(Response::new_err(id, INVALID_PARAMS, e.to_string())),
        };
        let pos = params.text_document_position;
        let Some(ctx) = self.doc_ctx(&pos.text_document.uri) else {
            return self.respond_null(id);
        };
        let offset = ctx.line_index.offset(&ctx.text, pos.position, ctx.encoding);
        let (nav, file, new_name) = (self.nav_ctx(), ctx.file, params.new_name);
        self.spawn_fallible(id, move |a| {
            handlers::rename(a, &nav, file, offset, &new_name)
        });
    }

    /// Dispatch `textDocument/codeAction` (the quick-fixes at the selection start).
    fn handle_code_actions(&self, req: Request) {
        let id = req.id.clone();
        let params: CodeActionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return self.send(Response::new_err(id, INVALID_PARAMS, e.to_string())),
        };
        let Some(ctx) = self.doc_ctx(&params.text_document.uri) else {
            return self.respond_null(id);
        };
        let offset = ctx
            .line_index
            .offset(&ctx.text, params.range.start, ctx.encoding);
        let (nav, file) = (self.nav_ctx(), ctx.file);
        self.spawn(id, move |a| handlers::code_actions(a, &nav, file, offset));
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
                self.maybe_update_project_config(&td.uri, &td.text);
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
                self.maybe_update_project_config(&uri, &new_text);
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
                    // Revert the host to the file's on-disk text (it stays part of the project);
                    // `commit` removes it entirely if it had no disk layer (an ad-hoc open).
                    self.commit(id);
                    clear_diagnostics(conn, p.text_document.uri)?;
                }
            }
            // The client finished initializing → scan the workspace into the host (once) + register
            // file watchers so external `.gd`/`.tscn`/`project.godot` edits keep it in sync.
            "initialized" => {
                self.load_project();
                if self.project_root.is_some() {
                    register_file_watchers(conn)?;
                }
            }
            "workspace/didChangeWatchedFiles" => self.on_watched_files_changed(conn, note)?,
            // `exit` is consumed by the main loop before reaching here.
            _ => {}
        }
        Ok(())
    }

    /// Apply external file changes (a `workspace/didChangeWatchedFiles` notification) to the host: a
    /// changed/created `.gd`/`.tscn` is re-read from disk into its background layer; a deleted one is
    /// dropped. An **open** document is left alone (its editor overlay wins). Open documents then get
    /// fresh diagnostics, since a background change can shift their cross-file resolution.
    fn on_watched_files_changed(&mut self, conn: &Connection, note: &Notification) -> Result<()> {
        let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(note.params.clone())
        else {
            return Ok(());
        };
        for ev in p.changes {
            // A project.godot change re-feeds the config (handled regardless of overlay state).
            if let Some(path) = project::uri_to_path(&ev.uri)
                && path.file_name().is_some_and(|n| n == "project.godot")
            {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let mut change = Change::new();
                    change.set_project_config(text);
                    self.host.apply_change(change);
                }
                continue;
            }
            let id = self.vfs.intern(&ev.uri);
            if self.vfs.doc(id).is_some() {
                continue; // an open document's overlay wins — don't disturb it from disk
            }
            if ev.typ == FileChangeType::DELETED {
                self.vfs.remove_disk(id);
                let mut change = Change::new();
                change.remove_file(id);
                self.host.apply_change(change);
                self.with_path.remove(&id);
                continue;
            }
            // Created / Changed: re-read the file into its background layer.
            let Some(path) = project::uri_to_path(&ev.uri) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let text: Arc<str> = Arc::from(String::from_utf8_lossy(&bytes).into_owned());
            self.vfs.set_disk(&ev.uri, text);
            if let Some(root) = &self.project_root
                && let Some(res) = project::res_path_for(root, &path)
            {
                self.vfs.set_res_path(id, res);
            }
            self.commit(id);
        }
        // Refresh open documents — their cross-file resolution may have shifted under the change.
        for id in self.vfs.open_ids() {
            self.publish_diagnostics(conn, id)?;
        }
        Ok(())
    }

    /// When `uri` is the project's `project.godot`, feed its text as the project config so an in-editor
    /// edit of autoloads/version takes effect without a restart.
    fn maybe_update_project_config(&mut self, uri: &Uri, text: &str) {
        if uri.as_str().rsplit('/').next() == Some("project.godot") {
            let mut change = Change::new();
            change.set_project_config(text.to_owned());
            self.host.apply_change(change);
        }
    }

    /// Feed a file's *effective* text (overlay if open, else its on-disk layer) to the salsa input,
    /// recording its `res://` path on first commit. A file with neither layer is removed from the host.
    fn commit(&mut self, id: FileId) {
        let Some(snap) = self.vfs.snapshot(id) else {
            let mut change = Change::new();
            change.remove_file(id);
            self.host.apply_change(change);
            self.with_path.remove(&id);
            return;
        };
        let mut change = Change::new();
        change.change_file(id, snap.text);
        if let Some(res) = self.vfs.res_path(id).map(str::to_owned)
            && self.with_path.insert(id)
        {
            change.set_file_path(id, res);
        }
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

/// Dynamically register file watchers so the client sends `workspace/didChangeWatchedFiles` for the
/// project's `.gd` / `.tscn` / `project.godot` files. This is a server→client *request*
/// (`client/registerCapability`); the client's reply is ignored (the main loop drops a stray
/// `Response`). Requires the client to support `workspace.didChangeWatchedFiles.dynamicRegistration`
/// (VS Code, Neovim, … do); a client that doesn't simply never sends the notification.
fn register_file_watchers(conn: &Connection) -> Result<()> {
    let watchers = ["**/*.gd", "**/*.tscn", "**/project.godot"]
        .into_iter()
        .map(|g| FileSystemWatcher {
            glob_pattern: GlobPattern::String(g.to_owned()),
            kind: None, // default: Create | Change | Delete
        })
        .collect();
    let options = DidChangeWatchedFilesRegistrationOptions { watchers };
    let params = RegistrationParams {
        registrations: vec![Registration {
            id: "gdscript-watch-files".to_owned(),
            method: "workspace/didChangeWatchedFiles".to_owned(),
            register_options: Some(serde_json::to_value(options)?),
        }],
    };
    let req = Request::new(
        RequestId::from("gdscript-register-watchers".to_owned()),
        "client/registerCapability".to_owned(),
        serde_json::to_value(params)?,
    );
    conn.sender.send(Message::Request(req))?;
    Ok(())
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
        GlobalState::new(
            PositionEncoding::Utf16,
            crossbeam_channel::unbounded().0,
            Vec::new(),
        )
    }

    /// A unique scratch directory that wipes itself on drop (no `tempfile` dep).
    struct TempProject(std::path::PathBuf);
    impl TempProject {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "gdlsp_{tag}_{}_{:p}",
                std::process::id(),
                &raw const tag
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.0.join(name), contents).unwrap();
        }
        fn uri(&self) -> Uri {
            project::path_to_uri(&self.0.canonicalize().unwrap()).unwrap()
        }
    }
    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn whole_project_loads_and_resolves_cross_file_without_collision() {
        // The headline Phase-5 fix: a scanned-but-unopened library file lets an opened file resolve a
        // cross-file `class_name`, AND opening that scanned file does NOT double-load it into a false
        // `class_name`-collision (the FileId is reused via the canonical-path interner).
        let proj = TempProject::new("xfile");
        proj.write("project.godot", "[application]\nconfig/name=\"t\"\n");
        proj.write(
            "lib.gd",
            "class_name Lib\nstatic func ping() -> int:\n\treturn 1\n",
        );
        proj.write("main.gd", "func go():\n\tvar n := Lib.ping()\n\treturn n\n");

        let mut state = test_state();
        state.roots = vec![proj.uri()];
        state.load_project();

        // Locate the scanned files by URI suffix.
        let find = |state: &GlobalState, suffix: &str| {
            state
                .vfs
                .known_ids()
                .into_iter()
                .find(|&id| state.vfs.uri(id).unwrap().as_str().ends_with(suffix))
                .unwrap()
        };
        let lib = find(&state, "lib.gd");
        let main = find(&state, "main.gd");

        // A seam (unresolved cross-file `Lib`) would manufacture these; their absence proves resolution.
        let unresolved = |d: &gdscript_base::Diagnostic| {
            d.code == "INFERENCE_ON_VARIANT" || d.code.starts_with("UNSAFE")
        };
        // Snapshot in a block so it is dropped before the host is next mutated (salsa's single writer
        // cancels outstanding snapshots and blocks until they release — a held snapshot would deadlock).
        {
            let a = state.host.analysis();
            // `Lib` resolves cross-file from main.gd → no inference-on-variant / unsafe-access warning.
            let main_diags = a.diagnostics(main).unwrap_or_default();
            assert!(
                !main_diags.iter().any(unresolved),
                "main.gd should resolve `Lib.ping()` cross-file (no seam warning), got {main_diags:?}",
            );
            // lib.gd defines `Lib` exactly once → no SHADOWED_GLOBAL_IDENTIFIER (the double-load symptom).
            let lib_diags = a.diagnostics(lib).unwrap_or_default();
            assert!(
                !lib_diags
                    .iter()
                    .any(|d| d.code == "SHADOWED_GLOBAL_IDENTIFIER"),
                "the scanned class_name must not collide with itself: {lib_diags:?}",
            );
        }

        // Opening main.gd (an overlay over its disk layer) must keep the same FileId + resolution.
        let main_uri = state.vfs.uri(main).unwrap().clone();
        let reopened = state.vfs.upsert(
            &main_uri,
            "func go():\n\tvar n := Lib.ping()\n".to_owned(),
            1,
        );
        assert_eq!(reopened, main, "didOpen must reuse the scanned FileId");
        state.commit(reopened);
        assert!(
            !state
                .host
                .analysis()
                .diagnostics(main)
                .unwrap_or_default()
                .iter()
                .any(unresolved),
            "cross-file resolution must survive opening the file as an overlay",
        );
    }

    #[test]
    fn watched_file_creation_lights_up_cross_file_resolution() {
        // didChangeWatchedFiles completes whole-project loading: a `.gd` created on disk *after* the
        // initial scan is ingested, so an existing file's cross-file reference now resolves.
        let proj = TempProject::new("watch");
        proj.write("project.godot", "[application]\nconfig/name=\"t\"\n");
        proj.write("main.gd", "func go():\n\tvar n := Lib.ping()\n\treturn n\n");
        // lib.gd does NOT exist at scan time → `Lib` is the seam.

        let (server, _client) = Connection::memory();
        let mut state = test_state();
        state.roots = vec![proj.uri()];
        state.load_project();
        let main = state
            .vfs
            .known_ids()
            .into_iter()
            .find(|&id| state.vfs.uri(id).unwrap().as_str().ends_with("main.gd"))
            .unwrap();

        // Create lib.gd on disk, then deliver the watcher event.
        proj.write(
            "lib.gd",
            "class_name Lib\nstatic func ping() -> int:\n\treturn 1\n",
        );
        let lib_path = proj.0.join("lib.gd").canonicalize().unwrap();
        let params = DidChangeWatchedFilesParams {
            changes: vec![lsp_types::FileEvent {
                uri: project::path_to_uri(&lib_path).unwrap(),
                typ: FileChangeType::CREATED,
            }],
        };
        let note = Notification::new(
            "workspace/didChangeWatchedFiles".to_owned(),
            serde_json::to_value(params).unwrap(),
        );
        state.on_watched_files_changed(&server, &note).unwrap();

        // main.gd now sees the newly-created `Lib` → `n` is typed int (an `: int` inlay hint appears).
        let hints = state.host.analysis().inlay_hints(main).unwrap_or_default();
        let json = serde_json::to_string(&hints).unwrap();
        assert!(
            json.contains("int"),
            "creating lib.gd should let main.gd resolve Lib.ping() and type `n` as int: {json}",
        );
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
    fn inlay_hints_over_the_public_api() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        let init: InitializeResult = serde_json::from_value(init.result.unwrap()).unwrap();
        assert!(
            init.capabilities.inlay_hint_provider.is_some(),
            "inlayHint advertised"
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
                    text: "func f():\n\tvar x := 1\n".to_owned(), // `x` gets a `: int` hint
                },
            },
        );
        let _ = next_diagnostics(&client);

        send_req(
            &client,
            2,
            "textDocument/inlayHint",
            serde_json::json!({
                "textDocument": { "uri": doc_uri.as_str() },
                "range": { "start": {"line": 0, "character": 0}, "end": {"line": 5, "character": 0} },
            }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "inlayHint errored: {resp:?}");
        let hints: Vec<lsp_types::InlayHint> =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(
            hints.iter().any(
                |h| matches!(&h.label, lsp_types::InlayHintLabel::String(s) if s.contains("int"))
            ),
            "expected a `: int` inlay hint: {hints:?}",
        );

        send_req(&client, 9, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn semantic_tokens_over_the_public_api() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        let init: InitializeResult = serde_json::from_value(init.result.unwrap()).unwrap();
        assert!(
            init.capabilities.semantic_tokens_provider.is_some(),
            "semanticTokens advertised"
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
                    text: "func greet() -> int:\n\treturn 1\n".to_owned(),
                },
            },
        );
        let _ = next_diagnostics(&client);

        send_req(
            &client,
            2,
            "textDocument/semanticTokens/full",
            serde_json::json!({ "textDocument": { "uri": doc_uri.as_str() } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "semanticTokens errored: {resp:?}");
        let result: lsp_types::SemanticTokensResult =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        let lsp_types::SemanticTokensResult::Tokens(t) = result else {
            panic!("expected full semantic tokens");
        };
        // `func`/`greet`/`int`/… → at least the 5-int records for greet + int.
        assert!(
            t.data.len() >= 2,
            "expected encoded tokens, got {:?}",
            t.data
        );

        send_req(&client, 9, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn goto_definition_and_references_over_the_public_api() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        let init: InitializeResult = serde_json::from_value(init.result.unwrap()).unwrap();
        assert!(
            init.capabilities.definition_provider.is_some(),
            "definition advertised"
        );
        assert!(
            init.capabilities.references_provider.is_some(),
            "references advertised"
        );
        send_note(&client, "initialized", InitializedParams {});

        // `total` is declared on line 1 and used on line 2.
        let doc_uri = uri("file:///main.gd");
        let gd = "func f():\n\tvar total := 1\n\treturn total\n";
        send_note(
            &client,
            "textDocument/didOpen",
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: doc_uri.clone(),
                    language_id: "gdscript".to_owned(),
                    version: 1,
                    text: gd.to_owned(),
                },
            },
        );
        let _ = next_diagnostics(&client);
        let u = doc_uri.as_str();

        // goto-definition from the use of `total` (line 2) → its declaration (line 1).
        let use_col = "\treturn ".len(); // column of `total` on line 2
        send_req(
            &client,
            2,
            "textDocument/definition",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 2, "character": use_col } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "definition errored: {resp:?}");
        let def: lsp_types::GotoDefinitionResponse =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        let lsp_types::GotoDefinitionResponse::Array(locs) = def else {
            panic!("expected an array of locations");
        };
        assert!(
            locs.iter()
                .any(|l| l.uri == doc_uri && l.range.start.line == 1),
            "definition should point at line 1: {locs:?}",
        );

        // find-references on `total` → at least the declaration + the use.
        send_req(
            &client,
            3,
            "textDocument/references",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 2, "character": use_col }, "context": { "includeDeclaration": true } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "references errored: {resp:?}");
        let refs: Vec<lsp_types::Location> = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(refs.len() >= 2, "expected decl + use references: {refs:?}");

        send_req(&client, 9, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    fn prepare_rename_validates_the_cursor() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let _ = next_response(&client);
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
                    text: "func f():\n\tvar total := 1\n\treturn total\n".to_owned(),
                },
            },
        );
        let _ = next_diagnostics(&client);
        let u = doc_uri.as_str();

        // inside `total` (line 1, char 7) → a renameable range.
        send_req(
            &client,
            2,
            "textDocument/prepareRename",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 1, "character": 7 } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "{resp:?}");
        let inside: Option<lsp_types::PrepareRenameResponse> =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(
            inside.is_some(),
            "cursor inside `total` should be renameable"
        );

        // on the leading tab (line 1, char 0) → null (not a symbol).
        send_req(
            &client,
            3,
            "textDocument/prepareRename",
            serde_json::json!({ "textDocument": { "uri": u }, "position": { "line": 1, "character": 0 } }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "{resp:?}");
        assert_eq!(
            resp.result,
            Some(serde_json::Value::Null),
            "whitespace is not renameable"
        );

        send_req(&client, 9, "shutdown", ());
        let _ = next_response(&client);
        send_note(&client, "exit", ());
        server_thread.join().unwrap().unwrap();
    }

    #[test]
    #[allow(
        clippy::mutable_key_type,
        reason = "lsp_types::Uri key — interior cache is hash-stable"
    )]
    fn rename_over_the_public_api() {
        let (server, client) = Connection::memory();
        let server_thread = std::thread::spawn(move || run(&server));
        send_req(&client, 1, "initialize", InitializeParams::default());
        let init = next_response(&client);
        let init: InitializeResult = serde_json::from_value(init.result.unwrap()).unwrap();
        assert!(
            init.capabilities.rename_provider.is_some(),
            "rename advertised"
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
                    text: "func f():\n\tvar total := 1\n\treturn total\n".to_owned(),
                },
            },
        );
        let _ = next_diagnostics(&client);

        // rename `total` (declared at line 1, col 5) → `sum`.
        send_req(
            &client,
            2,
            "textDocument/rename",
            serde_json::json!({
                "textDocument": { "uri": doc_uri.as_str() },
                "position": { "line": 1, "character": 5 },
                "newName": "sum",
            }),
        );
        let resp = next_response(&client);
        assert!(resp.error.is_none(), "rename errored: {resp:?}");
        let edit: lsp_types::WorkspaceEdit = serde_json::from_value(resp.result.unwrap()).unwrap();
        let changes = edit.changes.expect("changes map");
        let edits = changes.get(&doc_uri).expect("edits for the doc");
        assert!(edits.len() >= 2, "decl + use both renamed: {edits:?}");
        assert!(edits.iter().all(|e| e.new_text == "sum"), "{edits:?}");

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
