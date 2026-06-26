# gdscript-scene

A **`.tscn`/`.tres` parser** that feeds node-path typing — so `$Node`,
`%Unique`, and `get_node(...)` are typed against the actual scene tree. This is
the scene-aware capability the engine's own LSP doesn't fully provide.

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You normally don't depend on this crate directly — use
**[`gdscript-ide`](https://crates.io/crates/gdscript-ide)** (the public API), or
the **[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(Node) / **[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) packages.

- **API docs:** https://docs.rs/gdscript-scene
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
