# Architecture

This page is a short orientation. The **authoritative** technical reference is
[`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md), which fixes the
crate layering, the public API shape, the FFI/WASM strategy, the
incremental-computation plan, the engine data model, and the portability rules.
Read it before making architecturally consequential changes — and record any
such decision as a new [ADR](../adr/README.md).

## The big picture

gdscript-analyzer copies rust-analyzer's proven discipline:

1. **Layered crates, depending only downward.** Lower crates know nothing about
   LSP or FFI. See [Crate layout](./crates.md).
2. **A protocol-neutral analysis API.** `gdscript-ide` exposes
   `AnalysisHost` + immutable `Analysis` snapshots; every result is POD with
   byte offsets, never `lsp-types`. Clients map POD → their protocol.
   ([ADR-0001](../adr/0001-rust-library-not-server.md).)
3. **A parser we own.** A hand-written, lossless, error-recovering recursive-
   descent parser producing a `cstree` CST. tree-sitter-gdscript is only the
   MVP bootstrap and a permanent differential **test oracle**, never the
   grammar-of-record. ([ADR-0002](../adr/0002-handwritten-parser-treesitter-oracle.md).)
4. **One binding, two targets.** A single `gdscript-ffi` crate compiles via
   napi-rs v3 to both a Node `.node` addon and a `wasm32` target.
   ([ADR-0003](../adr/0003-napi-rs-v3-dual-target.md).)

## Cross-cutting invariants

- **The core is portable to WASM.** No `std::fs`, no `Instant::now()` /
  `SystemTime::now()`, no threads in the hot path, `getrandom`'s JS backend only
  in the wasm binding. File contents and clocks are *injected*. CI enforces this
  with `cargo check -p gdscript-ide --target wasm32-unknown-unknown` on every
  PR — the single most important Phase-0 invariant after "it compiles."
- **Engine-neutral results.** The library returns byte offsets + POD structs;
  clients convert to UTF-16 and their protocol shapes.
- **Stay synced with Godot, automatically.** The engine model is generated from
  `extension_api.json` + doc XML and kept current by a sync workflow.
- **Incremental, later.** The MVP recomputes whole files (they are small);
  salsa is adopted at Phase 3 when cross-file resolution makes per-keystroke
  full recompute untenable. Every derived computation is written as a pure
  `(db, file) -> value` function so the swap is localized.

## Where to go next

- [Crate layout](./crates.md) — the layer table and dependency edges.
- [Build & test](./build.md) — the exact onboarding commands.
- [`plans/ROADMAP.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/ROADMAP.md) — phase sequencing and exit
  criteria.
- [`plans/00-VISION-AND-SCOPE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/00-VISION-AND-SCOPE.md) — what
  the project is and isn't.
