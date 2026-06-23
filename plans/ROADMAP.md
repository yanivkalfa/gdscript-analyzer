# ROADMAP — Sequencing, Milestones, MVP & v1.0

> Canonical. Fixes the phase order, what ships in each, the Tier 0→3 mapping, and the dependency graph. Phase docs hold the detail; this fixes the *sequence* and *exit criteria*.

> **Current status (2026-06):** Phase 0 ✅ and Phase 1 ✅ are **COMPLETE and shipped to `master`** (CI green; both the Node and browser FFI demos pass). Broad real-corpus hardening of the parser is ongoing. **Active next: Phase 2 (the MVP).**

## The shape of the build

The owner's directive sets the spine: **ecosystem first, then features, value-earliest within features.**

```
Phase 0 ── ECOSYSTEM & TOOLING ───────────────► repo runs, CI green, releases automate, Godot-sync live
              │  (no analyzer features yet — pure foundation)
              ▼
Phase 1 ── PARSER & SYNTAX MVP (Tier 0) ──────► parse, CST, document symbols, folding, by-name completion
              │                                  + AnalysisHost skeleton + napi/wasm "hello analyzer"
              ▼
Phase 2 ── API + SINGLE-FILE SEMANTICS (Tier 1) ► extension_api.json model, single-file inference,
              │  ★ MVP COMPLETE ★                 hover w/ inferred types, member completion, sig-help,
              │                                    inlay hints, type/lint diagnostics
              ▼
        ┌─────┴───────────────────────────────────────────┐
        ▼                                                   ▼
Phase 3 ── PROJECT-WIDE + INCREMENTAL (Tier 2)      Phase 5 ── CLIENTS & DISTRIBUTION
   class_name/preload/extends graph, autoloads,        (can start the LSP + guitkx swap as soon
   cross-file goto/refs/rename, salsa                   as Phase 2 lands; web playground after WASM)
        │                                                   │
        ▼                                                   │
Phase 4 ── SCENE AWARENESS (Tier 3 slice) ◄─────────────────┘
   .tscn parsing, node-path typing ($Path → Button)
        │
        ▼
Phase 6 ── v1.0 RELEASE (Tier 3 full)
   full 48-warning set, flow narrowing, formatter, perf, docs, 1.0
```

## Phase-by-phase: deliverable + exit criteria

### Phase 0 — Ecosystem & Tooling *(no features; the foundation)* — ✅ **COMPLETE**
**Status:** shipped to `master`; CI green; docs site live; godot-sync bot live.
**Ships:** the cargo workspace skeleton (all crate stubs compiling), CI (fmt/clippy/test matrix/MSRV/wasm-check/coverage), the release toolchain (release-plz + changesets), docs scaffold (mdBook + docs.rs), governance files, dual licensing, the `xtask` automation, the **Godot-sync GitHub Action**, and `extension_api.json` vendored + codegen producing a `gdscript-api` data blob. A `CONTRIBUTING`/`README` so an external contributor can clone, build, and run.
**Exit criteria:** `cargo xtask ci` is green locally and in Actions; a no-op release dry-run succeeds; the Godot-sync workflow runs (dispatch) and opens a PR against a synthetic API change; `cargo check -p gdscript-ide --target wasm32-unknown-unknown` passes.
**Doc:** [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md), [`GODOT-SYNC.md`](GODOT-SYNC.md).

### Phase 1 — Parser & Syntax MVP *(Tier 0)* — ✅ **COMPLETE**
**Status:** shipped to `master` via PR #18; CI green; the Node (`hello.mjs`) and browser (wasm-pack) FFI demos both produce document symbols; tree-sitter differential oracle passing. Broad real-corpus hardening (parsing large idiomatic projects) is ongoing.
**Ships:** `gdscript-syntax` (logos lexer + indentation pre-pass + hand-written recursive-descent parser → `cstree` CST + AST), error recovery, the `Parser` trait (tree-sitter optional MVP backend + golden-tree differential oracle); `gdscript-base`; the `AnalysisHost`/`Analysis` skeleton in `gdscript-ide`; the `gdscript-ffi` napi+wasm binding returning **parse diagnostics, document symbols, folding ranges, and by-name (no-type) completion**.
**Exit criteria:** parses the entire Godot demo-projects corpus + a fixtures suite with zero panics and lossless round-trip (CST → source byte-identical); differential test vs tree-sitter passes; a Node script and a browser page both load the binding and get document symbols for a `.gd` file. **No Godot editor anywhere in the loop.**
**Doc:** [`PHASE-1-PARSER-AND-SYNTAX-MVP.md`](PHASE-1-PARSER-AND-SYNTAX-MVP.md).

### Phase 2 — API + Single-File Semantics *(Tier 1)* — **★ MVP ★**
**Ships:** `gdscript-api` fully wired (engine classes/inheritance/methods/properties/signals/enums + hand-authored GDScript layer + doc-XML hover); `gdscript-hir` single-file: name resolution (locals, members, `self`, inherited, globals), **gradual type inference** (`:=`, annotations, member access, returns, `is`/`as` narrowing), and the core type/safety diagnostics. Features: **hover with inferred types, member completion, signature help, inlay hints (inferred types), annotation/keyword/global completion, parse+type diagnostics, basic code actions** ("add type annotation", "import/preload").
**Exit criteria (= MVP):** on a single `.gd` file with no project context, completion after `button.` lists `Button`/`Control`/`Node` members; hover on `var x := get_node(...)` shows the inferred type; type-mismatch + a subset of the Godot `UNSAFE_*` warnings fire with engine-matching messages; **guitkx can replace its `godotProxy.ts` completion+hover path** for embedded GDScript using the napi build (validated against the guitkx repo). Performance: <50ms cold single-file analysis, <5ms warm.
**Doc:** [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md).

### Phase 3 — Project-Wide & Incremental *(Tier 2)*
**Ships:** the project model in `gdscript-db` (workspace scan, VFS, `project.godot` parse → autoloads + global groups + Godot version detection); the `class_name` global registry; `preload`/`load`/`extends` cross-file resolution; cross-file **go-to-definition, find-references, rename, workspace symbols**; **salsa** adopted for incremental recompute with durability. This phase holds most of the engineering risk (stale-cache invalidation).
**Exit criteria:** on a real multi-file Godot project, rename a `class_name` symbol and all references across files update; find-references on a method is complete and correct; editing one file does **not** re-type-check the whole project (measured: keystroke latency flat as project grows); autoload singletons complete globally.
**Doc:** [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md).

### Phase 4 — Scene Awareness *(Tier 3 slice — the killer feature)*
**Ships:** `gdscript-scene` (`.tscn`/`.tres` text parser → node tree with types, scripts, instanced sub-scenes); typing of `$Path`, `%Unique`, `@onready var x = $Path`, and `get_node("...")` by resolving the owning scene's node type; diagnostics for invalid node paths. Start with the 90% slice (direct `type=` nodes in the single owning scene) — a thin version can be **pulled forward** after Phase 2 as a wow-demo.
**Exit criteria:** `$Panel/VBox/StartButton` infers `Button` (not just `Node`) with zero annotations; an invalid `$DoesNotExist` path warns; attached-script types refine node types. This is intelligence the Godot editor LSP cannot produce.
**Doc:** [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md).

### Phase 5 — Clients & Distribution *(parallelizable from Phase 2)*
**Ships:** `gdscript-lsp` (a real, standalone, spec-compliant LSP server — with semantic tokens, inlay hints, workspace symbols, rename that the engine LSP lacks); the **guitkx integration** (delete `godotProxy.ts`, consume the napi package via a Volar-style source-map adapter); `gdscript-cli`; the **WASM web playground** (Monaco/CodeMirror, à la Ruff/Biome); and the published packages on **crates.io + npm** (per-platform napi binaries + wasm) GA, with the web playground live.
**Exit criteria:** the standalone LSP works in VS Code/Neovim with no Godot running; guitkx ships with the proxy removed; `npm i @gdscript-analyzer/core` + `cargo add gdscript-ide` both work; the playground analyzes pasted GDScript in-browser.
**Doc:** [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md).

### Phase 6 — v1.0 Release *(Tier 3 full)*
**Ships:** the complete Godot **48-warning** set with project-setting gating; full control-flow narrowing (beating the engine checker on `is`/`as` guards); a formatter (gdformat-compatible or better); performance hardening (large-project benchmarks); complete docs + "add a client" guide; stabilized semver'd 1.0 API.
**Exit criteria:** the v1.0 success bar in [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) §6 — real projects, scene typing, full warnings, stable packages, a live playground, and ≥1 external consumer beyond guitkx/our own clients.
**Doc:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md).

## Tier → Phase mapping (from [`research/09`](research/09-type-system-and-inference.md))

| Tier | Capability | Phase | Value/Risk |
|---|---|---|---|
| **0** | Parse + symbol table + by-name completion (no inference) | 1 | High value / Low risk — ships fast, ~80% of perceived "it works" |
| **1** | Single-file inference, hover w/ types, type diagnostics | 2 | **Biggest quality jump**, self-contained → do early |
| **3 (slice)** | Scene-typed node paths (single owning scene) | 4 (early slice possible) | Outsized wow / Moderate risk → pull a slice forward |
| **2** | Project-wide graph + incremental | 3 | Most engineering risk (invalidation) |
| **3 (full)** | Recursive scenes + flow narrowing + full warnings | 6 | Multi-year polish / lowest-but-steady |

Recommended value-earliest order (within the dependency constraints): **0 → 1 → early scene slice → 2 → rest of 3.**

## Dependency graph (what blocks what)

- **Phase 0** blocks everything (no repo/CI/data → nothing to build on).
- **Phase 1** blocks all semantics (no CST → no HIR).
- **Phase 2** blocks the guitkx swap and the MVP; needs `gdscript-api` (a Phase-0 codegen output) + Phase 1.
- **Phase 3** needs Phase 2 (single-file inference is the unit project-wide composes).
- **Phase 4** needs Phase 2 (to type a node, you need the type layer) and benefits from Phase 3 (cross-scene), but a single-scene slice only needs Phase 2 + `gdscript-scene`.
- **Phase 5** (LSP + guitkx + CLI) can begin as soon as Phase 2's API is stable; the **playground** needs the WASM binding (built in Phase 0, fed features as they land). **crates.io/npm GA** should wait until the Phase-2 API is semver-stable enough to not churn consumers.
- **Phase 6** needs all of the above.

## MVP and v1.0 in one line each

- **MVP** = end of **Phase 2**: a standalone, editor-free, single-file GDScript analyzer with real inference, consumable from Node + browser, with **guitkx's Godot proxy replaced**.
- **v1.0** = end of **Phase 6**: a project-wide, scene-aware, fully-warned, documented, semver-stable analyzer on crates.io + npm with a live playground and external adopters.

## A note on time

These phases are **scope units, not calendar commitments** — this is a serious, possibly multi-year, foundation (the type-inference research is explicit that full Tier 3 is "the multi-year polish"). Sequence and exit criteria are fixed here; the owner sets the pace. The single biggest risk is **project-wide incremental invalidation** (Phase 3); the single biggest enabler is **`extension_api.json`** (a complete, machine-readable engine model we get for free).
