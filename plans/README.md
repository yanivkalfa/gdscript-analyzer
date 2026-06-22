# gdscript-analyzer — Project Plan

> A **Roslyn for Godot**: a fast, embeddable, multi-target **GDScript static-analysis library**, written in Rust, that any tool can consume — LSP servers, web playgrounds, CLI linters, CI, and the [`guitkx`](https://github.com/yanivkalfa/ReactiveUI-Godot) markup toolchain (our first client).

This directory holds the complete, multi-phase implementation plan. It was produced from a thorough research pass (nine cited research notes under [`research/`](research/)) covering distribution tooling, parsing, Godot API sync, the GDScript language surface, prior art, analyzer architecture, the OSS ecosystem, WASM/web, and type inference.

## How to read this plan

Read in order. The first three docs are **canonical** — every phase doc obeys the decisions they fix.

| Doc | What it covers |
|-----|----------------|
| [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) | What we're building and why; the market gap; consumers; non-goals; success criteria; the language & "library-not-server" decisions. |
| [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) | The layered crate design, the public `AnalysisHost`/`Analysis` API, the FFI/WASM strategy, the salsa decision, the portability rules. **The canonical technical reference.** |
| [`ROADMAP.md`](ROADMAP.md) | Phase sequencing, milestones, the Tier 0→3 mapping, the MVP and v1.0 definitions, dependencies between phases. |
| [`GODOT-SYNC.md`](GODOT-SYNC.md) | The "rule file": how we track every Godot release and propagate API changes into the analyzer (automated). The owner's #1 priority. |

### The phases

The owner's directive: **build the ecosystem/tooling FIRST (Phase 0), then the library.**

| Phase | Doc | Theme | Tier |
|-------|-----|-------|------|
| **0** | [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) | Repo, workspace, CI, git flow, versioning, changelogs, docs, governance, licensing, build (napi/wasm), Godot-sync automation, "how to run it". | — |
| **1** | [`PHASE-1-PARSER-AND-SYNTAX-MVP.md`](PHASE-1-PARSER-AND-SYNTAX-MVP.md) | Lexer + lossless parser (CST) + AST; the `AnalysisHost`/`Analysis` skeleton; syntactic features (parse diagnostics, document symbols, folding, by-name completion). | Tier 0 |
| **2** | [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) | `extension_api.json` ingestion; single-file name resolution + type inference; hover, member completion, signature help, inlay hints, type/lint diagnostics. **MVP completes here.** | Tier 1 |
| **3** | [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) | Project graph (`class_name`, `preload`, `extends`, autoloads); cross-file goto/find-refs/rename/workspace-symbols; salsa incremental. | Tier 2 |
| **4** | [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) | `.tscn` parsing; node-path typing (`$Path`/`%Unique`/`get_node`). **The killer feature Godot's own LSP lacks.** | Tier 3 (slice) |
| **5** | [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) | The LSP server; the **guitkx integration** (delete the Godot proxy); the WASM web playground; the CLI; crates.io + npm GA. | — |
| **6** | [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) | Full 48-warning set; flow narrowing; formatter; performance; docs; the 1.0 bar. | Tier 3 (full) |

## The big-picture decisions (already settled)

These were decided with the owner before planning and are **fixed**:

1. **Language: Rust.** Maximal reach (native + Node-via-napi + browser-via-WASM + other-language bindings), best performance, and the credibility of being where every modern reusable analyzer lives (rust-analyzer, Biome, Ruff, oxc, swc). See [`00`](00-VISION-AND-SCOPE.md).
2. **A library, not an LSP server.** The analysis engine is engine-/protocol-neutral; the LSP server is just one of several clients. See [`01`](01-ARCHITECTURE.md).
3. **Ecosystem foundation ambition.** This is a serious, production-grade, community-supporting project — "the foundation other Godot tooling builds on," not a one-off for guitkx.
4. **Separate repo:** `C:\Yanivs\GameDev\gdscript-analyzer` (this repo), independent of the Unity/Godot ReactiveUI repos.
5. **guitkx is the first client** — it validates the API by replacing its current "proxy embedded GDScript to a running Godot editor" hack with our analyzer.

## Research notes (evidence base)

All claims in this plan trace to these cited notes:

- [`research/01-rust-distribution-tooling.md`](research/01-rust-distribution-tooling.md) — napi-rs/wasm/crates.io distribution; oxc/swc/Biome/Ruff precedents.
- [`research/02-parsing-strategy.md`](research/02-parsing-strategy.md) — tree-sitter-gdscript vs hand-written cstree parser; indentation handling.
- [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md) — `extension_api.json` schema; the sync pipeline; per-version dump sources.
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — the full GDScript 2.0 surface; 36 annotations; 48 warnings; LSP feature table.
- [`research/05-prior-art-and-landscape.md`](research/05-prior-art-and-landscape.md) — gdtoolkit, gdstyle, GDShrapt, the engine LSP; the market gap.
- [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) — rust-analyzer-style layering; `AnalysisHost`/`Analysis`; FFI shape.
- [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) — workspace layout, CI, release-plz/changesets, docs, governance, licensing.
- [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md) — WASM toolchain, playground precedents, bundle-size & data-shipping plan, portability rules.
- [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) — GDScript's (easy, gradual) type system; the HIR/binder/checker design; scene-aware typing; Tier 0→3.

---

*Status: planning. No library code written yet. This plan is the source of truth for sequencing the build.*
