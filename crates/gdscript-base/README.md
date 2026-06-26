# gdscript-base

Foundational POD types — `FileId`, `TextSize`/`TextRange`, `LineIndex`, and shared
result structs — used across the whole workspace.

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You normally don't depend on this crate directly — use
**[`gdscript-ide`](https://crates.io/crates/gdscript-ide)** (the public API), or
the **[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(Node) / **[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) packages.

- **API docs:** https://docs.rs/gdscript-base
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
