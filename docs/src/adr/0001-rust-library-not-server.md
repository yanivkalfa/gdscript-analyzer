# ADR-0001: Rust + library-not-server

- **Status:** Accepted
- **Date:** 2026-06-22

## Context

The motivating problem is that there is no way to get *semantic* GDScript
intelligence without a running Godot editor. Godot's built-in LSP requires the
editor process and a TCP connection (`:6005`); every other tool in the space is
syntactic-only, runtime-locked (Python, .NET), or otherwise editor-bound. The
landscape analysis in
[`plans/00-VISION-AND-SCOPE.md`](../../../plans/00-VISION-AND-SCOPE.md) §2 found
exactly one empty quadrant: **semantic-grade + engine-independent +
multi-target + library-first.** Filling it requires two foundational choices.

**Implementation language.** A reusable analysis core must reach native (CLI,
CI), Node (LSP servers, including guitkx's), the browser (web playgrounds), and
ideally other languages (Python, C ABI). It must also be fast enough for
keystroke-latency analysis on real projects, and credible enough to attract
contributors to "the foundation." Candidates weighed were Rust, TypeScript, C#,
and Python.

**Shape.** Even granting the language, the engine could be built as an LSP
*server* (speaks JSON-RPC, owns the editor protocol) or as a protocol-neutral
*library* (takes file contents + offsets, returns plain data). The first client,
guitkx, needs GDScript intelligence *inside* markup `{expr}` blocks via a
source-map adapter — which is an **analysis** need, not an LSP need. A
server-shaped core could not serve it without contortions.

## Decision

**We will build gdscript-analyzer in Rust, as an engine- and protocol-neutral
library — not as an LSP server.**

- **Rust**, because it reaches a *superset* of TypeScript's targets — native,
  Node via napi-rs, browser via WASM, other languages via PyO3 / C ABI — at
  full speed, and because every modern reusable analyzer (rust-analyzer, Biome,
  Ruff, oxc, swc) is Rust, which is itself a credibility and contribution
  signal. (TypeScript would run the analyzer on V8; C# is .NET-locked; Python is
  too slow and runtime-bound.)
- **A library**, following rust-analyzer's discipline: the analysis engine takes
  a `FileId` + **byte offsets** and returns **POD** (plain-old-data,
  serde-serializable) results. It knows nothing about LSP, JSON-RPC, or any
  editor. The public surface is `AnalysisHost` + immutable `Analysis` snapshots
  in the `gdscript-ide` crate. **Clients** — a standalone LSP server, a CLI, a
  web playground, the guitkx adapter — each map our neutral results onto their
  own protocol. The LSP server is *just one client*, not the core.

See [`plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §1–2.

## Consequences

**Easier / positive.**

- One core reaches every target; no per-target rewrite.
- guitkx becomes a first-class client (source-map adapter over the same
  library), which is the project's primary validation harness.
- Results are protocol-neutral POD, so a CLI, an LSP server, and a browser
  playground all consume the identical API — and we can swap or add protocols
  without touching the analysis core.
- Being Rust gives native performance and access to the proven analyzer
  ecosystem (`cstree`, `salsa`, `logos`).

**Harder / negative — the constraints this creates.**

- **Strict portability rules.** Because the core must compile to `wasm32`, it
  may not use `std::fs`, `Instant::now()`/`SystemTime::now()`, or threads in the
  hot path. File contents and clocks are *injected*; the client (or the native
  binding) does the I/O. CI enforces this with
  `cargo check -p gdscript-ide --target wasm32-unknown-unknown` on every PR.
- **A position-encoding seam.** The core emits byte offsets; LSP wants UTF-16.
  Each client converts at its boundary (a known footgun, handled in
  `gdscript-base`'s `LineIndex`).
- **No `lsp-types` in the core, ever.** Diagnostics carry our own codes and byte
  ranges; clients translate. This is more glue per client but keeps the contract
  clean.
- Rust's compile times and learning curve are a contributor cost we accept for
  the reach and performance.
