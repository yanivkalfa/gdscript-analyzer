//! `gdscript-scene` — a `.tscn`/`.tres` **text** parser for scene-aware analysis (Phase 4).
//!
//! Godot's text scene format is INI-like: bracketed section headers (`[node …]`, `[ext_resource …]`,
//! …) followed by `key = value` property lines. This crate parses that structure — node names,
//! types, parent paths, attached scripts, `unique_name_in_owner`, and instanced sub-scenes — into a
//! [`SceneModel`] **with byte spans**, so the type layer (Phase-4 M1+) can resolve `$Path` /
//! `%Unique` / `get_node("…")` to a node's real `Control`/`Node` subclass instead of bare `Node` —
//! intelligence the Godot editor's own LSP produces only in-editor and never flows into inference.
//!
//! **M0 scope:** the pure, wasm-clean [`parse_scene`] (`&str -> SceneModel`) + the model + byte
//! spans. It **records** the typing inputs (`type=`/`script=`/`instance=`); it does **not** resolve
//! a `Ty` (M1), recurse into instanced sub-scenes, build the project-wide script↔scene index, or
//! cache via salsa (M1+). See `plans/PHASE-4-M0-PLAYBOOK.md`.
//!
//! **Invariant:** the parser is strictly additive and **never fails** — every binary/malformed/
//! unknown form degrades to an empty-or-partial model + a [`SceneProblem`], never a panic or `Err`.
//! The floor is always parity with the engine's `Node`-everywhere baseline.
//!
//! **Portability:** a core crate — wasm32-clean (no `std::fs`, no `Instant`, no threads); `.tscn`
//! text is injected via the VFS exactly like `.gd`.
#![cfg_attr(docsrs, feature(doc_cfg))]

mod model;
mod parse;

pub use model::{
    ExtId, ExtResource, NodeIdx, NodePathResolution, NodeProp, SceneConnection, SceneKind,
    SceneModel, SceneNode, SceneProblem, SubResource,
};
pub use parse::parse_scene;

#[cfg(test)]
mod tests;
