# gdscript-analyzer

**A fast, embeddable, multi-target GDScript static-analysis library — Roslyn for Godot.**

[![crates.io](https://img.shields.io/crates/v/gdscript-ide.svg?logo=rust)](https://crates.io/crates/gdscript-ide)
[![docs.rs](https://img.shields.io/docsrs/gdscript-ide?logo=docsdotrs)](https://docs.rs/gdscript-ide)
[![CI](https://github.com/yanivkalfa/gdscript-analyzer/actions/workflows/ci.yml/badge.svg)](https://github.com/yanivkalfa/gdscript-analyzer/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@gdscript-analyzer/core?logo=npm)](https://www.npmjs.com/package/@gdscript-analyzer/core)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`gdscript-analyzer` parses and semantically understands **GDScript (Godot 4.x)**
and exposes an engine-independent query API — diagnostics, type-aware hover,
completion, go-to-definition, find-references, rename, document/workspace
symbols, signature help, and more — that any tool can embed: natively in Rust, in
**Node** via napi, or in the **browser** via WebAssembly.

It is, deliberately, the reusable *analysis brain* — separate from any one editor
or server, and it needs **no running Godot editor**. Think **rust-analyzer /
Ruff, for GDScript**.

**▶︎ Try it live in your browser — no install:** **[the playground](https://yanivkalfa.github.io/gdscript-analyzer/playground/)**

---

## What it is — and isn't

**It is:**

- A **library**, not an LSP server. It takes file contents and byte offsets and
  returns plain data structures (POD / JSON). It knows nothing about LSP,
  JSON-RPC, or any editor — clients map our neutral results to their own
  protocol. (A standalone LSP server is just *one* client of this library.)
- **Engine-neutral.** Results are offsets + POD. We never require a running Godot
  editor — the opposite of querying the engine's built-in LSP over TCP.
- **Multi-target.** One Rust core reaches native, Node (napi), and the browser
  (WASM) — same analysis, same results, everywhere.
- **Scene-aware.** It reads `.tscn`/`.tres` so `$Node`/`%Unique` paths and
  `get_node()` are typed against the actual scene tree — a Godot-specific feature
  the engine's own LSP doesn't fully provide.

**It isn't:**

- A Godot engine, runtime, or GDExtension binding — we do not *run* GDScript or
  talk to a live engine.
- A replacement for the Godot editor — we complement it.
- A GDScript 1.x (Godot 3) tool — **Godot 4.x / GDScript 2.0 only.**

See [`plans/00-VISION-AND-SCOPE.md`](plans/00-VISION-AND-SCOPE.md) for the full framing.

---

## Status — `0.1.0`, published and working

Everything below is **live**: `cargo add`-able, `npm i`-able, and exercised in
the [playground](https://yanivkalfa.github.io/gdscript-analyzer/playground/).

**Works today** (try the playground): headless GDScript analysis — **parse + type
diagnostics**, **type-aware hover** (gradual type inference), **completion**, plus
the full LSP-grade query surface (document/workspace symbols, go-to-definition,
find-references, rename, signature help, folding ranges, inlay hints, code
actions), and the **Godot 4.x engine model** (classes, methods, signals, enums)
loaded from `extension_api.json`.

**Growing across the `0.x` line:** the complete GDScript warning catalog, a
`gdformat`-compatible formatter, deeper control-flow narrowing, and performance
hardening for large projects. Track it in [`plans/ROADMAP.md`](plans/ROADMAP.md).

---

## Install & quickstart

### From the browser (WebAssembly) — `@gdscript-analyzer/wasm`

```sh
npm i @gdscript-analyzer/wasm
```

```js
import init, { Analyzer } from "@gdscript-analyzer/wasm";

await init();                              // load the .wasm
const az = new Analyzer();
az.openDocument("inmemory://main.gd", "extends Node\nfunc _ready():\n\tvar x = 5 / 2\n", null);

console.log(az.diagnostics("inmemory://main.gd")); // → JSON: INTEGER_DIVISION warning
```

Byte-offset ⇄ UTF-16 conversion is the page's job (the analyzer speaks UTF-8 byte
offsets); see [`playground/index.html`](playground/index.html) for a complete,
copy-pasteable example. Engine-class completion is enabled by loading the bundled
engine model — see the [wasm package README](bindings/wasm/README.md).

### From Node (napi) — `@gdscript-analyzer/core`

```sh
npm i @gdscript-analyzer/core
```

```js
import { AnalysisHandle } from "@gdscript-analyzer/core";

const az = new AnalysisHandle();
az.openDocument("inmemory://player.gd", "class_name Player extends Node\nvar hp := 100\n", "res://player.gd");

JSON.parse(az.documentSymbols("inmemory://player.gd")); // outline
JSON.parse(az.completions("inmemory://player.gd", 42));  // completions at byte 42
```

Native addon (no WASM overhead), with prebuilt binaries for macOS (x64/arm64),
Windows (x64), and Linux (x64/arm64). Ideal for LSP servers, CLIs, and editor
extensions. Full API + examples in the
[Node package README](bindings/node/README.md).

### From Rust — `gdscript-ide`

```sh
cargo add gdscript-ide
```

```rust
use gdscript_ide::AnalysisHost;
// AnalysisHost owns the files; a cheap, cloneable Analysis snapshot answers
// queries that return POD + byte offsets. See https://docs.rs/gdscript-ide
```

---

## The workspace — crates & packages

One Rust core, layered, with thin bindings on top:

```
gdscript-base     POD types (FileId, TextRange, LineIndex, results)
   └ gdscript-syntax   lexer + lossless (cstree) parser
        ├ gdscript-api   Godot engine model (from extension_api.json)
        ├ gdscript-scene .tscn/.tres parser → node-path typing
        └ gdscript-db    inputs / project model / salsa incremental layer
             └ gdscript-hir   name resolution + gradual type inference + warnings
                  └ gdscript-ide   ← the public API (AnalysisHost / Analysis)
                       ├ @gdscript-analyzer/core   Node addon (napi-rs)
                       └ @gdscript-analyzer/wasm    browser (wasm-bindgen)
```

| Surface | Install | Crate / package |
| --- | --- | --- |
| Rust | `cargo add gdscript-ide` | [`gdscript-ide`](https://crates.io/crates/gdscript-ide) (+ `-base`, `-syntax`, `-api`, `-db`, `-hir`, `-scene`) |
| Node | `npm i @gdscript-analyzer/core` | [`@gdscript-analyzer/core`](https://www.npmjs.com/package/@gdscript-analyzer/core) |
| Browser | `npm i @gdscript-analyzer/wasm` | [`@gdscript-analyzer/wasm`](https://www.npmjs.com/package/@gdscript-analyzer/wasm) |

All three surfaces share one URI-keyed session model: `openDocument` /
`changeDocument` / `closeDocument` (+ `setProjectConfig`), then query by **byte
offset**. Native/WASM queries return **JSON**; Rust returns POD.

---

## Godot version support

The engine knowledge (classes, methods, signals, enums, builtins) is sourced from
Godot's `extension_api.json` and class documentation, vendored per version and
kept in sync with Godot releases automatically.

- **First bundled version: Godot `4.5-stable`.**

See [`plans/GODOT-SYNC.md`](plans/GODOT-SYNC.md) for the multi-version policy.

---

## Documentation & contributing

- **Docs & guide** — [the mdBook](https://yanivkalfa.github.io/gdscript-analyzer/)
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
