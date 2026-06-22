//! `gdscript-db` — the input layer for the analyzer.
//!
//! Holds the virtual file system (`FileId` → text, always injected — never `std::fs`), the project
//! model, and `apply_change`. Phase 0/1/2 use plain maps with re-parse-on-change; Phase 3 adopts
//! salsa (inputs + tracked queries + durability) for incremental recomputation.
//!
//! Phase 0: empty, compiling stub. Must build for `wasm32`.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        // Phase 0: this crate is an empty, compiling stub.
    }
}
