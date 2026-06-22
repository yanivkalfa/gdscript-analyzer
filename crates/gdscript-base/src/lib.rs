//! `gdscript-base` — foundational POD types shared across the gdscript-analyzer.
//!
//! This is the lowest layer of the crate stack (see `plans/01-ARCHITECTURE.md` §1). It will hold
//! `FileId`, `TextSize`/`TextRange`, `LineIndex` (byte ↔ UTF-16/line-col conversion), and the
//! engine-neutral, `serde`-serializable result structs every client maps to its own protocol.
//!
//! Phase 0: empty, compiling stub. It must build for `wasm32` — no `std::fs`, no clocks, no threads.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Phase 0: this crate is an empty, compiling stub.
    }
}
