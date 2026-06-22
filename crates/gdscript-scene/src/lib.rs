//! `gdscript-scene` — a `.tscn`/`.tres` text parser for scene-aware analysis.
//!
//! Phase 4 fills this in: parse the INI-like scene format into a node tree (name, type, parent
//! path, attached script, `unique_name_in_owner`, instanced sub-scenes) so the type layer can
//! resolve `$Path`/`%Unique`/`get_node()` to the node's real `Control` type — intelligence the
//! Godot editor LSP cannot produce.
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
