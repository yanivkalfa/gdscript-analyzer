# 00 — Vision & Scope

> Canonical. Defines what `gdscript-analyzer` is, why it exists, who consumes it, what it will and won't do, and how we'll know it succeeded.

## 1. One-sentence definition

**`gdscript-analyzer` is a Rust library that parses and semantically understands GDScript (Godot 4.x), exposing an engine-independent query API (completion, hover, diagnostics, go-to-definition, rename, type inference, …) that any tool can embed — natively, in Node via napi, in the browser via WASM, or from other languages.**

It is, deliberately, **"Roslyn / rust-analyzer for Godot"**: the reusable *analysis brain*, separate from any one editor or server.

## 2. Why this exists — the gap

The research (see [`research/05-prior-art-and-landscape.md`](research/05-prior-art-and-landscape.md)) found the GDScript tooling space is busier than expected, but **one quadrant is empty**:

| Project | Lang | Semantics? | Engine-independent? | Multi-target (native/WASM/napi)? | Library-first? |
|---|---|---|---|---|---|
| **Godot built-in LSP** | C++ | partial | ❌ needs running editor (TCP :6005) | ❌ | ❌ |
| **gdtoolkit** (gdformat/gdlint) | Python | ❌ syntactic | ✅ | ❌ (Python runtime) | ⚠️ CLI-first |
| **gdstyle** | Rust | ❌ syntactic only | ✅ | ❌ | ⚠️ lint-shaped |
| **GDQuest formatter** | Rust | ❌ format only | ✅ | ❌ | ❌ |
| **GDShrapt** | C#/.NET | ✅ deep | ✅ | ❌ .NET-locked | ⚠️ |
| **`gdscript-analyzer` (us)** | **Rust** | ✅ | ✅ | ✅ | ✅ |

The empty quadrant — **semantic-grade + engine-independent + Rust→multi-target + library-first** — is exactly what guitkx needs (it currently can't analyze embedded GDScript without a running Godot editor) and what the wider community lacks. There is documented demand: Godot engine issues asking for a project-wide static analyzer, a 2019 GSoC static-analyzer effort, refactoring/rename requests, an engine proposal (#11056) to *externalize* the in-editor LSP, and browser playgrounds that need in-page analysis. Our tagline: **"Ruff/ty for GDScript, with a rust-analyzer-style consumable API and a WASM story no competitor offers."**

## 3. The two settled decisions that shape everything

### 3.1 Language = Rust

Chosen for **reach + performance + credibility** (see the conversation that preceded this plan). A Rust core reaches a *superset* of TypeScript's targets:

- **Native** — any platform, full speed (CLI, CI).
- **Node** — via **napi-rs** (native addon, no WASM overhead) → powers LSP servers, including guitkx's.
- **Browser** — via **WASM** → web playgrounds, in-page analysis.
- **Other languages** — Python (PyO3/maturin), C ABI (cbindgen), so the analyzer can be embedded by tooling in any stack.

Every modern reusable analyzer (rust-analyzer, Biome, Ruff, oxc, swc) is Rust; being Rust is itself a credibility and contribution-magnet signal for "the foundation."

> Note on "TypeScript is being ported to Go": that speeds the TS *compiler*, not TS *programs* at runtime. A TS analyzer would still run on V8. It does not change the reach/perf calculus above.

### 3.2 A library, not an LSP server

The **analysis engine is protocol-neutral**: it takes file contents + positions (byte offsets) and returns plain data structures. It knows nothing about LSP, JSON-RPC, or any editor. **Clients** (an LSP server, the guitkx adapter, a CLI, a WASM playground) each map our neutral results to their protocol. This is rust-analyzer's discipline: the `ide` crate "knows nothing about LSP"; only the thin server crate does. See [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md).

This is *why* guitkx can be a first-class client: it needs GDScript intelligence inside markup `{expr}` blocks, which is not an "LSP" need — it's an analysis need, served by the same library through a source-map adapter.

## 4. Consumers (who we build the API for)

In priority order:

1. **guitkx** (the ReactiveUI-for-Godot markup toolchain) — our **first client and validation harness**. Today it proxies embedded-GDScript completion/hover/definition to a *running Godot editor* over TCP (`ide-extensions/lsp-server/src/godotProxy.ts`). The whole motivation for this project is to **delete that proxy** and answer those queries ourselves, with no editor required. It consumes the **napi** build from its Node LSP server.
2. **A standalone GDScript LSP server** (`gdscript-lsp`) — we ship one, both as a real product (better than the engine's: standalone, spec-compliant, with semantic tokens, inlay hints, workspace symbols, rename — features the engine LSP lacks) and as the reference client for the API.
3. **A CLI** (`gdscript-cli`) — `check`/`lint`/`format`/`symbols` for CI and pre-commit, no editor, no Godot.
4. **A web playground** — Rust→WASM in the browser (Monaco/CodeMirror), à la Ruff's `play.ruff.rs` and Biome's playground; also enables a future guitkx web playground.
5. **The community** — other editors, custom tooling, other-language consumers (Python via PyO3, anything via the C ABI).

## 5. Scope

### In scope (the destination, across phases)
- A **lossless, error-recovering parser** for GDScript 2.0 (Godot 4.x), with incremental reparsing.
- **Name resolution + gradual type inference** (single-file → project-wide).
- **Authoritative engine knowledge** from `extension_api.json` + doc XML, **kept in sync with every Godot release** (see [`GODOT-SYNC.md`](GODOT-SYNC.md)).
- **Project-wide model**: `class_name` registry, `preload`/`load`/`extends` graph, `[autoload]` singletons.
- **`.tscn` scene awareness** for node-path typing (`$Path` → its real Control type).
- The full **IDE feature set**: completion, hover, signature help, go-to-definition, find-references, rename, document/workspace symbols, semantic tokens, inlay hints (inferred types), diagnostics (parse + the 48 Godot warnings + lints), code actions, folding, formatting.
- **Distribution** to crates.io, npm (napi + wasm), and a web playground.

### Out of scope (non-goals)
- **Not a Godot engine, runtime, or GDExtension binding.** We do not *run* GDScript or talk to a live engine. (`godot-rust/gdext` is bindings, not an analyzer — different project; we only borrow its `extension_api.json`-parsing approach.)
- **Not a replacement for the Godot editor.** We complement it. We never require the editor to be running (the opposite of today's guitkx proxy).
- **Not tied to guitkx.** guitkx is the first client, not the owner. The library has no guitkx/markup knowledge; embedded-language support is a *generic* source-map facility any client can use.
- **No bespoke GUI** (beyond the playground demo). We ship a library + reference clients.
- **Not GDScript 1.x (Godot 3).** Godot 4.x / GDScript 2.0 only. (Reconsider only if there's demand.)

## 6. Success criteria

**MVP (end of Phase 2)** — a developer can:
- point a tool at a `.gd` file and get parse + type diagnostics, document symbols, hover with **inferred types**, member/keyword/annotation completion, and signature help — **with no Godot editor running**;
- consume it from Node (napi) and the browser (WASM);
- and **guitkx can replace its Godot proxy** for single-file embedded-GDScript completion/hover.

**v1.0 (end of Phase 6)** — the analyzer:
- handles real multi-file Godot projects (cross-file goto/find-refs/rename, autoloads, `class_name`);
- **types node paths from `.tscn`** (the feature the engine LSP can't do);
- emits the full Godot warning set with messages matching the engine;
- ships stable, documented, semver'd packages on crates.io + npm with a live web playground;
- has ≥1 external consumer beyond guitkx and our own LSP/CLI.

## 7. Guiding principles

1. **The core is portable.** No `std::fs`, no `Instant::now()`, no threads in `gdscript-core`/`-ide` — those break WASM. File contents and clocks are *injected*. (See [`01`](01-ARCHITECTURE.md) §portability and [`research/08`](research/08-wasm-web-and-bindings.md).)
2. **Engine-neutral results.** The library returns offsets + POD structs; clients map to LSP/their protocol. Never leak `lsp-types` into the core.
3. **Stay synced with Godot, automatically.** API drift is the maintenance risk; we automate detection + propagation from day one ([`GODOT-SYNC.md`](GODOT-SYNC.md)).
4. **Own the grammar.** We end on a hand-written parser we control (tree-sitter-gdscript is an MVP stopgap + test oracle, not the grammar-of-record). ([`research/02`](research/02-parsing-strategy.md).)
5. **Ship value earliest.** Tier 0 (syntactic) is useful on its own; Tier 1 (single-file inference) is the biggest quality jump; a thin scene-typing slice is the wow demo. ([`research/09`](research/09-type-system-and-inference.md).)
6. **Production-grade ecosystem first.** The owner's explicit ordering: tooling/CI/release/docs/sync **before** features. ([`PHASE-0`](PHASE-0-ECOSYSTEM-AND-TOOLING.md).)
