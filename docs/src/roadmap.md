# Roadmap

The public, living view of where `gdscript-analyzer` is and where it's going. This is a
direction-of-travel document, not a dated commitment — issues labeled `C-tracking` hold the
fine-grained status. The detailed phase plans live in
[`plans/`](https://github.com/yanivkalfa/gdscript-analyzer/tree/master/plans).

## Where we are

The analyzer is a **headless, embeddable Rust library** (rust-analyzer's architecture, applied
to GDScript): a lossless CST, an incremental salsa query graph, gradual type inference with
flow-sensitive narrowing, scene (`.tscn`) awareness, and a Godot-matching warning set — all
behind one engine-/protocol-neutral `gdscript-ide` API, consumed by a CLI, an LSP server, a
napi binding, and a wasm playground.

- **Phases 1–5 — shipped** (`0.x` on crates.io + npm): lexer/parser, type inference + IDE
  features, cross-file + scene resolution, the CLI/LSP/napi/wasm clients, and the GA CI/CD +
  distribution pipeline.
- **Phase 6 — in progress** (the road to `1.0`): quality + stability + the API freeze.

## Phase 6 — the road to 1.0

| Workstream | What it delivers | Status |
|---|---|---|
| **W1 — warning set** | The full Godot warning catalog behind an emit-then-gate seam (`WarningCode` + `gate()`), project-setting gating, and `@warning_ignore[_start\|_restore]`. | Gate seam + a self-contained-check subset landed; the rest of the 48 codes are additive (tracked in `TECH_DEBT.md`). |
| **W2 — flow narrowing** | A real per-body control-flow dataflow: `is`/`!= null`/early-return/`and`-`or`/`else` narrowing that **beats the engine** on `is`-guards ([#93510](https://github.com/godotengine/godot/issues/93510)), plus reachability. | Landed (CFG + checker wiring + short-circuit + `UNREACHABLE_CODE`); match-arm narrowing + loop fixpoints deferred post-1.0. |
| **W3 — formatter** | A `gdscript-fmt` crate: `format` / `format_range` with idempotence + semantics-preservation, wired into the CLI and LSP. | In progress. |
| **W4 — performance** | A tiered perf fixture corpus + criterion benches (cold / warm-keystroke / completion), memory profiling, and CI size/latency guards. | In progress. |
| **W5 — docs** | A generated warning reference (from the `WarningCode` catalog), a Configuration page, the contract page, and CI-built examples. | In progress. |
| **W6 — API freeze** | `#[non_exhaustive]` on the consumer-matched types, a PR-time + release-time semver gate, and the published 1.0 contract. | Held for last (irreversible — see the execution overview). |
| **W7 — ecosystem** | Labels-as-code, issue forms, this roadmap, the [ADR](./adr/README.md) + RFC process ([ADR-0004](./adr/0004-rfc-graduation-trigger.md)). | Landed. |

The **1.0 cut line**: 1.0 is the *freeze*, not the feature-complete point. We ship Phase-6
improvements as `0.x` releases (each consumable + dogfoodable), and reserve the `1.0` tag for
when the public surface is stable — at which point `#[non_exhaustive]` and the semver policy
lock it.

## After 1.0 (direction, not commitment)

- **Narrowing precision** (the multi-year tail): loop-carried fixpoints, aliasing, narrowing
  through call results, enum/discriminant narrowing — each a MINOR/PATCH quality change, never
  an API break.
- **The remaining warning checks** + an opt-in Godot-differential CI harness.
- **Distribution reach**: the napi `musl`/`armv7`/WASI matrix, a wasm-size regression guard.
- **Editor polish** across the VS Code / Rider / Visual Studio extensions and the guitkx
  (ReactiveUI-Godot) consumer.

See [`TECH_DEBT.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/TECH_DEBT.md)
for the honest, itemized backlog.
