//! `gdscript-wasm` — the browser binding (wasm-bindgen), a **thin wrapper** over
//! [`gdscript_session::Session`] (mirrors the napi binding `gdscript-ffi`).
//!
//! The Phase-1 browser path (Playbook §2 / §WS4): a single-threaded `wasm32-unknown-unknown` build,
//! packaged with `wasm-pack build --target web`, loaded from a static page via
//! `<script type="module">` — **no** server-side WASI, `SharedArrayBuffer`, or COOP/COEP. (napi-rs's
//! wasm target is `wasm32-wasip1-threads` and needs cross-origin isolation, so it is not usable for
//! a static page — hence this separate crate.)
//!
//! All real logic lives in the pure-Rust, fully unit-tested `gdscript-session`; this crate just
//! exposes it to JS. The page holds one [`Analyzer`], pushes documents by **URI string**, and
//! queries by URI + **byte offset**; every query returns a **native JS object** — the session's
//! [`serde_json::Value`] converted via `serde_wasm_bindgen` (json-compatible, so objects are plain
//! objects, not JS `Map`s) — so the page does **not** `JSON.parse`. Navigation/edit results carry a
//! `uri` per file (mirror-free). The page still maps byte offsets to UTF-16 (§4.3).
//!
//! **Engine model:** the `extension_api` blob is **not** embedded on `wasm32` (Playbook §4.4); the
//! page `fetch`es it and installs it via [`Analyzer::load_engine_api`] before its first query, so
//! completion/hover for engine classes (`Button`/`Control`/…) light up.
//!
//! Build: `wasm-pack build --target web --out-dir ../../playground/pkg --out-name gdscript bindings/wasm`
#![cfg_attr(docsrs, feature(doc_cfg))]
// wasm-bindgen's `#[wasm_bindgen]` expands to `unsafe extern` glue; the binding handle is an opaque
// JS object that needs no `Debug`.
#![allow(unsafe_code, missing_debug_implementations)]
// Exported binding methods take JS-mapped owned values (`String`/`Option<String>`) by the FFI ABI;
// the thin delegation only borrows them, so clippy's by-value lint is a false positive here.
#![allow(clippy::needless_pass_by_value)]

use gdscript_session::Session;
use serde::Serialize;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

/// Install a panic hook that routes Rust panics to the browser console (dev aid).
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// Convert a serializable value (the session's `serde_json::Value` results) into a native JS value.
/// Uses `Serializer::json_compatible()` so a `serde_json::Value::Object` becomes a plain JS object
/// (the default serializer would emit a JS `Map`, breaking `result.field` access). On the
/// practically-impossible conversion failure, yields `null`.
fn to_js<T: Serialize>(value: &T) -> JsValue {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .unwrap_or(JsValue::NULL)
}

/// A live, URI-keyed analysis session in the browser. Construct once with `new Analyzer()`, push
/// documents with `openDocument`/`changeDocument`/`closeDocument` (+ `setProjectConfig`,
/// `loadEngineApi`), then query. The Rust `AnalysisHost` (and its salsa cache) stays alive across
/// edits. Single-threaded — wasm has one thread, so the held salsa state is never shared.
#[wasm_bindgen]
pub struct Analyzer {
    session: Session,
}

#[wasm_bindgen]
impl Analyzer {
    /// Create an empty analysis session.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: Session::new(),
        }
    }

    /// Install a `fetch`ed `extension_api` blob (the engine model — not embedded on wasm). Returns
    /// `false` if the bytes fail to decode. Call once, before querying, so engine-class
    /// completion/hover work.
    #[wasm_bindgen(js_name = loadEngineApi)]
    pub fn load_engine_api(&mut self, bytes: &[u8]) -> bool {
        self.session.load_engine_api(bytes)
    }

    // ---- document lifecycle ----

    /// Open or replace a document by `uri`. Pass `resPath` (`res://…`) on first open to enable
    /// cross-file `preload` / `extends` / autoload resolution.
    #[wasm_bindgen(js_name = openDocument)]
    pub fn open_document(&mut self, uri: String, text: String, res_path: Option<String>) {
        self.session.open(&uri, &text, res_path.as_deref());
    }

    /// Replace a document's text by `uri` (its `res://` path is unchanged).
    #[wasm_bindgen(js_name = changeDocument)]
    pub fn change_document(&mut self, uri: String, text: String) {
        self.session.change(&uri, &text);
    }

    /// Close (remove) a document by `uri`.
    #[wasm_bindgen(js_name = closeDocument)]
    pub fn close_document(&mut self, uri: String) {
        self.session.close(&uri);
    }

    /// Set the project's `project.godot` text (enables `[autoload]` singleton resolution).
    #[wasm_bindgen(js_name = setProjectConfig)]
    pub fn set_project_config(&mut self, text: String) {
        self.session.set_project_config(&text);
    }

    /// Declare whether the opened file set is the **whole project** (every `.gd` under the
    /// project root). Pass `true` only after feeding every project file — it arms the
    /// absence-based `UNDEFINED_FUNCTION` / `UNDEFINED_IDENTIFIER` diagnostics (silent otherwise).
    #[wasm_bindgen(js_name = setWorkspaceComplete)]
    pub fn set_workspace_complete(&mut self, complete: bool) {
        self.session.set_workspace_complete(complete);
    }

    /// Whether `uri` is currently open (distinguishes "not tracked" from a genuine empty result).
    #[wasm_bindgen(js_name = isOpen)]
    #[must_use]
    pub fn is_open(&self, uri: String) -> bool {
        self.session.is_open(&uri)
    }

    // ---- queries (native JS objects: `serde_json::Value` of `gdscript-base` POD) ----

    /// Parse + type diagnostics for `uri`, as a JS array.
    #[must_use]
    pub fn diagnostics(&self, uri: String) -> JsValue {
        to_js(&self.session.diagnostics(&uri))
    }

    /// The document outline for `uri`, as a JS array.
    #[wasm_bindgen(js_name = documentSymbols)]
    #[must_use]
    pub fn document_symbols(&self, uri: String) -> JsValue {
        to_js(&self.session.document_symbols(&uri))
    }

    /// Foldable ranges for `uri`, as a JS array.
    #[wasm_bindgen(js_name = foldingRanges)]
    #[must_use]
    pub fn folding_ranges(&self, uri: String) -> JsValue {
        to_js(&self.session.folding_ranges(&uri))
    }

    /// Inlay hints for `uri`, as a JS array.
    #[wasm_bindgen(js_name = inlayHints)]
    #[must_use]
    pub fn inlay_hints(&self, uri: String) -> JsValue {
        to_js(&self.session.inlay_hints(&uri))
    }

    /// Completions at a byte `offset` in `uri`, as a JS array.
    #[must_use]
    pub fn completions(&self, uri: String, offset: u32) -> JsValue {
        to_js(&self.session.completions(&uri, offset))
    }

    /// Hover at a byte `offset` in `uri`; `null` when there is nothing typed there.
    #[must_use]
    pub fn hover(&self, uri: String, offset: u32) -> JsValue {
        match self.session.hover(&uri, offset) {
            Some(v) => to_js(&v),
            None => JsValue::NULL,
        }
    }

    /// Signature help at a byte `offset` in `uri`; `null` when not at a call site.
    #[wasm_bindgen(js_name = signatureHelp)]
    #[must_use]
    pub fn signature_help(&self, uri: String, offset: u32) -> JsValue {
        match self.session.signature_help(&uri, offset) {
            Some(v) => to_js(&v),
            None => JsValue::NULL,
        }
    }

    /// Code actions at a byte `offset` in `uri`, as a JS array.
    #[wasm_bindgen(js_name = codeActions)]
    #[must_use]
    pub fn code_actions(&self, uri: String, offset: u32) -> JsValue {
        to_js(&self.session.code_actions(&uri, offset))
    }

    /// Go-to-definition target(s) for the symbol at a byte `offset` in `uri`, as a JS array (each
    /// target carries a `uri`).
    #[wasm_bindgen(js_name = gotoDefinition)]
    #[must_use]
    pub fn goto_definition(&self, uri: String, offset: u32) -> JsValue {
        to_js(&self.session.goto_definition(&uri, offset))
    }

    /// Every reference to the symbol at a byte `offset` in `uri`, as a JS array (each carries a `uri`).
    #[wasm_bindgen(js_name = findReferences)]
    #[must_use]
    pub fn find_references(&self, uri: String, offset: u32) -> JsValue {
        to_js(&self.session.find_references(&uri, offset))
    }

    /// Rename the symbol at a byte `offset` in `uri` to `newName`. A JS object:
    /// `{ ok: <SourceChange> }` | `{ error: <RenameError | reason> }` (edits carry a `uri`).
    #[must_use]
    pub fn rename(&self, uri: String, offset: u32, new_name: String) -> JsValue {
        to_js(&self.session.rename(&uri, offset, &new_name))
    }

    /// Project-wide symbols matching `query`, as a JS array.
    #[wasm_bindgen(js_name = workspaceSymbols)]
    #[must_use]
    pub fn workspace_symbols(&self, query: String) -> JsValue {
        to_js(&self.session.workspace_symbols(&query))
    }

    /// The pretty-printed syntax tree for `uri` (debugging); `undefined` for an unknown `uri`.
    #[wasm_bindgen(js_name = syntaxTree)]
    #[must_use]
    pub fn syntax_tree(&self, uri: String) -> Option<String> {
        self.session.syntax_tree(&uri)
    }

    /// Semantic-highlighting tokens for `uri`, as a JS array.
    #[wasm_bindgen(js_name = semanticTokens)]
    #[must_use]
    pub fn semantic_tokens(&self, uri: String) -> JsValue {
        to_js(&self.session.semantic_tokens(&uri))
    }

    /// Format `uri`'s whole document; the tidied text, or `null` for an unknown `uri`.
    #[must_use]
    pub fn format(&self, uri: String) -> Option<String> {
        self.session.format(&uri)
    }

    /// Format only the lines overlapping the byte range `[start, end)`; a `{ range, new_text }` object,
    /// or `null` when the selection's lines do not change / `uri` is unknown.
    #[wasm_bindgen(js_name = formatRange)]
    #[must_use]
    pub fn format_range(&self, uri: String, start: u32, end: u32) -> JsValue {
        match self.session.format_range(&uri, start, end) {
            Some(v) => to_js(&v),
            None => JsValue::NULL,
        }
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}
