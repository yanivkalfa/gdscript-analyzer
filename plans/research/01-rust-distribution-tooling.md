# Rust Distribution Tooling for `gdscript-analyzer`

**A "Roslyn for Godot": one Rust core, many consumers (Node LSP, browser playground, CLI, CI, other languages).**

Research note — compiled 2026-06-22. Every claim is cited inline with a URL. Versions are snapshots as of June 2026; the *structural* patterns (workspace shapes, per-platform packaging, napi/wasm split) are stable across releases unless noted.

---

## 0. TL;DR recommendation

Build a **single platform-agnostic core** (`crates/gdscript-*`) and ship it through **thin per-target wrapper crates**:

| Consumer | Binding crate | Tool | Artifact |
|---|---|---|---|
| Node LSP server (TS) | `crates/napi` | **napi-rs v3** + `@napi-rs/cli` | `.node` addon → npm `@gdscript/core` + per-platform `@gdscript/core-<triple>` optionalDependencies |
| Browser playground | reuse `crates/napi` → `wasm32-wasip1-threads` (napi-rs v3) **or** dedicated `crates/wasm` (wasm-bindgen) | napi-rs v3 / `wasm-pack` | `@gdscript/core-wasm32-wasi` **or** `@gdscript/wasm-{web,bundler,nodejs}` |
| CLI linter / CI | `crates/cli` (`clap` binary) | `dist` (cargo-dist) | cross-platform binaries + installers on GitHub Releases |
| Rust consumers | the core crates themselves | `release-plz` | crates.io publish |
| Other languages (later) | `crates/ffi` (cbindgen) / `crates/py` (PyO3) | cbindgen / maturin | C header + cdylib/staticlib / PyPI wheel |

This is, almost exactly, **oxc's** architecture. We copy oxc most heavily, borrow rust-analyzer's **library-layering discipline** and `ra_ap_*` crates.io trick, and borrow swc's/Biome's **per-platform loader** idiom.

---

## 1. The "Rust core, JS consumer" distribution pattern in the leading projects

There are **three distribution archetypes** across the five projects studied — the single most important framing for our own design:

1. **Prebuilt CLI binary + JS wrapper that `spawnSync`s it** (no in-process FFI) → **Biome**.
2. **napi-rs `.node` addon, runs in-process inside Node** → **swc, oxc**.
3. **maturin `bindings = "bin"` → a binary bundled in a Python wheel** (Python ecosystem, not napi) → **Ruff, ty**.

Plus two cross-cutting axes: how WASM is produced (wasm-bindgen vs napi-rs WASI vs wasm-pack), and whether the Rust crates are usable from crates.io.

### 1.1 Biome (biomejs/biome)

Repo: <https://github.com/biomejs/biome> — npm `@biomejs/biome` at **2.5.0**; crates.io lineage stale at **0.5.x** (March 2024).

- **Workspace:** root `Cargo.toml` `members = ["crates/*", "xtask/codegen", "xtask/coverage", "xtask/glue", "xtask/rules_check"]` (<https://raw.githubusercontent.com/biomejs/biome/main/Cargo.toml>); `crates/` holds ~96 `biome_*` crates (<https://github.com/biomejs/biome/tree/main/crates>). Key roles:
  - `biome_rowan` — Rowan fork; the lossless green-tree foundation under every parser/formatter (this is the same CST approach rust-analyzer pioneered).
  - Per-language stacks each split into `_syntax`/`_factory`/`_parser`/`_formatter`/`_analyze`/`_semantic`: `biome_js_*`, `biome_json_*`, `biome_css_*`, `biome_graphql_*`, `biome_html_*`, `biome_grit_*` (GritQL), etc.
  - `biome_analyze` (generic rule engine), `biome_formatter` (language-agnostic doc IR), `biome_service` (the orchestration hub / "workspace API"), `biome_cli` (the native binary), `biome_lsp` (LSP server), `biome_wasm` (the WASM build).
- **Node binding — NO napi-rs.** `@biomejs/biome` is a tiny JS wrapper that resolves a **prebuilt native CLI binary** and runs it with `child_process.spawnSync`, forwarding argv (<https://raw.githubusercontent.com/biomejs/biome/main/packages/%40biomejs/biome/bin/biome>). Docs confirm: "@biomejs/biome doesn't ship any binaries directly; the file @biomejs/biome/bin is just a tiny wrapper that delegates the operation to the real binary" (<https://biomejs.dev/guides/manual-installation/>).
- **npm per-platform optionalDependencies** (all `2.5.0`): `@biomejs/cli-{win32-x64,win32-arm64,darwin-x64,darwin-arm64,linux-x64,linux-arm64,linux-x64-musl,linux-arm64-musl}`; each platform package sets its own `os`/`cpu` (<https://raw.githubusercontent.com/biomejs/biome/main/packages/%40biomejs/biome/package.json>).
- **Browser:** a *separate* `biome_wasm` crate using **wasm-bindgen**, exposing a `Workspace` class + an in-memory `MemoryFileSystem` (no disk in WASM). Three env-specific packages: `@biomejs/wasm-bundler`, `@biomejs/wasm-nodejs`, `@biomejs/wasm-web`, wrapped by `@biomejs/js-api` (<https://deepwiki.com/biomejs/biome/7.2-javascript-api-and-wasm>).
- **crates.io:** published but neglected (`biome_js_parser` live at 0.5.7, 2024-03-12: <https://crates.io/crates/biome_js_parser>); maintainers explicitly deprioritize them (<https://github.com/biomejs/biome/discussions/7904>). Treat as best-effort.

### 1.2 swc (swc-project/swc)

Repo: <https://github.com/swc-project/swc> — `@swc/core` at **1.15.43**.

- **napi-rs confirmed.** Binding crate `bindings/binding_core_node`; `packages/core/package.json` build script is `napi build --manifest-path ../../Cargo.toml --platform -p binding_core_node ...` (<https://raw.githubusercontent.com/swc-project/swc/main/packages/core/package.json>). The crate is `crate-type = ["cdylib"]`, depends on `napi`/`napi-derive`/`napi-build` and on `swc_core` (<https://raw.githubusercontent.com/swc-project/swc/main/bindings/binding_core_node/Cargo.toml>).
- **napi v3 config:** `"napi": { "binaryName": "swc", "targets": [ ...12 triples... ] }`. Generates one `@swc/core-<platform>` package per triple.
- **The loader** (`packages/core/binding.js`) is the reference per-platform resolver: an `isMusl()` helper + a `process.platform × process.arch` switch that tries a local `./swc.<platform>.node`, then `require('@swc/core-<platform>')`, then a `@swc/core-wasm32-wasi` WASI fallback, throwing `Failed to load native binding` on total failure (<https://raw.githubusercontent.com/swc-project/swc/main/packages/core/binding.js>).
- **12 optionalDependencies** spanning darwin x64/arm64, win32 x64/ia32/arm64-msvc, linux x64/arm64 gnu+musl, arm-gnueabihf, ppc64, s390x (<https://registry.npmjs.org/@swc/core/latest>).
- **Workspace:** `members = ["xtask", "bindings/*", "crates/*", "tools/generate-code", "tools/swc-releaser"]` (<https://raw.githubusercontent.com/swc-project/swc/main/Cargo.toml>). `crates/*` libs **are published & independently versioned** — note they do *not* share the npm 1.x line: `swc_ecma_parser` 41.1.2, `swc_common` 23.0.2, `swc_core` 71.0.3 (<https://crates.io/api/v1/crates/swc_core>). `bindings/*` are not published.
- **Why swc chose napi-rs:** migration from Neon for "minimal overhead" + "stable native API," with a maintainer benchmark showing **~2.5×** throughput (5,538 vs 2,177 ops/sec on an ES2018 transform) (<https://github.com/swc-project/swc/issues/852>).

### 1.3 oxc (oxc-project/oxc) — the closest template for us

Repo: <https://github.com/oxc-project/oxc> — workspace **0.137.0**, oxlint **1.71.0**.

- **Workspace:** `members = ["apps/*", "crates/*", "napi/*", "tasks/*"]` (<https://raw.githubusercontent.com/oxc-project/oxc/main/Cargo.toml>); 35 crates under `crates/` (<https://github.com/oxc-project/oxc/tree/main/crates>). Key crates: `oxc` (umbrella, feature-gated), `oxc_parser`, `oxc_ast`, `oxc_semantic` (scope/symbol resolution — our analog), `oxc_span`, `oxc_allocator` (arena backing the AST), `oxc_diagnostics`, `oxc_codegen`, `oxc_linter` (the oxlint engine), `oxc_formatter`, `oxc_language_server`, `oxc_napi` (shared NAPI glue).
- **Thin napi bindings** in `napi/{parser,transform,minify,playground}`. **Verified directly** from `napi/parser/Cargo.toml`:
  - `[lib] crate-type = ["cdylib", "lib"]` — `cdylib` to make the loadable addon, `lib` so the crate is *also* a normal Rust lib.
  - Core pulled in via workspace inheritance: `oxc = { workspace = true, features = ["ast_visit", "regular_expression", "semantic", "serialize"] }` — **the binding selects core feature flags; it holds almost no logic.**
  - `napi = { workspace = true, features = ["async"] }`, `napi-derive = { workspace = true }`, `[build-dependencies] napi-build = { workspace = true }`.
  - Native allocator gated out of core: `mimalloc-safe` under `[target.'cfg(...)']` per-OS/arch (macOS; Linux non-ARM; Linux arm64) — core never sees it.
  (Verified 2026-06-22 against <https://raw.githubusercontent.com/oxc-project/oxc/main/napi/parser/Cargo.toml>.)
- **npm:** `oxc-parser` ships 20 `@oxc-parser/binding-<platform>` optionalDependencies (all `0.137.0`) including `@oxc-parser/binding-wasm32-wasi` (<https://registry.npmjs.org/oxc-parser/latest>). oxlint ships as npm `oxlint` with ~19 `@oxlint/binding-*` packages — same pattern.
- **WASM — the key modernization.** The *legacy* `@oxc-parser/wasm` (wasm-pack `--target web/bundler/nodejs`) is **deprecated and removed** — `wasm/` 404s (<https://github.com/oxc-project/oxc/issues/10778>). The *modern* path: the **same** `napi/*` crate is compiled to `wasm32-wasip1-threads`, so napi-rs emits `@oxc-parser/binding-wasm32-wasi` from the identical wrapper — **one binding, both `.node` and browser WASM** (<https://github.com/oxc-project/oxc/issues/21038>, <https://napi.rs/docs/concepts/webassembly>).
- **crates.io split:** reusable libs published (`oxc`, `oxc_parser`, `oxc_semantic`, …, all 0.137.0); **`oxc_linter` is NOT published** (404), and the napi wrappers + `oxlint-app` are `publish = false`. Deliberate: libs → crates.io, end-user tools → npm only.

### 1.4 Ruff & ty (astral-sh/ruff, astral-sh/ty)

Repo: <https://github.com/astral-sh/ruff> — Ruff **0.15.18**, ty **0.0.51**. Both the linter and type-checker crates live in the one monorepo; `astral-sh/ty` is a thin hub that includes ruff as a git submodule.

- **Workspace:** root `members = ["crates/*"]`, edition 2024 (<https://raw.githubusercontent.com/astral-sh/ruff/main/Cargo.toml>). `crates/` holds both families: Ruff (`ruff` binary, `ruff_linter`, `ruff_python_parser`, `ruff_python_ast`, `ruff_python_formatter`, `ruff_server` LSP, `ruff_wasm`, `ruff_db`, `ruff_python_semantic`, …) and ty (`ty` binary, `ty_python_semantic` type inference, `ty_ide`, `ty_server`, `ty_project`, `ty_vendored` typeshed, `ty_wasm`). Historically `red_knot_*`, now fully renamed `ty_*`.
- **PyPI via maturin, `bindings = "bin"` (NOT pyo3/abi3).** **Verified directly:** root `pyproject.toml` has `build-backend = "maturin"`, `[tool.maturin] bindings = "bin"`, `manifest-path = "crates/ruff/Cargo.toml"` (<https://raw.githubusercontent.com/astral-sh/ruff/main/pyproject.toml>, verified 2026-06-22). Because it's a bundled **binary** (not a C-extension), wheels are tagged `py3-none-<platform>` — Python-version- and ABI-agnostic, platform-specific (no abi3). PyPI shows 0.15.18 wheels for Windows/macOS + many Linux/musllinux arches (<https://pypi.org/project/ruff/>).
- **WASM playground:** `ruff_wasm` (`publish = false`, `crate-type = ["cdylib","rlib"]`, **wasm-bindgen**) compiles the *same* `ruff_linter`/`ruff_python_*` core to `.wasm`, built with **wasm-pack** for three targets and published as `@astral-sh/ruff-wasm-{web,bundler,nodejs}` (all 0.15.18) (<https://raw.githubusercontent.com/astral-sh/ruff/main/crates/ruff_wasm/Cargo.toml>, <https://registry.npmjs.org/@astral-sh/ruff-wasm-web>). Playground is React + Vite + Monaco (<https://raw.githubusercontent.com/astral-sh/ruff/main/playground/README.md>).
- **ty packaging:** same maturin `bindings = "bin"` (manifest pointing into the ruff submodule); distributed via **PyPI / standalone installer / GitHub Releases / Docker — no npm channel** (<https://docs.astral.sh/ty/installation/>). `ty_wasm` exists but is **not** published to npm.
- **crates.io:** library/wasm crates `publish = false` (`ruff_linter` 404s); the `ruff` name on crates.io is an unrelated 0.0.1 placeholder — so Ruff is effectively **not** `cargo install`-able.

> **Note for us:** Ruff's PyPI model is irrelevant to a Node-first analyzer *except* as the canonical proof that maturin + a Rust binary is the right Python path **if** we ever want `pip install gdscript-analyzer`. Its WASM-playground model (separate wasm-bindgen crate, three wasm-pack targets) is directly relevant.

### 1.5 rust-analyzer (rust-lang/rust-analyzer) — the library-layering exemplar

Repo: <https://github.com/rust-lang/rust-analyzer>. Even though it *ships* as an LSP server, it is structured as **a stack of reusable libraries** — this is the single best precedent for "Roslyn for Godot."

- **36 crates** with explicit **API Boundaries** (<https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/docs/book/src/contributing/architecture.md>):
  - `parser` + `syntax` — hand-written recursive-descent parser; lossless CST; *"completely independent from the rest of rust-analyzer. It knows nothing about salsa or LSP."*
  - `base-db` — salsa infrastructure + input queries; deliberately knows nothing about cargo.
  - `hir-expand` / `hir-def` / `hir-ty` — macro expansion, name resolution, type inference ("the brain"); invariant: *"typing inside a function's body never invalidates global derived data."*
  - `hir` — the **façade** (an API Boundary): *"the façade you'll be talking to"* to use ra as a library.
  - `ide` (+ `ide-db`, `ide-assists`, `ide-completion`, `ide-diagnostics`, `ide-ssr`) — high-level features as a **Rust API**, not LSP: *"`ide` uses Rust API and is intended for use by various tools."*
  - `rust-analyzer` — *"the only crate that knows about LSP and JSON serialization."*
  - Flow: `syntax/parser → base-db → hir-* → hir → ide-db → ide-* → ide → rust-analyzer`; everything below the top crate is LSP-free and reusable.
- **The snapshot query model:** `AnalysisHost` is mutable state you `apply_change` into; `Analysis` is an immutable **salsa** snapshot you run semantic queries (completions, goto-def, hover, references) against — incremental, memoized, selectively invalidated. **This is the IDE-query-API shape we want.**
- **WASM — library yes, binary no.** CI runs `cargo check --target=wasm32-unknown-unknown -p ide` with the literal comment *"The rust-analyzer binary is not expected to compile on WASM, but the IDE crate should"* (<https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/.github/workflows/ci.yaml>). So the `ide` library is **CI-enforced wasm-buildable**; the LSP binary is not; there is **no official browser product** (the old `rust-analyzer-wasm` demo was archived 2025-05-29).
- **The `ra_ap_*` crates.io trick:** internal crates are `publish = false` under their real names, but the whole workspace is auto-published under an `ra_ap_` prefix at synthetic `0.0.$N` versions via a weekly `autopublish.yaml` running `cargo workspaces publish` (live at v0.0.338, 2026-06-22: <https://crates.io/crates/ra_ap_ide>). This is the official "use ra as a library" path. **We can do the same for early consumers without committing to stable public versions.**

### 1.6 Cross-project comparison

| | Workspace shape | JS/Py artifact | In-process? | Per-platform model | WASM | Rust libs on crates.io |
|---|---|---|---|---|---|---|
| **Biome** | `crates/*` (~96) | prebuilt CLI binary + JS `spawnSync` | No | `@biomejs/cli-<platform>` optDeps | `biome_wasm` (wasm-bindgen), 3 pkgs | published but stale (0.5.x) |
| **swc** | `crates/*` + `bindings/*` | napi-rs `.node` | Yes | `@swc/core-<platform>` optDeps | WASI fallback via napi | published, independently versioned |
| **oxc** | `apps/* crates/* napi/* tasks/*` | napi-rs `.node` | Yes | `@oxc-parser/binding-<platform>` optDeps | **same napi crate → `wasm32-wasip1-threads`** (old wasm-pack pkg deprecated) | libs yes; `oxc_linter`/oxlint no |
| **Ruff/ty** | `crates/*` (ruff_* + ty_*) | maturin `bindings = "bin"` → binary-in-wheel | No (binary) | `py3-none-<platform>` PyPI wheels | `ruff_wasm` (wasm-bindgen + wasm-pack) → `@astral-sh/ruff-wasm-{web,bundler,nodejs}` | all `publish=false`; `ruff` is a placeholder |
| **rust-analyzer** | `crates/*` (36) | (n/a — Rust/LSP) | — | rustup + GH-release binaries + VS Code bundle | `ide` crate CI-checked on wasm; no binary/browser product | auto-published under `ra_ap_*` prefix |

**Patterns worth copying:** (1) the per-platform `optionalDependencies` + tiny resolver loader (musl detection, platform×arch switch, local-then-package require, throw on failure) — swc's `binding.js` is the reference; (2) core logic in `crates/*`, thin per-target wrappers elsewhere — oxc's single `napi/parser` → both `.node` and `wasm32-wasi` is the cleanest; (3) deliberately split crates.io publishing (publish libraries, mark tool/binding crates `publish = false`) — oxc and rust-analyzer's `ra_ap_*` do this cleanly.

---

## 2. napi-rs — Node native addons in Rust

**Definition:** "A framework for building compiled Node.js add-ons in Rust via Node-API" (<https://github.com/napi-rs/napi-rs>). Current major line **v3** (config uses `binaryName`/`targets`); legacy **v2** used `name`/`triples`.

### 2.1 How it works
napi-rs targets **Node-API (N-API)**, which shipped in **Node.js v8.0.0** and is **ABI-stable across Node versions** — "different versions of Node.js use the same interface, which is stably ABI-compatible," shifting the model from "write once, compile everywhere" to **"write once, compile once"** (<https://napi.rs/docs/deep-dive/history>). Node's own docs: Node-API "is independent from the underlying JavaScript runtime … and will be Application Binary Interface (ABI) stable across versions of Node.js" (<https://nodejs.org/api/n-api.html>). Output is a standard **`.node` native addon** you `require()` — built "purely with the Rust/JavaScript toolchain and without involving node-gyp." Version floor: Node-API 1 ⇒ Node 8.0.0; async/Promises ⇒ Node 10.6.0; BigInt ⇒ Node 10.7.0. MSRV **Rust 1.88.0** (<https://github.com/napi-rs/napi-rs>).

### 2.2 The `#[napi]` macro + auto `.d.ts`
`#[napi]` "automatically generate[s] module registering code" (<https://napi.rs/docs/concepts/exports>). Building emits, alongside the `.node`, an `index.js` loader and an `index.d.ts` (<https://napi.rs/docs/cli/build>). Minimal example:
```rust
use napi_derive::napi;
#[napi]
pub fn sum(a: i32, b: i32) -> i32 { a + b }
```
→ generated `index.d.ts`: `export function sum(a: number, b: number): number` (<https://napi.rs/docs/introduction/simple-package>). Classes via `#[napi(constructor)]` on a struct + `#[napi]` on the `impl`; enums via `#[napi] pub enum`.

### 2.3 The per-platform prebuilt-binary npm model
Two tiers (<https://napi.rs/docs/deep-dive/release>):
- A **main package** = JS-only wrapper (`index.js` loader + `.d.ts`) declaring each platform package as an `optionalDependency`.
- **Per-platform packages** = one `.node` each, named `@scope/pkg-${triple}` (`linux-x64-gnu`, `darwin-arm64`, `win32-x64-msvc`, …), each carrying its own `os`/`cpu` so npm installs only the match.

Main `package.json`:
```json
{
  "name": "@gdscript/core",
  "optionalDependencies": {
    "@gdscript/core-linux-x64-gnu": "1.0.0",
    "@gdscript/core-darwin-arm64": "1.0.0",
    "@gdscript/core-win32-x64-msvc": "1.0.0"
  }
}
```
The auto-generated loader reads `process.platform`/`process.arch`, then tries (1) a local `.node` (dev), (2) `@scope/pkg-{triple}`, (3) a WASI fallback. On Linux it distinguishes **gnu vs musl** with an `isMusl()` function using three strategies (`/usr/bin/ldd` read, `process.report.header.glibcVersionRuntime` absence, `ldd --version`) (<https://github.com/napi-rs/package-template/blob/main/index.js>).

### 2.4 `@napi-rs/cli` build/publish flow
- `napi new` — scaffold (prompts for name, targets, optional CI).
- `napi build --platform --release [--target <triple>]` — compile; `--platform` embeds the triple in the filename; `--strip/-s` minimizes size; flags after `--` pass to cargo (<https://napi.rs/docs/cli/build>).
- `napi artifacts` — "Copy artifacts from GitHub Actions into npm packages and ready to publish"; organizes downloaded `.node`s into `npm/<triple>/` dirs (<https://napi.rs/docs/cli/artifacts>).
- `napi create-npm-dirs` — generate the per-platform package dirs.
- `napi prepublish -t npm` — patch the main package's `optionalDependencies`, publish the sub-packages, (by default) cut a GitHub release; wired into `prepublishOnly` (<https://napi.rs/docs/cli/pre-publish>).
- **Config (v3):** `"napi": { "binaryName": "...", "targets": [ ...triples... ], "packageName": "...", "npmClient": "npm" }` (<https://napi.rs/docs/cli/napi-config>). (v2 used `name` + `triples.{defaults,addition}` — <https://napi.rs/en/docs/more/v2-v3-migration-guide.en>.)

### 2.5 CI cross-compile matrix (GitHub Actions)
napi-rs cross-compiles many targets from few runners; `@napi-rs/cli` picks toolchains and injects env, integrating **`cargo-zigbuild`** (musl/Linux, `-x` flag), **`cargo-xwin`** (Windows cross), and **`@napi-rs/cross-toolchain`** (`--use-napi-cross` for glibc) (<https://napi.rs/docs/cross-build.en>, <https://napi.rs/blog/announce-v3>). Real production matrix (oxc-resolver `release-napi.yml`, <https://github.com/oxc-project/oxc-resolver/blob/main/.github/workflows/release-napi.yml>):
```yaml
- { os: windows-latest, target: x86_64-pc-windows-msvc,        build: pnpm build }
- { os: windows-latest, target: aarch64-pc-windows-msvc,       build: pnpm build }
- { os: ubuntu-latest,  target: x86_64-unknown-linux-gnu,      build: pnpm build --use-napi-cross }
- { os: ubuntu-latest,  target: x86_64-unknown-linux-musl,     build: pnpm build -x }          # cargo-zigbuild
- { os: ubuntu-latest,  target: aarch64-unknown-linux-gnu,     build: pnpm build --use-napi-cross }
- { os: ubuntu-latest,  target: aarch64-unknown-linux-musl,    build: pnpm build -x }
- { os: ubuntu-latest,  target: armv7-unknown-linux-gnueabihf, build: pnpm build --use-napi-cross }
- { os: macos-latest,   target: x86_64-apple-darwin,           build: pnpm build }
- { os: macos-latest,   target: aarch64-apple-darwin,          build: pnpm build }
- { os: ubuntu-latest,  target: wasm32-wasip1-threads,         build: pnpm run build:debug --release }
```
Each job uploads `napi/*.node` as `bindings-${target}`; a publish job downloads all, runs `napi artifacts`, and `npm publish napi/ --provenance`. Runner→triple summary: `windows-latest` → msvc x64/i686/arm64; `macos-latest` → darwin x64/arm64; `ubuntu-latest` → all linux gnu/musl/arm/android + wasm; FreeBSD via `cross-platform-actions/action`.

### 2.6 Limitations
Native (not browser-runnable as a `.node`); needs a prebuilt binary **per platform** (or source-compile/WASI fallback); the **gnu/musl** split doubles the Linux matrix and needs runtime libc detection; binary size (hence `--strip`); Node-only with version floors; meaningful CI/toolchain complexity vs a pure-JS package (<https://napi.rs/docs/deep-dive/release>, <https://github.com/napi-rs/napi-rs>).

---

## 3. wasm-bindgen / wasm-pack / wasm32

### 3.1 What they are
**wasm-bindgen** layers a richer ABI over raw WASM (which can only exchange i32/i64/f32/f64) so you can pass strings, structs, closures, classes, and it auto-generates JS glue + `.d.ts` (<https://rustwasm.github.io/docs/wasm-bindgen/>; canonical docs now at <https://wasm-bindgen.github.io/>). The `#[wasm_bindgen]` macro exports Rust items to JS or imports JS APIs into Rust. **wasm-pack** orchestrates the build: compiles to WASM, runs wasm-bindgen, optionally runs `wasm-opt`, and assembles a `pkg/` (the `.wasm`, a JS wrapper, `.d.ts`, a generated `package.json`) (<https://rustwasm.github.io/docs/wasm-pack/commands/build.html>).

### 3.2 `wasm-pack build --target {web,nodejs,bundler,no-modules}`
(<https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html>)
- **`bundler`** (default) — ESM for webpack/rollup/Parcel/Vite; wired via `module` key, `sideEffects:false`.
- **`web`** — native ESM, no bundler, **but you must `await init()`** to load the `.wasm`; ideal for `<script type="module">`. (Also the target `wasm-bindgen-rayon` requires.)
- **`nodejs`** — CommonJS `require`; `.wasm` loaded synchronously from disk; wired via `main`.
- **`no-modules`** — legacy plain-JS, single `<script>`, limited feature subset.

Profiles: `--dev`/`--profiling`/`--release`. Publish: `wasm-pack pack` (tarball) and `wasm-pack publish [--tag <tag>]` wrap `npm pack`/`npm publish` (<https://rustwasm.github.io/docs/wasm-pack/commands/pack-and-publish.html>).

### 3.3 Crossing the JS↔WASM boundary
- **Numbers** map to WASM value types — ~free.
- **Strings are COPIED** in both directions: passing `&str`/returning `String` copies UTF-8 bytes through linear memory as a `(ptr,len)` pair (<https://github.com/rustwasm/wasm-bindgen/issues/2741>, <https://wasmbyexample.dev/examples/passing-high-level-data-types-with-wasm-bindgen/passing-high-level-data-types-with-wasm-bindgen.rust.en-us>).
- **Structured data via `serde-wasm-bindgen`** — the **officially preferred** Serde integration (superseded `JsValue::from_serde`/`into_serde`, which went through `serde_json`). Maps structs→JS objects, `Vec`→arrays, `HashMap`→`Map`; "much smaller code size overhead than JSON, and … much faster" because it builds JS values directly (<https://github.com/RReverser/serde-wasm-bindgen>).
- **Cost is O(data size)** even with serde-wasm-bindgen — design the API to cross the boundary *rarely* and operate on shared linear-memory buffers for large payloads (relevant: returning a full AST/diagnostics list is exactly the expensive case).

### 3.4 Size optimization
```toml
[profile.release]
opt-level = "z"      # also try "s" — sometimes smaller; "Always measure!"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```
Then **`wasm-opt -Oz`** (Binaryen) — WASM-specific passes ("minification for WebAssembly"), typically another **15–20%** off; **wasm-pack runs it automatically** (<https://rustwasm.github.io/docs/book/reference/code-size.html>, <https://github.com/WebAssembly/binaryen>). Profile size with **`twiggy top -n 20 pkg/foo_bg.wasm`** (also `dominators`/`paths`/`monos`/`garbage`) (<https://rustwasm.github.io/twiggy/>). **`wee_alloc` is unmaintained (RUSTSEC-2022-0054, 2022-09-08) — avoid it**; use the default allocator or `lol_alloc` (<https://rustsec.org/advisories/RUSTSEC-2022-0054.html>).

### 3.5 Threading / atomics
WASM threads need: Web Workers + shared memory (`SharedArrayBuffer`) + WASM atomics (atomics + bulk-memory proposals) + **COOP/COEP cross-origin-isolation headers** (`Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Embedder-Policy: require-corp`) (<https://web.dev/articles/webassembly-threads>). **`wasm-bindgen-rayon`** brings Rayon to the web but requires **nightly Rust**, `-Z build-std=panic_abort,std`, `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory -C link-arg=--max-memory=…"`, `--target web`, and a runtime `await initThreadPool(...)` (<https://github.com/RReverser/wasm-bindgen-rayon>). Treat browser threading as **advanced/experimental**.

### 3.6 wasm32 target selection
- **`wasm32-unknown-unknown`** — the **browser/JS** target (what wasm-bindgen/wasm-pack use); `std` heavily stubbed: `println!` no-ops, `std::fs` errors, `std::thread::spawn` panics (<https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html>).
- **WASI** — for **server/standalone runtimes** needing real `std`. `wasm32-wasi` was **removed from stable in Rust 1.84 (2025-01-09)**; use **`wasm32-wasip1`**/`wasm32-wasip2`, and **`wasm32-wasip1-threads`** for threads (<https://blog.rust-lang.org/2024/04/09/updates-to-rusts-wasi-targets/>). napi-rs v3's browser story uses `wasm32-wasip1-threads` precisely because threads/`tokio` work there.

---

## 4. napi vs WASM — the trade-off, and the recommendation

### 4.1 The evidence
- **Native napi is ~1.3×–2.5× faster** than the same Rust compiled to WASM: Fibonacci microbench native ~2.27s vs WASM ~3.29s (<https://yieldcode.blog/post/native-rust-wasm/>); a heavier zip/parse/deserialize workload measured native **1.75×–2.5×** faster (<https://nickb.dev/blog/wasm-and-native-node-module-performance-comparison/>); broader benches put WASM overhead at ~1.3× typical, up to ~2.5×, with the wasm-bindgen "convenience layer add[ing] slight overhead for data marshalling" (<https://byteiota.com/rust-webassembly-performance-8-10x-faster-2025-benchmarks/>).
- **napi wins** on raw speed, full CPU/threads/SIMD, low marshaling cost for large ASTs/diagnostics, and N-API ABI stability (one prebuilt binary per platform, no recompilation). **It loses** on portability — Node-only, requires per-platform binaries.
- **WASM wins** on portability (browser, any JS runtime, sandboxed, single artifact, no per-platform binaries). **It loses** on speed and marshaling, has limited threading, and "communication overhead kills gains for DOM-heavy operations" (<https://karnwong.me/posts/2024/12/native-implementation-vs-wasm-for-go-python-and-rust-benchmark/>).
- **All three reference tools do both:** native for the CLI/Node path, WASM for the browser playground (Biome `biome_wasm` wasm-bindgen; oxc napi→wasm32-wasi; Ruff `ruff_wasm` wasm-bindgen) (<https://deepwiki.com/biomejs/biome/7.2-javascript-api-and-wasm>, <https://napi.rs/docs/concepts/webassembly>).

### 4.2 The 2025–2026 modernization (verified)
napi-rs **v3** lets the **same** napi binding compile to WASM — *"you don't need to write 2 different bindings for the same project"* — targeting **`wasm32-wasip1-threads`** so `std::thread`/`tokio` run in the browser unmodified; the browser Node-API runtime is **emnapi**. The explicit motivation was oxc's pain: oxc *"maintained a `wasm-bindgen` binding before. However, as the project grew larger … the maintenance cost became higher and higher"* (<https://napi.rs/blog/announce-v3>, quotes verified 2026-06-22). Trade-off: `wasm32-wasip1-threads` artifacts are **larger** and need the emnapi runtime + SharedArrayBuffer (cross-origin isolation) in the browser — napi-rs accepts this because playground/StackBlitz users "are not very sensitive to bundle size" (<https://napi.rs/docs/concepts/webassembly>).

### 4.3 Recommendation for gdscript-analyzer
- **Node / LSP → native napi-rs addon.** Fastest, lowest marshaling cost for large semantic payloads, ABI-stable prebuilt binaries.
- **Browser playground → WASM.** Two viable routes:
  - **(A) napi-rs v3 → `wasm32-wasip1-threads`** (oxc's current approach): one binding to maintain, threads work; larger artifact + emnapi + COOP/COEP. **Recommended default** to avoid dual-binding maintenance.
  - **(B) a dedicated `crates/wasm` (wasm-bindgen + wasm-pack)** (Biome/Ruff approach): smaller artifact, mature, no SharedArrayBuffer requirement; but a second binding surface to keep in sync. **Pick this if playground bundle size or single-thread simplicity matters more than dedup.**
- **Rust consumers → the plain core crates.** No binding overhead at all.

---

## 5. Single core, multiple thin bindings (workspace layout)

### 5.1 The layout (verified against oxc)
```
gdscript-analyzer/
├── Cargo.toml                 # [workspace], [workspace.dependencies] = single source of truth
├── crates/
│   ├── gdscript_syntax/       # lossless CST / parser — knows nothing about LSP or platform
│   ├── gdscript_db/           # salsa input layer (rust-analyzer base-db analog)
│   ├── gdscript_hir/          # name resolution / semantic model
│   ├── gdscript_ide/          # IDE query API (completions, goto-def, hover) — Rust API, LSP-free
│   ├── gdscript_linter/       # rule engine
│   ├── napi/                  # crate-type=["cdylib","lib"]; napi-rs binding; depends on core via workspace
│   ├── wasm/                  # (optional route B) wasm-bindgen binding
│   ├── cli/                   # clap binary; real fs/threads/time live HERE, not in core
│   ├── ffi/                   # (later) cbindgen cdylib/staticlib
│   └── py/                    # (later) PyO3 + maturin
```
This mirrors oxc exactly: core in `crates/*`, thin wrappers select **core feature flags** and hold almost no logic; `[lib] crate-type = ["cdylib", "lib"]`; deps via `{ workspace = true }`; native-only deps (e.g. mimalloc) gated behind `[target.'cfg(...)']` so core never sees them (verified §1.3). Centralize **all** versions in `[workspace.dependencies]` (oxc uses resolver v3 + centralized deps — <https://deepwiki.com/oxc-project/oxc/8.1-build-system-and-workspace>). Borrow rust-analyzer's **layering discipline**: explicit API boundaries at the `hir` façade and the `ide` query crate, and the `AnalysisHost`/`Analysis` snapshot model (§1.5).

### 5.2 wasm32 pitfalls — keep these OUT of `crates/core`
On `wasm32-unknown-unknown` the std lib is heavily restricted (<https://rustwasm.github.io/docs/book/reference/which-crates-work-with-wasm.html>):
- **No filesystem** — crates assuming `std::fs` fail. Provide an in-memory FS in the wasm binding (Biome's `MemoryFileSystem` pattern).
- **`std::thread::spawn` panics (wasm trap)** — this is *why* napi-rs v3 chose `wasm32-wasip1-threads`, where threads work.
- **No system/C libraries.**
- **`std::time` panics:** `Instant::now()`/`SystemTime::now()` panic on `wasm32-unknown-unknown`; use the `web-time` crate as a drop-in (<https://docs.rs/web-time/latest/web_time/>).
- **RNG / `getrandom`** (a common transitive break via `rand`/`uuid`/`ahash`/`hashbrown`): on wasm you must enable a JS backend — feature **`js`** in getrandom **0.2** (`features = ["js"]`), renamed to **`wasm_js`** in **0.3+** (plus `RUSTFLAGS='--cfg getrandom_backend="wasm_js"'`). The docs **warn against enabling it in libraries** because it "break[s] non-Web WASM builds" and bloats `Cargo.lock` (<https://docs.rs/getrandom/latest/getrandom/>, <https://docs.rs/getrandom/0.2.15/getrandom/>). **Enable it only in the wasm binding crate, never in `crates/core`.**
- **No sockets/`tokio`/`mio` I/O** on `wasm32-unknown-unknown`; `wasm32-wasip1-threads` mitigates (napi-rs v3 claims tokio works there).
- Add **`console_error_panic_hook`** in the wasm binding so panics reach `console.error` (<https://rustwasm.github.io/book/reference/crates.html>).

**Structuring rules:** `crates/core` = pure logic, no `std::fs`, no `SystemTime::now()` (use `web-time`), no thread-spawn in the hot path, no getrandom js feature — parsers/analyzers fit the rustwasm "works off-the-shelf" categories. Each binding opts into its platform features; the CLI is where real fs/threads/time live.

---

## 6. C ABI / FFI and other-language reach (brief)

### 6.1 cbindgen → C header (any-language-with-C-FFI reach)
**cbindgen** "creates C/C++11 headers for Rust libraries which expose a public C API" by parsing your Rust source AST, finding `#[no_mangle] pub extern fn`, `pub static`, `pub const`, and spidering referenced types (<https://github.com/mozilla/cbindgen>, <https://github.com/mozilla/cbindgen/blob/master/docs.md>; latest 0.29.4, 2026-06-10). The C surface needs three attributes: `extern "C"` (calling convention), `#[no_mangle]` (stable symbol name — forgetting it gives a header with an undefined-reference at link time), `#[repr(C)]` (C layout). Build with `crate-type = ["cdylib", "staticlib"]`. **Reach:** any language with a C FFI — C, C++, Python (ctypes/cffi), Go (cgo), C#/.NET (P/Invoke), Swift, Java (JNI). **Cost:** you must design a C-shaped API — **no exported generic functions**, **no `&dyn Trait` or `&[T]` wide pointers**, no anonymous tuples; use **opaque pointers** (`*mut Foo` + `foo_new`/`foo_free`) and **manual memory management** (Rust must expose a free fn; C can't run `Drop`).

### 6.2 PyO3 + maturin → PyPI (if Python is ever wanted)
**PyO3:** `#[pyfunction]` + `#[pymodule]` + `wrap_pyfunction!`, crate built as `cdylib` (<https://www.maturin.rs/tutorial.html>, <https://pyo3.rs/main/building-and-distribution>). **maturin** builds & publishes wheels for Python 3.8+ on Win/Linux/macOS/FreeBSD (<https://github.com/PyO3/maturin>): `maturin develop` (local), `maturin build --release` (wheel in `target/wheels/`), `maturin publish` (→ PyPI). Linux needs **manylinux** (`--manylinux 2014`, since Rust 1.64 requires glibc ≥ 2.17); `maturin generate-ci github` emits the full CI matrix. **abi3 stable-ABI wheels** (`pyo3 = { features = ["abi3-py39"] }`) collapse the per-Python-version matrix to one wheel per OS/arch (<https://pyo3.rs/main/building-and-distribution>). **Ruff is the precedent** — but note Ruff uses maturin `bindings = "bin"` (a bundled binary, no PyO3 extension); a *library* Python API would instead use PyO3 + maturin `bindings = "pyo3"`.

---

## 7. Concrete build commands & CI

### 7.1 Per-artifact commands
```bash
# --- Rust crate (for Rust consumers / crates.io) ---
cargo build --release -p gdscript_ide
cargo test --workspace
cargo check --target wasm32-unknown-unknown -p gdscript_ide   # CI-enforce wasm-buildability (rust-analyzer trick)

# --- Node native addon (napi-rs) ---
npm i -g @napi-rs/cli
napi build --platform --release                               # local
napi build --platform --release --target aarch64-apple-darwin # cross
napi prepublish -t npm                                        # in prepublishOnly

# --- Browser WASM, route A (napi-rs v3, single binding) ---
napi build --platform --release --target wasm32-wasip1-threads

# --- Browser WASM, route B (wasm-bindgen) ---
wasm-pack build crates/wasm --target web --release            # also: --target bundler / nodejs
twiggy top -n 20 crates/wasm/pkg/*_bg.wasm                    # size profile
wasm-opt -Oz -o pkg/foo_bg.wasm pkg/foo_bg.wasm               # wasm-pack runs this automatically
wasm-pack publish

# --- C header (cbindgen) ---
cbindgen --config cbindgen.toml --crate gdscript_ffi --lang c --output gdscript.h

# --- Python wheel (maturin, if/when) ---
maturin build --release --manylinux 2014
maturin publish
```

### 7.2 GitHub Actions release matrix (sketch)
- **napi job** (per-platform `.node`): `windows-latest` → `x86_64`/`aarch64-pc-windows-msvc`; `macos-latest` → `x86_64`/`aarch64-apple-darwin`; `ubuntu-latest` → `x86_64`/`aarch64-unknown-linux-gnu` (`--use-napi-cross`), `x86_64`/`aarch64-unknown-linux-musl` (`-x`, cargo-zigbuild), `armv7-unknown-linux-gnueabihf`; plus `wasm32-wasip1-threads` on ubuntu. Each job `upload-artifact` `bindings-${target}`; a publish job `download-artifact` → `napi artifacts` → `npm publish napi/ --provenance` (pattern from oxc-resolver `release-napi.yml`, §2.5).
- **wasm job** (route B): one `ubuntu-latest` job, `wasm-pack build --target {web,bundler,nodejs}` ×3 → `wasm-pack publish` (Ruff's `publish-wasm.yml` pattern).
- **CLI binaries job** → **`dist`** (cargo-dist).

### 7.3 Release automation
- **`dist` (cargo-dist, axodotdev)** — "distributes your binaries": `dist init` **generates its own `release.yml`** that on a pushed git tag does Plan→Build→Host→Publish→Announce, building cross-platform binaries + installers (shell, PowerShell, MSI, Homebrew, npm) and uploading to GitHub Releases. It does **not** bump versions, write changelogs, or publish to crates.io. Actively maintained — **v0.32.0 (May 2026)** (<https://axodotdev.github.io/cargo-dist/>, <https://github.com/axodotdev/cargo-dist>).
- **`release-plz`** — "Publish Rust crates from CI with a Release PR": `release-plz update` opens/maintains a PR bumping versions (SemVer via Conventional Commits + cargo-semver-checks) and updating `CHANGELOG.md` (git-cliff); on merge, `release-plz release` tags `<pkg>-v<version>`, runs `cargo publish`, and cuts a GitHub release. Multi-crate workspace aware (<https://release-plz.dev/>, <https://github.com/release-plz/release-plz>).
- **The niches don't overlap:** release-plz = crates.io + changelog/version PRs; dist = cross-platform app binaries/installers on GitHub Releases; napi-rs `artifacts`/`prepublish` = `.node` → npm; `wasm-pack publish` = WASM → npm; maturin = wheels → PyPI. A common wiring: **release-plz cuts the tag → that tag fires dist + the napi/wasm publish workflows.**

---

## 8. Decisions for gdscript-analyzer (the bottom line)

1. **Workspace = oxc's shape.** Pure core crates (`gdscript_syntax`/`_db`/`_hir`/`_ide`/`_linter`), thin wrappers in `crates/{napi,wasm,cli}`. Centralize versions in `[workspace.dependencies]`. CI-check `cargo check --target wasm32-unknown-unknown -p gdscript_ide` to keep the query API portable (rust-analyzer's enforced invariant).
2. **API design = rust-analyzer's.** Expose an `ide`-style Rust query API behind a `hir` façade with an `AnalysisHost`/`Analysis` salsa snapshot model. This is the reusable "Roslyn" surface every consumer (napi, wasm, CLI, FFI) sits on.
3. **Node/LSP = napi-rs v3.** `.node` addon, per-platform `optionalDependencies`, swc's `binding.js`-style loader (with `isMusl()`).
4. **Browser = WASM.** Default to napi-rs v3 → `wasm32-wasip1-threads` (single binding, oxc's current path); fall back to a dedicated wasm-bindgen `crates/wasm` if bundle size / no-SharedArrayBuffer matters.
5. **crates.io = the `ra_ap_*` trick** if you want early Rust consumers without committing to stable public versions; otherwise `release-plz` for real published libs.
6. **Release = dist + release-plz**, with napi-rs/wasm-pack owning their npm publishes.
7. **Other languages = cheap optionality:** a `crates/ffi` (cbindgen) gets you C/Go/C#/Swift; a `crates/py` (PyO3 + maturin) gets you PyPI — both later, both proven (Ruff = the maturin precedent).
8. **Keep wasm-hostile features out of core:** no `std::fs`, no `SystemTime::now()` (use `web-time`), no thread-spawn in hot paths, getrandom `js`/`wasm_js` **only** in the wasm binding (note the 0.2→0.3 `js`→`wasm_js` rename).

---

## Appendix: primary sources

**Projects:** Biome <https://github.com/biomejs/biome> · <https://raw.githubusercontent.com/biomejs/biome/main/packages/%40biomejs/biome/bin/biome> · <https://biomejs.dev/guides/manual-installation/> · <https://deepwiki.com/biomejs/biome/7.2-javascript-api-and-wasm> ; swc <https://raw.githubusercontent.com/swc-project/swc/main/packages/core/binding.js> · <https://raw.githubusercontent.com/swc-project/swc/main/bindings/binding_core_node/Cargo.toml> · <https://github.com/swc-project/swc/issues/852> ; oxc <https://raw.githubusercontent.com/oxc-project/oxc/main/napi/parser/Cargo.toml> · <https://raw.githubusercontent.com/oxc-project/oxc/main/Cargo.toml> · <https://github.com/oxc-project/oxc/issues/21038> · <https://deepwiki.com/oxc-project/oxc/8.1-build-system-and-workspace> ; Ruff/ty <https://raw.githubusercontent.com/astral-sh/ruff/main/pyproject.toml> · <https://raw.githubusercontent.com/astral-sh/ruff/main/crates/ruff_wasm/Cargo.toml> · <https://docs.astral.sh/ty/installation/> ; rust-analyzer <https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/docs/book/src/contributing/architecture.md> · <https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/.github/workflows/ci.yaml> · <https://crates.io/crates/ra_ap_ide>

**napi-rs:** <https://napi.rs/docs/deep-dive/history> · <https://napi.rs/docs/deep-dive/release> · <https://napi.rs/docs/concepts/exports> · <https://napi.rs/docs/cli/build> · <https://napi.rs/docs/cli/pre-publish> · <https://napi.rs/docs/cli/artifacts> · <https://napi.rs/docs/cli/napi-config> · <https://napi.rs/docs/cross-build.en> · <https://napi.rs/blog/announce-v3> · <https://napi.rs/docs/concepts/webassembly> · <https://github.com/napi-rs/napi-rs> · <https://github.com/napi-rs/package-template/blob/main/index.js> · <https://github.com/oxc-project/oxc-resolver/blob/main/.github/workflows/release-napi.yml> · <https://nodejs.org/api/n-api.html>

**WASM:** <https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html> · <https://rustwasm.github.io/docs/wasm-pack/commands/build.html> · <https://rustwasm.github.io/docs/wasm-pack/commands/pack-and-publish.html> · <https://github.com/RReverser/serde-wasm-bindgen> · <https://github.com/rustwasm/wasm-bindgen/issues/2741> · <https://rustwasm.github.io/docs/book/reference/code-size.html> · <https://github.com/WebAssembly/binaryen> · <https://rustwasm.github.io/twiggy/> · <https://rustsec.org/advisories/RUSTSEC-2022-0054.html> · <https://github.com/RReverser/wasm-bindgen-rayon> · <https://web.dev/articles/webassembly-threads> · <https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html> · <https://blog.rust-lang.org/2024/04/09/updates-to-rusts-wasi-targets/>

**napi-vs-wasm / wasm pitfalls:** <https://yieldcode.blog/post/native-rust-wasm/> · <https://nickb.dev/blog/wasm-and-native-node-module-performance-comparison/> · <https://byteiota.com/rust-webassembly-performance-8-10x-faster-2025-benchmarks/> · <https://rustwasm.github.io/docs/book/reference/which-crates-work-with-wasm.html> · <https://docs.rs/web-time/latest/web_time/> · <https://docs.rs/getrandom/latest/getrandom/> · <https://docs.rs/getrandom/0.2.15/getrandom/>

**FFI / Python / release:** <https://github.com/mozilla/cbindgen> · <https://github.com/mozilla/cbindgen/blob/master/docs.md> · <https://www.maturin.rs/tutorial.html> · <https://www.maturin.rs/distribution.html> · <https://github.com/PyO3/maturin> · <https://pyo3.rs/main/building-and-distribution> · <https://axodotdev.github.io/cargo-dist/> · <https://github.com/axodotdev/cargo-dist> · <https://release-plz.dev/> · <https://github.com/release-plz/release-plz>
