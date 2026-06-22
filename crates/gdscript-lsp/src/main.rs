//! `gdscript-lsp` — a standalone GDScript LSP server (Phase 5).
//!
//! The only crate that knows LSP / JSON-RPC; it maps `gdscript-ide` POD results (byte offsets) to
//! the protocol (UTF-16). Unlike Godot's built-in server it needs no running editor and adds
//! semantic tokens, inlay hints, workspace symbols, and rename.
//!
//! Phase 0: placeholder binary.

fn main() {
    eprintln!("gdscript-lsp: not implemented yet (arrives in Phase 5).");
}
