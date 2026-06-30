# gdscript-lsp

A standalone, spec-compliant **GDScript Language Server** from [gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer) — a fast, embeddable GDScript (Godot 4.x) static-analysis library. **Roslyn for Godot.**

[![crates.io](https://img.shields.io/crates/v/gdscript-lsp.svg?logo=rust)](https://crates.io/crates/gdscript-lsp)
[![docs.rs](https://img.shields.io/docsrs/gdscript-lsp?logo=docsdotrs)](https://docs.rs/gdscript-lsp)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/yanivkalfa/gdscript-analyzer#license)

An [LSP 3.17](https://microsoft.github.io/language-server-protocol/) server over the
[`gdscript-ide`](https://crates.io/crates/gdscript-ide) core — **no running Godot
editor required**. On `initialized` it scans the workspace to `project.godot` and
loads every `.gd` + `.tscn` into one host, so `class_name` / autoloads / `preload` /
scene-aware typing work and navigation/rename span the whole project (not just open
documents); `workspace/didChangeWatchedFiles` keeps it in sync.

```sh
cargo install gdscript-lsp       # installs the `gdscript-lsp` binary
```

It speaks LSP over stdio — point your editor's GDScript LSP client at the
`gdscript-lsp` binary. Provides diagnostics (debounced), hover, completion,
signature help, go-to-definition, find-references, rename, document & workspace
symbols, folding ranges, and inlay hints. The single mutable host runs on the event
loop (text sync + dispatch); read requests are snapshotted and computed on a bounded
worker pool, so a slow read never blocks edits, and a concurrent edit cancels
in-flight reads (mapped to `ContentModified`).

Part of the gdscript-analyzer workspace — see the
[repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
for the full crate map. Licensed MIT OR Apache-2.0.
