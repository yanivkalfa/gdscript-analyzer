# Crate layout

The workspace is a flat `crates/*` virtual workspace (matklad's "Large Rust
Workspaces"), with thin `bindings/{node,wasm}` packages and an `xtask/` build
crate. **Each crate depends only downward** — lower crates know nothing about
the layers above them. The authoritative version is in
[`plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §1.

## The layer table

| Crate | Responsibility | Depends on | wasm-safe? |
|---|---|---|---|
| `gdscript-base` | POD types: `FileId`, `TextSize`/`TextRange`, `LineIndex`, position/range conversions, the serde result structs shared with clients. No logic. | — | ✅ |
| `gdscript-syntax` | Lexer (`logos`) + indentation pre-pass + hand-written recursive-descent parser → lossless `cstree` CST + typed AST. Error recovery. | base | ✅ |
| `gdscript-api` | The Godot engine model generated from `extension_api.json` + doc XML: classes, inheritance, methods, properties, signals, enums, singletons, utility functions, builtins — plus the hand-authored GDScript layer (keywords, annotations, builtins) the dump omits. | base | ✅ |
| `gdscript-db` | Input layer: a virtual file system (`FileId` → text, **injected**, never `std::fs`), the project model, `apply_change`. MVP: plain maps; v1: salsa inputs + tracked queries. | base, syntax, api | ✅ |
| `gdscript-hir` | Semantic layer: lower AST → HIR, scope tree, name resolution, gradual type inference, the GDScript warning checks. | base, syntax, api, db | ✅ |
| `gdscript-ide` | The **feature** layer and **public API**: `AnalysisHost` + immutable `Analysis`, one method per IDE feature, POD results. **The crate external Rust consumers depend on, and the wasm-check target.** | all above | ✅ |
| `gdscript-scene` | `.tscn`/`.tres` parser; node-tree model for path typing (Phase 4). | base | ✅ |
| `gdscript-ffi` | The **only** crate with napi/wasm glue. napi-rs v3 → a Node `.node` addon **and** a `wasm32` build from one source. Holds an `AnalysisHandle`; JSON in / JSON out. `publish = false`. | ide | n/a (is the binding) |
| `gdscript-lsp` | A real, standalone LSP server binary. The only place that knows `lsp-types`/JSON-RPC. (Phase 5.) | ide | native |
| `gdscript-cli` | `check`/`lint`/`format`/`symbols` for CI. (Phase 5.) | ide | native |
| `xtask` | Build automation: `codegen-api`, `fixtures`, `dist`, `release` helpers, the local `ci` gate. | — | native |

## Dependency direction

```
base ◀── syntax ◀── db ◀── hir ◀── ide ◀── ffi ◀── (bindings: node, wasm)
  ▲         ▲       ▲       ▲       ▲  ◀── lsp
  └── api ──┘───────┘───────┘       └── cli
  base ◀── scene
```

A crate may only `use` crates to its left. Adding an upward edge is an
architectural change and should be questioned in review.

## Phase-0 state

Every crate currently exists as a **stub**: a `lib.rs`/`main.rs` that is (at
most) a doc comment plus a trivial smoke test, with the correct dependency edges
and metadata declared in `Cargo.toml`. They compile, lint, test, and (for the
core crates) pass the wasm portability check — but contain **no domain logic**.
Features arrive phase by phase per [`plans/ROADMAP.md`](../../../plans/ROADMAP.md).

## Publishing note

Internal crate names use the `gdscript-` prefix. The public Rust crate is
`gdscript-ide`. The npm scope is `@gdscript-analyzer/*`. Non-library crates
(`gdscript-ffi`, `gdscript-lsp`, `gdscript-cli`, the bindings) carry
`publish = false` until they are ready (Phase 5 for the binaries).
