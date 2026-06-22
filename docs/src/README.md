# gdscript-analyzer

**gdscript-analyzer is a Rust library that parses and semantically understands
GDScript (Godot 4.x)** and exposes an engine-independent query API —
completion, hover, diagnostics, go-to-definition, find-references, rename, type
inference, and more — that any tool can embed: natively, in Node via
[napi-rs](https://napi.rs), in the browser via WebAssembly, or from other
languages through a C ABI.

Think of it as **"Roslyn / rust-analyzer for Godot"**: the reusable *analysis
brain*, deliberately separated from any one editor or server.

## A library, not a server

The single most important design decision is that this is a **library, not an
LSP server**. The analysis engine is protocol-neutral: you give it file
contents and byte offsets, and it returns plain old data (POD) structs. It knows
nothing about LSP, JSON-RPC, or any particular editor.

*Clients* — a standalone LSP server, a CLI, a web playground, or a markup
toolchain such as guitkx — each map those neutral results onto their own
protocol. This is rust-analyzer's discipline: the `ide` crate "knows nothing
about LSP"; only a thin server crate does. See
[ADR-0001](adr/0001-rust-library-not-server.md) and
[`plans/00-VISION-AND-SCOPE.md`](../../plans/00-VISION-AND-SCOPE.md).

## Who consumes it

In rough priority order:

1. **guitkx** — the ReactiveUI-for-Godot markup toolchain, our first client and
   validation harness. It needs GDScript intelligence inside markup `{expr}`
   blocks *without a running Godot editor*.
2. **A standalone GDScript LSP server** (`gdscript-lsp`) — both a real product
   and the reference client for the API.
3. **A CLI** (`gdscript-cli`) — `check` / `lint` / `format` / `symbols` for CI
   and pre-commit hooks.
4. **A web playground** — Rust→WASM, in-browser analysis of pasted GDScript.
5. **The wider community** — other editors and other-language consumers.

## Why it does not exist yet, and why it should

Today the only way to get *semantic* GDScript intelligence is to run the Godot
editor and talk to its built-in LSP over TCP. Every other tool in the space is
either syntactic-only, Python/.NET-locked, or editor-bound. The empty
quadrant — **semantic-grade + engine-independent + Rust→multi-target +
library-first** — is exactly what this project fills. The full landscape
analysis is in [`plans/00-VISION-AND-SCOPE.md`](../../plans/00-VISION-AND-SCOPE.md).

## Project status — Phase 0

The project is being built **ecosystem-first**: tooling, CI, release
automation, the engine-data sync pipeline, and docs come *before* analyzer
features. We are in **Phase 0**, which ships a runnable, releasable,
contributable repository with **zero analyzer features** — the crate skeleton,
the build/release machinery, and this documentation scaffold.

The analysis APIs sketched throughout these docs **land in Phase 1 and later**
and are marked as forthcoming where relevant. For the full sequencing, see
[`plans/ROADMAP.md`](../../plans/ROADMAP.md); for the foundation work itself,
[`plans/PHASE-0-ECOSYSTEM-AND-TOOLING.md`](../../plans/PHASE-0-ECOSYSTEM-AND-TOOLING.md).

## Next steps

- [Install](guide/install.md) the crate or npm package.
- [Quickstart](guide/quickstart.md) — analyze a `.gd` string (forthcoming API).
- Consuming the library from [Rust](consume/rust.md), [Node](consume/node.md),
  or [the browser](consume/browser.md).
- [Contributing](contributing/architecture.md) and the
  [Architecture Decision Records](adr/README.md).
