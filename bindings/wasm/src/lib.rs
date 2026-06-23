//! `gdscript-wasm` — the browser binding (wasm-bindgen).
//!
//! The Phase-1 browser path (Playbook §2): a single-threaded `wasm32-unknown-unknown`
//! build, packaged with `wasm-pack build --target web`, that loads from a static page
//! via `<script type="module">` with **no** server-side WASI, `SharedArrayBuffer`, or
//! COOP/COEP requirement. (napi-rs's wasm target is `wasm32-wasip1-threads` and needs a
//! runtime polyfill + cross-origin isolation, so it is not usable here — hence this
//! separate crate.)
//!
//! A stateful [`WasmAnalysis`] keeps a `gdscript-ide` `AnalysisHost` alive across edits.
//! Each query returns a **JSON string** of the engine-neutral `gdscript-base` POD
//! results; the page `JSON.parse`s it and converts byte offsets to UTF-16.
//!
//! Build: `wasm-pack build --target web --out-dir ../../playground/pkg --out-name gdscript bindings/wasm`
#![cfg_attr(docsrs, feature(doc_cfg))]
// wasm-bindgen's `#[wasm_bindgen]` expands to `unsafe extern` glue; the binding handle
// is an opaque JS object that needs no `Debug`.
#![allow(unsafe_code, missing_debug_implementations)]

use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{AnalysisHost, Change};
use wasm_bindgen::prelude::wasm_bindgen;

/// Install a panic hook that routes Rust panics to the browser console (dev aid).
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// A live analysis session in the browser. Construct once, push files with
/// `applyChange`, then query; results are JSON strings of `gdscript-base` POD types.
#[wasm_bindgen]
pub struct WasmAnalysis {
    host: AnalysisHost,
}

#[wasm_bindgen]
impl WasmAnalysis {
    /// Create an empty analysis session.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            host: AnalysisHost::new(),
        }
    }

    /// Add/replace (`Some`) or remove (`None`) a file by id.
    #[wasm_bindgen(js_name = applyChange)]
    pub fn apply_change(&mut self, file_id: u32, text: Option<String>) {
        let mut change = Change::new();
        match text {
            Some(t) => change.change_file(FileId(file_id), t),
            None => change.remove_file(FileId(file_id)),
        }
        self.host.apply_change(change);
    }

    /// Parse-error diagnostics, as a JSON array string.
    #[must_use]
    pub fn diagnostics(&self, file_id: u32) -> String {
        json(
            &self
                .host
                .analysis()
                .diagnostics(FileId(file_id))
                .unwrap_or_default(),
        )
    }

    /// The document outline, as a JSON array string.
    #[wasm_bindgen(js_name = documentSymbols)]
    #[must_use]
    pub fn document_symbols(&self, file_id: u32) -> String {
        json(
            &self
                .host
                .analysis()
                .document_symbols(FileId(file_id))
                .unwrap_or_default(),
        )
    }

    /// Folding ranges, as a JSON array string.
    #[wasm_bindgen(js_name = foldingRanges)]
    #[must_use]
    pub fn folding_ranges(&self, file_id: u32) -> String {
        json(
            &self
                .host
                .analysis()
                .folding_ranges(FileId(file_id))
                .unwrap_or_default(),
        )
    }

    /// By-name completions at a byte `offset`, as a JSON array string.
    #[must_use]
    pub fn completions(&self, file_id: u32, offset: u32) -> String {
        json(
            &self
                .host
                .analysis()
                .completions(FilePosition {
                    file: FileId(file_id),
                    offset,
                })
                .unwrap_or_default(),
        )
    }
}

impl Default for WasmAnalysis {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize a POD result to a JSON string (empty array on the impossible error).
fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".to_owned())
}
