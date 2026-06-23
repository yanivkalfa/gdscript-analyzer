//! `gdscript-ffi` — the napi-rs v3 Node binding (ADR-0003).
//!
//! A stateful [`AnalysisHandle`] keeps a `gdscript-ide` `AnalysisHost` alive across
//! edits (so the analysis state survives between calls). The JS side calls
//! `applyChange` then the Tier-0 queries, each of which returns a **JSON string** of
//! the engine-neutral `gdscript-base` POD results (the serde value-object shape from
//! `plans/research/06-analyzer-architecture.md` §6). The client `JSON.parse`s it and
//! converts byte offsets to its own position encoding.
//!
//! This crate is the Node path only (native + `wasm32-wasip1-threads`). The **browser**
//! binding is the separate `bindings/wasm` crate (wasm-bindgen + `wasm-pack --target
//! web`) — napi-rs's wasm target is not a drop-in for a static page (Playbook §2).
//!
//! Build: `napi build --platform --release` (see `bindings/node/`).
#![cfg_attr(docsrs, feature(doc_cfg))]
// napi-derive expands to `unsafe extern "C"` glue; that is the crate's only `unsafe`.
// The binding handle is an opaque JS object that needs no `Debug`.
#![allow(unsafe_code, missing_debug_implementations)]

use gdscript_base::FileId;
use gdscript_ide::{AnalysisHost, Change};
use napi_derive::napi;

/// A live analysis session. Construct once, push files with [`AnalysisHandle::apply_change`],
/// then query. Each query returns a JSON string of `gdscript-base` POD results.
#[napi]
pub struct AnalysisHandle {
    host: AnalysisHost,
}

#[napi]
impl AnalysisHandle {
    /// Create an empty analysis session.
    #[napi(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            host: AnalysisHost::new(),
        }
    }

    /// Add or replace (`text`) or remove (`null`) a file by id. Keeps the session's
    /// state alive across edits.
    #[napi]
    pub fn apply_change(&mut self, file_id: u32, text: Option<String>) {
        let mut change = Change::new();
        match text {
            Some(t) => change.change_file(FileId(file_id), t),
            None => change.remove_file(FileId(file_id)),
        }
        self.host.apply_change(change);
    }

    /// Parse-error diagnostics for a file, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn diagnostics(&self, file_id: u32) -> String {
        let result = self
            .host
            .analysis()
            .diagnostics(FileId(file_id))
            .unwrap_or_default();
        serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_owned())
    }

    /// The document outline for a file, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn document_symbols(&self, file_id: u32) -> String {
        let result = self
            .host
            .analysis()
            .document_symbols(FileId(file_id))
            .unwrap_or_default();
        serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_owned())
    }

    /// Folding ranges for a file, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn folding_ranges(&self, file_id: u32) -> String {
        let result = self
            .host
            .analysis()
            .folding_ranges(FileId(file_id))
            .unwrap_or_default();
        serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_owned())
    }

    /// By-name completions at a byte `offset`, as a JSON array string.
    #[napi]
    #[must_use]
    pub fn completions(&self, file_id: u32, offset: u32) -> String {
        let result = self
            .host
            .analysis()
            .completions(gdscript_base::FilePosition {
                file: FileId(file_id),
                offset,
            })
            .unwrap_or_default();
        serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_owned())
    }
}

impl Default for AnalysisHandle {
    fn default() -> Self {
        Self::new()
    }
}
