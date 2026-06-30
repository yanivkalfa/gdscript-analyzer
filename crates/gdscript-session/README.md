# gdscript-session

A URI-keyed session over the analyzer that returns structured `serde_json::Value`
results — the shared, pure-Rust, **wasm-clean** core the Node (napi) and browser
(wasm) bindings wrap as thin delegators.

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You normally don't depend on this crate directly — use
**[`gdscript-ide`](https://crates.io/crates/gdscript-ide)** (the public Rust API), or
the **[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(Node) / **[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) packages.

It owns the document lifecycle and a canonical URI→`FileId` interner so a binding
only marshals values: open/change/close documents by URI, then query (diagnostics,
hover, completion, …) and get the result as a native `serde_json::Value` (no
client-side `JSON.parse`; cross-file targets carry their resolved `uri`). Holding
all logic here keeps the binding crates near-trivial and fully unit-testable in pure
Rust.

- **API docs:** https://docs.rs/gdscript-session
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
