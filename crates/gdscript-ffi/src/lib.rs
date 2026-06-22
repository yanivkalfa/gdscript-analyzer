//! `gdscript-ffi` — the napi-rs binding surface (ADR-0003).
//!
//! Phase 1 wires napi-rs v3 here so a single binding source produces **both** the Node `.node`
//! addon and the `wasm32` target: a stateful `AnalysisHandle` over `gdscript-ide` (kept alive
//! across edits), queried by `(fileId, offset)`, returning `serde` JSON. The npm packaging lives in
//! `bindings/node/`; the wasm-bindgen fallback lives in `bindings/wasm/`.
//!
//! Phase 0: empty `cdylib` stub (proves the cross-compile toolchain before there is anything to bind).
#![cfg_attr(docsrs, feature(doc_cfg))]
