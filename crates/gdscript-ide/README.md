# gdscript-ide

The **public API** of [gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer) — a fast, embeddable GDScript (Godot 4.x) static-analysis library. **Roslyn for Godot.**

[![crates.io](https://img.shields.io/crates/v/gdscript-ide.svg?logo=rust)](https://crates.io/crates/gdscript-ide)
[![docs.rs](https://img.shields.io/docsrs/gdscript-ide?logo=docsdotrs)](https://docs.rs/gdscript-ide)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/yanivkalfa/gdscript-analyzer#license)

This is the crate you depend on. It exposes an `AnalysisHost` that owns the
project's files (push edits through `&mut`) and a cheap, cloneable `Analysis`
snapshot that answers queries — **diagnostics, type-aware hover, completion,
go-to-definition, find-references, rename, document & workspace symbols, signature
help, folding ranges, inlay hints, code actions** — as plain data (POD) addressed
by **UTF-8 byte offsets**. It is engine-neutral and protocol-neutral: no LSP, no
JSON-RPC, and **no running Godot editor**.

```sh
cargo add gdscript-ide
```

See **[docs.rs/gdscript-ide](https://docs.rs/gdscript-ide)** for the exact API.

**Not using Rust?** The same engine ships as
**[`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core)**
(native Node) and
**[`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm)**
(browser) — try it in the
**[playground](https://yanivkalfa.github.io/gdscript-analyzer/playground/)**.

Part of the gdscript-analyzer workspace — see the
[repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
for the full crate map. Licensed MIT OR Apache-2.0.
