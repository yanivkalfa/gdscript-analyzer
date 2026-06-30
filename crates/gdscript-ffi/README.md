# gdscript-ffi

The **napi-rs v3 Node binding** — a thin `.node` addon over the analyzer, exposing
the [`gdscript-session`](https://crates.io/crates/gdscript-session) core to
JavaScript.

An internal layer of **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
a fast, embeddable GDScript (Godot 4.x) static-analysis library ("Roslyn for
Godot"). You don't depend on this crate directly — install the published npm package
**[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**,
which ships this addon. (For the browser, use
**[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**;
for Rust, **[`gdscript-ide`](https://crates.io/crates/gdscript-ide)**.)

All real logic — the URI→`FileId` interner, the document lifecycle, the
serialization of query results — lives in the pure-Rust, fully unit-tested
`gdscript-session` core (ADR-0003). This crate is just the napi marshaling boundary,
returning native JS values (no `JSON.parse`). It builds only with the napi toolchain
(`@napi-rs/cli`), so it is CI-built per platform rather than via plain `cargo`.

- **Package:** https://www.npmjs.com/package/@gdscript-analyzer/core
- **Crate map:** [repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
- **License:** MIT OR Apache-2.0
