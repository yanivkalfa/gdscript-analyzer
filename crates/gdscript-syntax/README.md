# gdscript-syntax

The lexer, indentation pre-pass, and **lossless (cstree) parser** that turn
GDScript source into a concrete syntax tree — error-resilient, full-fidelity
(every byte, including trivia, is preserved).

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You normally don't depend on this crate directly — use
**[`gdscript-ide`](https://crates.io/crates/gdscript-ide)** (the public API), or
the **[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(Node) / **[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) packages.

- **API docs:** https://docs.rs/gdscript-syntax
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
