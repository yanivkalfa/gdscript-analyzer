# ADR-0003: napi-rs v3 dual-target binding

- **Status:** Accepted
- **Date:** 2026-06-22

## Context

The Rust core must be consumable from **Node** (LSP servers, including guitkx's,
and CLIs that live in the JS ecosystem) and from the **browser** (web
playgrounds, in-page analysis). These are two different runtime targets with
different constraints:

- Node wants a **native addon** — full speed, no WASM overhead, free filesystem
  access on the host side.
- The browser needs **WebAssembly**, and ideally a small artifact that does not
  require `SharedArrayBuffer` / COOP-COEP headers (which many static hosts can't
  set).

The naive approach is to write and maintain **two separate bindings** (a napi
addon and a wasm-bindgen module), duplicating the FFI surface and its
serialization glue. That doubles maintenance and invites the two surfaces to
drift. See [`plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §4 and
the WASM/bindings research it cites.

A key enabling fact: **napi-rs v3 can compile the same binding source to both a
Node native `.node` addon and a `wasm32-wasip1-threads` target** — "you don't
need to write two different bindings." This collapses the duplication, at the
cost of pinning the MSRV to napi-rs v3's floor (**Rust 1.88.0**).

## Decision

**We will write one binding crate, `gdscript-ffi`, on napi-rs v3, and compile it
to both the Node `.node` addon and the `wasm32` target from a single source.
wasm-bindgen is kept as a documented, optional fallback — not the primary path.**

- **`gdscript-ffi` is the only crate with napi/wasm glue.** It holds a stateful
  `AnalysisHandle` (so the analysis cache survives edits across calls), exposes a
  small, flat surface (`applyChange`, per-feature queries, plus a stateless
  one-shot `analyze`), and passes **JSON POD by copy** across the boundary
  (`serde` / `serde-wasm-bindgen`). It never returns a whole AST per call — only
  the feature result.
- **Node** consumers get the native addon (`@gdscript-analyzer/core`) with
  per-platform prebuilt binaries via `optionalDependencies`.
- **Browser** consumers get the napi-rs wasm target as the **primary** route.
- **Fallback:** a dedicated `bindings/wasm` crate using **wasm-bindgen**
  (`wasm-pack build --target web`) is retained and documented, to be used if and
  when we want a smaller artifact with no SharedArrayBuffer requirement (Biome /
  Ruff's approach). The choice between the napi-wasm target and the
  wasm-bindgen fallback is made **per measured bundle size in Phase 5**.
- **MSRV is pinned to 1.88.0** — napi-rs v3's floor — for the whole workspace,
  and CI enforces it.

## Consequences

**Easier / positive.**

- One FFI surface to write, test, and evolve — Node and browser cannot drift
  apart because they are the same source.
- Native speed on Node (the guitkx / LSP path) with no WASM penalty.
- The stateful `AnalysisHandle` keeps the incremental cache alive across edits on
  both targets, which is what makes keystroke-latency analysis possible from JS.
- Keeping wasm-bindgen as a documented fallback means we are not locked in if the
  napi-wasm artifact turns out too large or too constrained for static hosting.

**Harder / negative — the constraints this creates.**

- **MSRV is dictated by the binding** (1.88.0). A core-crate dependency that
  raises *its* MSRV above ours fails the `msrv` CI job; MSRV bumps are
  deliberate and ADR-worthy.
- The napi cross-compile matrix (zig, musl, QEMU, per-platform packaging) is
  fragile toolchain-wise; we mitigate by wiring it in Phase 0 against an *empty*
  binding so failures are toolchain-only, not logic.
- Maintaining the wasm-bindgen fallback is a second (if dormant) path to keep
  compiling.
- The "strings and structs cross by copy" rule must be respected by every query
  — returning large payloads (e.g. a full AST) per call would be a performance
  cliff on the WASM boundary.
