//! `gdscript-ide` — the public, engine-/protocol-neutral analysis API.
//!
//! This is the crate external Rust consumers depend on. It exposes `AnalysisHost` (the single
//! mutable owner; `apply_change`) and immutable, `Send` `Analysis` snapshots whose queries take
//! byte offsets and return plain, `serde`-serializable result structs — never `lsp-types`. Each
//! client (the LSP server, the guitkx adapter, the CLI, the WASM playground) maps these POD results
//! to its own protocol. See `plans/01-ARCHITECTURE.md` §2 and ADR-0001.
//!
//! Phase 0: empty, compiling stub. **This is the crate the wasm portability guard checks**
//! (`cargo check -p gdscript-ide --target wasm32-unknown-unknown`), so it must always build for
//! the browser target — no `std::fs`, no clocks, no threads in the hot path.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Phase 0: this crate is an empty, compiling stub.
    }
}
