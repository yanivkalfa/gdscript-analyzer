//! `gdscript-wasm` — the wasm-bindgen browser binding (the documented fallback to the napi-rs
//! `wasm32` target; see ADR-0003 and `plans/research/08-wasm-web-and-bindings.md`).
//!
//! Phase 5 wires the web-playground analysis entry points here (source string in → diagnostics /
//! completions JSON out), behind the size-optimized `wasm-release` profile.
//!
//! Phase 0: empty `cdylib` stub.
#![cfg_attr(docsrs, feature(doc_cfg))]
