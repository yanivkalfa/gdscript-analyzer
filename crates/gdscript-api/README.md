# gdscript-api

The **Godot engine model** — classes, methods, signals, properties, enums, and
builtins — generated from Godot's `extension_api.json`, so the analyzer knows the
4.x standard library without a running engine.

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You normally don't depend on this crate directly — use
**[`gdscript-ide`](https://crates.io/crates/gdscript-ide)** (the public API), or
the **[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(Node) / **[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) packages.

- **API docs:** https://docs.rs/gdscript-api
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
