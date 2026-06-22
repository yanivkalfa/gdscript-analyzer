//! `gdscript-syntax` — lexer + indentation pre-pass + lossless parser for GDScript.
//!
//! Phase 1 fills this in: a `logos` lexer, a hand-written indentation pre-pass (INDENT/DEDENT/
//! NEWLINE), and a hand-written recursive-descent parser producing a lossless `cstree` CST plus a
//! typed AST, behind a `Parser` trait (tree-sitter-gdscript is the MVP bootstrap + differential
//! test oracle, never the grammar-of-record — see ADR-0002).
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
