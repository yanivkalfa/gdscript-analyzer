//! `gdscript-ffi` — the napi-rs v3 Node binding (ADR-0003), a **thin wrapper** over
//! [`gdscript_session::Session`].
//!
//! All real logic — the URI→`FileId` interner, the document lifecycle, the JSON serialization of
//! query results — lives in the pure-Rust, fully unit-tested `gdscript-session` core. This crate is
//! a near-trivial `#[napi]` delegator, because a napi `cdylib` cannot be `cargo test`ed natively
//! (no Node runtime / `libnode` at link time), so the testable logic must live elsewhere
//! (`gdscript-session` is in `default-members`; `xtask ci` tests it). Mirrors the wasm binding
//! (`gdscript-wasm`), which wraps the same `Session`.
//!
//! The JS side holds one [`AnalysisHandle`], pushes documents by **URI string**, and queries by
//! URI + **byte offset**; every query returns a **JSON string** of the engine-neutral
//! `gdscript-base` POD results (the client `JSON.parse`s it and maps byte offsets to its own
//! position encoding — UTF-16 in a JS editor). `null`-returning queries (`hover`, `signatureHelp`,
//! `syntaxTree`) map a Rust `None` to JS `null`.
//!
//! This crate is the Node path only (native + `wasm32-wasip1-threads`); the browser binding is the
//! separate `bindings/wasm` crate. Build: `napi build --platform --release`.
#![cfg_attr(docsrs, feature(doc_cfg))]
// napi-derive expands to `unsafe extern "C"` glue; that is the crate's only `unsafe`. The binding
// handle is an opaque JS object that needs no `Debug`.
#![allow(unsafe_code, missing_debug_implementations)]

use gdscript_session::Session;
use napi_derive::napi;

/// A live, URI-keyed analysis session. Construct once with `new AnalysisHandle()`, push documents
/// with `openDocument`/`changeDocument`/`closeDocument` (+ `setProjectConfig`), then query. The Rust
/// `AnalysisHost` (and its salsa cache) stays alive across edits. napi objects are owned by the
/// single JS thread, so the held (non-`Sync`) salsa state is never shared across threads.
#[napi]
pub struct AnalysisHandle {
    session: Session,
}

#[napi]
impl AnalysisHandle {
    /// Create an empty analysis session.
    #[napi(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: Session::new(),
        }
    }

    // ---- document lifecycle ----

    /// Open or replace a document by `uri`. Pass `resPath` (`res://…`) on first open to enable
    /// cross-file `preload` / `extends` / autoload resolution; it is recorded once and ignored on
    /// later opens (re-sending would needlessly invalidate the resource-path registry).
    #[napi]
    pub fn open_document(&mut self, uri: String, text: String, res_path: Option<String>) {
        self.session.open(&uri, &text, res_path.as_deref());
    }

    /// Replace a document's text by `uri` (its `res://` path is unchanged). An unknown `uri` is
    /// interned as a new document (an upsert).
    #[napi]
    pub fn change_document(&mut self, uri: String, text: String) {
        self.session.change(&uri, &text);
    }

    /// Close (remove) a document by `uri`. A later re-open assigns a fresh file id.
    #[napi]
    pub fn close_document(&mut self, uri: String) {
        self.session.close(&uri);
    }

    /// Set the project's `project.godot` text (enables `[autoload]` singleton resolution).
    #[napi]
    pub fn set_project_config(&mut self, text: String) {
        self.session.set_project_config(&text);
    }

    // ---- queries (JSON strings of `gdscript-base` POD) ----

    /// Parse + type diagnostics for `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn diagnostics(&self, uri: String) -> String {
        self.session.diagnostics(&uri)
    }

    /// The document outline for `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn document_symbols(&self, uri: String) -> String {
        self.session.document_symbols(&uri)
    }

    /// Foldable ranges for `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn folding_ranges(&self, uri: String) -> String {
        self.session.folding_ranges(&uri)
    }

    /// Inlay hints for `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn inlay_hints(&self, uri: String) -> String {
        self.session.inlay_hints(&uri)
    }

    /// Completions at a byte `offset` in `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn completions(&self, uri: String, offset: u32) -> String {
        self.session.completions(&uri, offset)
    }

    /// Hover at a byte `offset` in `uri`; JS `null` when there is nothing typed there.
    #[napi]
    #[must_use]
    pub fn hover(&self, uri: String, offset: u32) -> Option<String> {
        self.session.hover(&uri, offset)
    }

    /// Signature help at a byte `offset` in `uri`; JS `null` when not at a call site.
    #[napi]
    #[must_use]
    pub fn signature_help(&self, uri: String, offset: u32) -> Option<String> {
        self.session.signature_help(&uri, offset)
    }

    /// Code actions at a byte `offset` in `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn code_actions(&self, uri: String, offset: u32) -> String {
        self.session.code_actions(&uri, offset)
    }

    /// Go-to-definition target(s) for the symbol at a byte `offset` in `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn goto_definition(&self, uri: String, offset: u32) -> String {
        self.session.goto_definition(&uri, offset)
    }

    /// Every reference to the symbol at a byte `offset` in `uri`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn find_references(&self, uri: String, offset: u32) -> String {
        self.session.find_references(&uri, offset)
    }

    /// Rename the symbol at a byte `offset` in `uri` to `newName`. JSON object string:
    /// `{"ok": <SourceChange>}` or `{"error": <RenameError>}`.
    #[napi]
    #[must_use]
    pub fn rename(&self, uri: String, offset: u32, new_name: String) -> String {
        self.session.rename(&uri, offset, &new_name)
    }

    /// Project-wide symbols matching `query`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn workspace_symbols(&self, query: String) -> String {
        self.session.workspace_symbols(&query)
    }

    /// The pretty-printed syntax tree for `uri` (debugging); JS `null` for an unknown `uri`.
    #[napi]
    #[must_use]
    pub fn syntax_tree(&self, uri: String) -> Option<String> {
        self.session.syntax_tree(&uri)
    }
}

impl Default for AnalysisHandle {
    fn default() -> Self {
        Self::new()
    }
}
