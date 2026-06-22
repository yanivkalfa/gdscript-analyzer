# Consuming from Node

> **Status:** the binding structure exists as an empty stub in Phase 0; the
> real surface is wired in **Phase 1+** ([`plans/ROADMAP.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/ROADMAP.md)).

Node consumers install the napi-rs native addon:

```bash
npm i @gdscript-analyzer/core
# pnpm add @gdscript-analyzer/core
```

The addon is a real **native `.node` binary** built with
[napi-rs v3](https://napi.rs) — no WASM overhead, full native speed. This is the
path that powers Node-based LSP servers, including guitkx's. Per-platform
prebuilt binaries (`@gdscript-analyzer/core-linux-x64-gnu`, `-darwin-arm64`,
`-win32-x64-msvc`, …) are pulled in automatically through
`optionalDependencies`, so there is no compile step for consumers.

## napi vs wasm

There is **one binding crate** — `gdscript-ffi` — and napi-rs v3 compiles it to
*both* a Node native addon and a `wasm32` target from the same source. For
Node you almost always want the **native addon** (this package): it is faster,
has no `SharedArrayBuffer`/COOP-COEP requirements, and reads files on the host
side. Reach for the [wasm package](./browser.md) only for the browser or for a
sandboxed/edge runtime where a native addon can't load. See
[ADR-0003](../adr/0003-napi-rs-v3-dual-target.md).

## The shape across the boundary (forthcoming)

The binding keeps a **stateful `AnalysisHandle`** alive inside Rust so the
analysis cache survives edits. The JS side pushes changes and runs queries;
results come back as **JSON POD** (serde). The surface is intentionally small
and flat — strings and structs cross the boundary *by copy*, so a query returns
only its feature result, never a whole AST.

```js
// Illustrative — API lands in Phase 1+.
import { AnalysisHandle } from "@gdscript-analyzer/core";

const host = new AnalysisHandle();
host.applyChange({
  fileId: 0,
  text: "extends Node\n\nfunc _ready() -> void:\n\tprint(1 + 1)\n",
});

// Byte offsets in; POD JSON out. The client maps byte offsets -> UTF-16.
const diagnostics = host.diagnostics(0);
const symbols = host.documentSymbols(0);
console.log(symbols);
```

A stateless one-shot `analyze(files) -> Report` is also provided for CLI/CI use
where you don't need an incremental cache.

## Position encoding (the footgun)

The core emits **byte offsets**. LSP uses **UTF-16** code units. The binding
glue ships a byte→UTF-16 converter (backed by `gdscript-base`'s `LineIndex`) —
do the conversion at the boundary, not in your application code. This is
discussed in [`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md) §4.
