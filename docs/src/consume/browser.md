# Consuming from the Browser

> **Status:** the wasm binding is an empty stub in Phase 0; the real surface is
> wired in **Phase 1+**, and the published web playground lands in **Phase 5**
> ([`plans/ROADMAP.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/ROADMAP.md)).

For in-page analysis — web playgrounds, browser-based editors (Monaco /
CodeMirror), or any client that can't load a native addon — there is a
WebAssembly build:

```bash
npm i @gdscript-analyzer/wasm
```

## How the wasm build is produced

The **primary** route reuses the single `gdscript-ffi` binding: napi-rs v3
compiles the same source to a `wasm32-wasip1-threads` target. A **documented
fallback** is a dedicated wasm-bindgen package (`wasm-pack build --target web`),
kept for cases where we want a smaller artifact with no `SharedArrayBuffer` /
COOP-COEP requirement (the approach Biome and Ruff take). The choice between the
two is made per measured bundle size in Phase 5. See
[ADR-0003](../adr/0003-napi-rs-v3-dual-target.md) and
[`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md) §4.

## The shape across the boundary (forthcoming)

As in Node, the wasm module holds a stateful `AnalysisHandle` and returns
**JSON POD** per query. Strings and structs cross the boundary *by copy*
(`serde-wasm-bindgen`), so a query returns only its feature result — never a
full AST per call.

```js
// Illustrative — API lands in Phase 1+.
import init, { AnalysisHandle } from "@gdscript-analyzer/wasm";

await init(); // load + instantiate the .wasm module

const host = new AnalysisHandle();
host.applyChange({ fileId: 0, text: "extends Node\nfunc f(): pass\n" });
const diagnostics = host.diagnostics(0); // byte offsets in, POD JSON out
```

## Engine data is shipped separately

The Godot engine model (`extension_api.json`) is several megabytes. For the
browser it is pruned, converted to a zero-copy format (rkyv/postcard), brotli-
compressed, and fetched as a **separate content-hashed asset** — *not*
`include_bytes!`'d into the wasm module. That keeps the wasm artifact small and
lets the data update independently of the code.

## Portability is enforced from day one

The core crates must compile to `wasm32` — no `std::fs`, no
`Instant::now()`/`SystemTime::now()`, no threads in the hot path, and
`getrandom`'s JS backend only in the wasm binding. CI runs
`cargo check -p gdscript-ide --target wasm32-unknown-unknown` on every PR. The
full rules are in [`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md) §7.
