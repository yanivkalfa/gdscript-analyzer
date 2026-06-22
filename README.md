# gdscript-analyzer

**A fast, embeddable, multi-target GDScript static-analysis library — Roslyn for Godot.**

[![crates.io](https://img.shields.io/crates/v/gdscript-ide.svg?logo=rust)](https://crates.io/crates/gdscript-ide)
[![docs.rs](https://img.shields.io/docsrs/gdscript-ide?logo=docsdotrs)](https://docs.rs/gdscript-ide)
[![CI](https://github.com/yanivkalfa/gdscript-analyzer/actions/workflows/ci.yml/badge.svg)](https://github.com/yanivkalfa/gdscript-analyzer/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@gdscript-analyzer/core?logo=npm)](https://www.npmjs.com/package/@gdscript-analyzer/core)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`gdscript-analyzer` parses and semantically understands **GDScript (Godot 4.x)**
and exposes an engine-independent query API — completion, hover, diagnostics,
go-to-definition, find-references, rename, type inference, and more — that any
tool can embed: natively, in **Node** via napi, in the **browser** via
WebAssembly, or from other languages.

It is, deliberately, the reusable *analysis brain* — separate from any one editor
or server. Think **rust-analyzer / Ruff, for GDScript**.

---

## What it is — and isn't

**It is:**

- A **library**, not an LSP server. It takes file contents and byte offsets and
  returns plain data structures (POD). It knows nothing about LSP, JSON-RPC, or
  any editor — clients map our neutral results to their own protocol. (A
  standalone LSP server is just *one* client of this library.)
- **Engine-neutral.** Results are offsets + POD structs. We never require a
  running Godot editor — the opposite of querying the engine's built-in LSP over
  TCP.
- **Multi-target.** One Rust core reaches native, Node (napi), the browser
  (WASM), and — via C ABI / PyO3 — other languages.

**It isn't:**

- A Godot engine, runtime, or GDExtension binding — we do not *run* GDScript or
  talk to a live engine.
- A replacement for the Godot editor — we complement it.
- A GDScript 1.x (Godot 3) tool — **Godot 4.x / GDScript 2.0 only.**

See [`plans/00-VISION-AND-SCOPE.md`](plans/00-VISION-AND-SCOPE.md) for the full
framing.

---

## Status

> **Phase 0 — scaffolding.** The repository is a *runnable, releasable,
> contributable* workspace with **no analyzer features yet**: compiling crate
> stubs, the `xtask` build automation, CI, the release machinery, and the
> Godot engine-data pipeline. Analyzer features arrive in later phases.

Track progress in [`plans/ROADMAP.md`](plans/ROADMAP.md).

---

## Quickstart

> The crate and packages below are **not published yet** — they arrive over the
> `0.x` line. These snippets show the intended consumption surface.

### From Rust

```sh
cargo add gdscript-ide
```

```rust
// The public API crate is `gdscript-ide` (AnalysisHost / Analysis).
// Full usage examples land as the API surface fills in — see plans/ROADMAP.md.
use gdscript_ide as ide;
```

### From Node (napi)

```sh
npm i @gdscript-analyzer/core
```

```js
// Native addon via napi-rs — no WASM overhead. Powers LSP servers and CLIs.
import * as gdscript from "@gdscript-analyzer/core";
```

### From the browser (WebAssembly)

```js
// @gdscript-analyzer/wasm — coming in the 0.x line (not yet published).
import init, { /* analyze, … */ } from "@gdscript-analyzer/wasm";

await init();              // load the wasm module
// const result = analyze(source, { /* … */ });
```

---

## Godot version support

The engine knowledge (classes, methods, signals, enums, builtins) is sourced
from Godot's `extension_api.json` and class documentation, vendored per version
under `vendor/godot/<version>/` and kept in sync with Godot releases
automatically.

- **First bundled version: Godot `4.5-stable`.**

See [`plans/GODOT-SYNC.md`](plans/GODOT-SYNC.md) for the multi-version policy.

---

## Documentation & contributing

- **Architecture & design** — [`plans/01-ARCHITECTURE.md`](plans/01-ARCHITECTURE.md)
- **Roadmap** — [`plans/ROADMAP.md`](plans/ROADMAP.md)
- **Contributing** — [`CONTRIBUTING.md`](CONTRIBUTING.md) (build, test,
  `cargo xtask ci`, portability rules, Conventional-Commit PR titles, changesets)
- **Support** — [`SUPPORT.md`](SUPPORT.md) · **Security** — [`SECURITY.md`](SECURITY.md)
- **Governance** — [`GOVERNANCE.md`](GOVERNANCE.md) · **Code of Conduct** — [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)

Build and test the whole workspace, then run the full local gate:

```sh
cargo build --workspace
cargo test  --workspace
cargo xtask ci     # fmt + clippy -D + test + wasm-check + cargo deny
```

---

## License

Licensed under either of

- **MIT license** ([`LICENSE-MIT`](LICENSE-MIT)), or
- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))

at your option.

Third-party attributions are recorded in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.
