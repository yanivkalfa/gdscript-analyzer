//! `gdscript-hir` — the semantic / type layer.
//!
//! Lowers the AST to a HIR (an `ItemTree` of signatures + per-function `Body`), runs name resolution
//! (local → class member → inherited → global), gradual type inference (Variant by default,
//! `:=`/annotations, member lookup over the engine inheritance table, `is`/`as` narrowing), and the
//! GDScript warning checks. Single-file in Phase 2; project-wide + scene-aware later.
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
