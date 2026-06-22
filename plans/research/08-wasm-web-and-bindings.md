# 08 — WASM, the Web Playground, and Multi-Language Bindings (maximum reach)

**Status:** research note (toolchain + architecture constraints)
**Date:** 2026-06-22
**Owner goal (load-bearing):** MAXIMUM REACH. One Rust core (`gdscript-analyzer`) must power
(1) a **Node LSP** at native speed (via **napi**), (2) a **browser web playground** (via **WASM**),
and ideally (3) other-language consumers (Python, C ABI). The "web playground" is an explicit
driver: reach is the reason the core must be binding-agnostic. This note covers the Rust→WASM
toolchain, the precedent playgrounds we emulate, browser editor integration without a real LSP
server, the bundle-size/data-shipping plan, the portability rules the core must obey, and the
napi/Python/C reach.

**TL;DR recommendation:**
- One **binding-agnostic core crate** (no `std::fs`, content fed in as `&str`/VFS, threads
  feature-gated and `#[cfg]`-off on `wasm32`).
- A thin **`*_wasm` crate** (`wasm-bindgen`, built with `wasm-pack build --target web` on
  `wasm32-unknown-unknown`) for the playground.
- A thin **`*_napi` crate** (napi-rs → native `.node`) for the Node LSP (native speed, real threads).
- Editor integration: call the WASM analyzer's plain exported functions **directly** from the
  editor's completion/hover/lint providers — **no LSP-over-WASM**.
- Ship Godot's `extension_api.json` as a **pruned, binary (rkyv/postcard), brotli-compressed,
  separately-fetched, content-hashed CDN asset** — never `include_bytes!`'d into the wasm.
- This is exactly the **Biome** and **Ruff** topology.

---

## 1. The Rust → WASM toolchain for the browser

### 1.1 `wasm-bindgen` — the JS ↔ WASM bridge

`wasm-bindgen` is "a Rust library and CLI tool that facilitate high-level interactions between
Wasm modules and JavaScript." Raw WebAssembly can only pass integers/floats across the JS
boundary; `wasm-bindgen` raises this so you work "with rich types like strings, numbers, classes,
closures, and objects." The CLI post-processes the `.wasm` from `rustc` and emits a `.js` glue
file (plus `.d.ts` TypeScript), generating bindings **only for the imports you use and the Rust
you export**, so "you don't pay for unused functionality."
([wasm-bindgen Guide — Introduction](https://rustwasm.github.io/docs/wasm-bindgen/);
[Hello, World! — Rust+WASM book](https://rustwasm.github.io/book/game-of-life/hello-world.html);
[wasm-bindgen README](https://github.com/wasm-bindgen/wasm-bindgen))

For us: annotate an analysis entry point with `#[wasm_bindgen]`; the tool produces `.wasm` + JS shim
the playground imports.

### 1.2 `wasm-pack --target` options

`wasm-pack` wraps `cargo build --target wasm32-unknown-unknown`, runs `wasm-bindgen`, optionally
runs `wasm-opt`, and emits a publishable `pkg/`. The `--target` flag shapes the JS glue
([wasm-pack `build` docs](https://rustwasm.github.io/docs/wasm-pack/commands/build.html);
[wasm-bindgen Deployment](https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html)):

| `--target` | Produces | Use when |
|---|---|---|
| `bundler` (default) | ES module for a bundler (Webpack/Rollup/Vite); wasm treated as native ESM | Playground built through a bundler |
| **`web`** | **ES module natively importable in a browser**; no postprocessing; you `await` an init fn that manually instantiates the wasm | **Best fit: no-bundler static-host playground** |
| `nodejs` | CommonJS `require`; Node ≥ 8 | Node CLI / SSR / tests (not browser) |
| `no-modules` | `<script>`-tag global `wasm_bindgen`; fewer features, no local JS snippets | Legacy pages; superseded by `web` |
| `deno` | ES module for Deno | Deno only |

Invariant across targets: "WebAssembly still does not support ECMAScript modules," so even `web`
must manually instantiate. **Pick `wasm-pack build --target web`.**

### 1.3 `wasm32-unknown-unknown` vs `wasm32-wasip1`

- **`wasm32-unknown-unknown`** — the minimal target; "does not import any functions from the host."
  `core`/`alloc` work, but OS `std` is stubbed: "`println!` does nothing, `std::fs` always return
  errors, and `std::thread::spawn` will panic. There is no means by which this can be overridden."
  This is the browser/`wasm-bindgen` target.
  ([rustc: wasm32-unknown-unknown](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html))
- **`wasm32-wasip1`** (formerly `wasm32-wasi`, renamed March 2024) — imports the `wasi_snapshot_preview1`
  syscalls so real `std::fs` works, **but requires a WASI runtime and is not directly browser-runnable**
  without a polyfill; threads/process-spawn unavailable.
  ([rustc: wasm32-wasip1](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html))

**Pick `wasm32-unknown-unknown` for the playground** — the analyzer takes a source string in memory
and returns structured results; no filesystem/syscalls needed. (`wasm32-wasip1` only if you ever
need real `std::fs` under a server-side WASI runtime.)

### 1.4 How JS calls in (the `--target web` flow)

```js
import init, { analyze } from './pkg/gdscript_analyzer.js';
await init();                       // loads + compiles the .wasm (locates it via import.meta)
const diagnostics = analyze(src);   // calls an exported #[wasm_bindgen] fn
```

`init()` "loads and compiles the WebAssembly file," locating the `.wasm` via `import.meta` (or an
explicit path/`Response`/`ArrayBuffer`).
([wasm-bindgen — Without a Bundler](https://rustwasm.github.io/docs/wasm-bindgen/examples/without-a-bundler.html))

### 1.5 Passing structured data in/out (source string → diagnostics/completions)

**Use `serde-wasm-bindgen`** — converts Rust structs ↔ native JS objects "without requiring JSON
serialization as an intermediary step," giving "much smaller code size overhead than JSON, and, in
most common cases, … much faster serialization/deserialization." `to_value(&x) -> JsValue` and
`from_value(jsval)`. The data struct needs only `#[derive(Serialize, Deserialize)]`; `#[wasm_bindgen]`
goes on the exported function. Use `Result<JsValue, JsValue>` so errors surface as JS exceptions.
([serde_wasm_bindgen docs.rs](https://docs.rs/serde-wasm-bindgen/latest/serde_wasm_bindgen/);
[wasm-bindgen — Arbitrary Data with Serde](https://rustwasm.github.io/docs/wasm-bindgen/reference/arbitrary-data-with-serde.html))

```rust
#[derive(Serialize, Deserialize)]            // no #[wasm_bindgen] on the data type
pub struct Diagnostic { pub from: u32, pub to: u32, pub severity: String, pub message: String }

#[wasm_bindgen]
pub fn analyze(src: &str) -> Result<JsValue, JsValue> {
    let diags: Vec<Diagnostic> = run_analysis(src);
    Ok(serde_wasm_bindgen::to_value(&diags)?)
}
```

The natural API: `analyze(src) -> Vec<Diagnostic>`, `complete(src, line, col) -> Vec<Completion>`,
`hover(src, pos) -> Option<Hover>`. **Alternative:** serialize to a JSON string (`serde_json::to_string`)
and `JSON.parse` in JS — simpler/debuggable, but slower and larger on the hot keystroke path; the old
`JsValue::into_serde` JSON path was deprecated in favor of `serde-wasm-bindgen` for exactly this reason.
Default to `serde-wasm-bindgen` for the hot path; a JSON fallback is fine for one-off config/logging.

### 1.6 Build optimization (in this toolchain)

`wasm-pack` runs **`wasm-opt`** automatically on the release profile (configurable under
`[package.metadata.wasm-pack.profile.release]`, e.g. `wasm-opt = ['-Os']`); see §4 for the full
size budget.
([wasm-pack Cargo.toml config](https://rustwasm.github.io/docs/wasm-pack/cargo-toml-configuration.html))

---

## 2. Precedent playgrounds — analyzers in the browser via WASM

All `.wasm` sizes below are **uncompressed** (read from the published npm packages); they
brotli-compress to roughly 30–45% over the wire, but the browser still compiles the full uncompressed
module.

| Project | Lang→WASM | Editor | Approx `.wasm` (uncompressed) | Notes / limitations | Source |
|---|---|---|---|---|---|
| **rust-analyzer** (community demos) | Rust → `wasm-pack --target web` (wasm-bindgen) | Monaco | Multi-MB; no official slim build | Full compiler front-end → impractically large in-browser; **the ceiling, not a template** | [rust-analyzer-wasm](https://github.com/rust-analyzer/rust-analyzer-wasm) (archived) · [detrumi/rust-analyzer-wasm](https://github.com/detrumi/rust-analyzer-wasm) · [ra-wasm demo](https://ra-wasm.netlify.app/) |
| **Biome** | Rust → wasm-bindgen (`biome_wasm`, 3 targets) | **CodeMirror 6** + Lezer | **`biome_wasm_bg.wasm` ≈ 37 MB** (v2.5.0) | Full formatter+linter+analyzer; heaviest | [biomejs/biome](https://github.com/biomejs/biome) · [biomejs/website](https://github.com/biomejs/website) · [DeepWiki: JS API & WASM](https://deepwiki.com/biomejs/biome/7.2-javascript-api-and-wasm) |
| **Ruff** | Rust → wasm-bindgen (`ruff_wasm`) | **Monaco** | **`ruff_wasm_bg.wasm` ≈ 10.8 MB** (v0.15.18) | **React + Vite** SPA; `localStorage`; must build wasm in **release** or perf suffers | [ruff/playground README](https://github.com/astral-sh/ruff/blob/main/playground/README.md) · [npm @astral-sh/ruff-wasm-web](https://www.npmjs.com/package/@astral-sh/ruff-wasm-web) |
| **oxc** | Rust → **`wasm32-wasip1-threads`** (WASI threads, **not** the usual wasm-bindgen path) | **Monaco** | Playground few-MB; standalone `@oxc-parser/wasm` ≈ **0.72 MB** | **Vue 3 + Vite**, built via `just install-wasm`; parser-only wasm is ~50× smaller than the full toolchain | [oxc-project/playground](https://github.com/oxc-project/playground) · [oxc justfile](https://github.com/oxc-project/oxc/blob/main/justfile) · [npm @oxc-parser/wasm](https://www.npmjs.com/package/@oxc-parser/wasm) |
| **swc** | Rust → wasm-bindgen (`@swc/wasm-web`) | **Monaco** (dual input/output) | **`wasm_bg.wasm` ≈ 19.3 MB** (v1.15.43) | Version switcher, AST view, share-by-URL | [swc-playground](https://github.com/swc-project/swc-playground) · [npm @swc/wasm-web](https://www.npmjs.com/package/@swc/wasm-web) |
| **web-tree-sitter** | C → WASM (Emscripten / wasi-sdk) | Editor-agnostic lib | **runtime ≈ 195 KB**; grammars: JS ≈ 402 KB, Python ≈ 447 KB, **Rust ≈ 1.08 MB** | **Lazy `Language.load(url)` per grammar** — tiny core + on-demand modules; wasm grammars slower than native | [tree-sitter binding_web README](https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md) · [npm web-tree-sitter](https://www.npmjs.com/package/web-tree-sitter) |

**Key reads:**
- **Ruff** is the cleanest reference template: React + Vite + Monaco + a `ruff_wasm` wasm-bindgen
  crate, `new Workspace(settings, PositionEncoding.UTF16)` → `workspace.check(src): Diagnostic[]`.
  Its README inspired by Biome/Tailwind Play; the React+Vite+Monaco+wasm-bindgen stack is the de-facto
  template. ([Ruff playground README](https://github.com/astral-sh/ruff/blob/main/playground/README.md))
- **Biome** uses CodeMirror 6 (its website `package.json` has `@codemirror/*` + `@lezer/*` +
  `@biomejs/wasm-web`, **no `monaco-editor`**); the worker builds a `MemoryFileSystem` + `Workspace`
  and calls `workspace.pullDiagnostics(...)`. The `_bg.wasm` suffix is the wasm-bindgen tell.
  ([biomejs/website](https://github.com/biomejs/website))
- **rust-analyzer** is the cautionary tale: a complete semantic compiler front-end is multi-MB and
  can't do real cross-crate analysis in-browser. **Scope the analysis surface deliberately.**
  ([rustwasm code-size guide](https://rustwasm.github.io/book/reference/code-size.html))
- **web-tree-sitter** is the architecture to emulate if we want multi-language/multi-grammar scale:
  a tiny (~200 KB) runtime + lazily `fetch`-ed per-grammar `.wasm`. Directly relevant if we use
  tree-sitter for parsing. ([binding_web README](https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md))

**Bundle reality:** a full Rust analyzer compiled to wasm realistically lands **10–37 MB
uncompressed** (Ruff 10.8, swc 19.3, Biome 37); a parser-only build can be **< 1 MB** (oxc 0.72).

---

## 3. Editor integration in the browser — no real LSP server

Mental model for **both** editors: completion, hover, and diagnostics are **just callbacks**. You
register a function; the editor calls it with a position/model; you return data in the editor's shape.
A WASM analyzer fits perfectly — its exports run in-process, synchronously (or promisified). **No
protocol layer is needed** between "analyzer produced a result" and "editor displays it."

### 3.1 Monaco (used by Ruff, swc, oxc)

- **Completion:** `monaco.languages.registerCompletionItemProvider(lang, { provideCompletionItems(model, position, …) {…} })`
  — read `model.getValue()` + `position`, call `wasm.completions(...)`, return a `CompletionList`.
  `triggerCharacters` auto-invokes after `.`/`:`; `resolveCompletionItem` lazily computes docs.
- **Hover:** `monaco.languages.registerHoverProvider(lang, { provideHover(model, position, …) {…} })`
  → return `{ range, contents: IMarkdownString[] }`.
- **Diagnostics:** **not** a provider — pushed via `monaco.editor.setModelMarkers(model, owner, markers)`
  with `IMarkerData { severity, message, startLineNumber, startColumn, endLineNumber, endColumn }`,
  severity = `monaco.MarkerSeverity.{Error|Warning|Info|Hint}`. This is "analogous to diagnostics in
  the language server protocol… Monaco itself only supports this through `setModelMarkers`."

([Monaco docs index](https://microsoft.github.io/monaco-editor/docs.html);
[registerCompletionItemProvider](https://microsoft.github.io/monaco-editor/typedoc/functions/languages.registerCompletionItemProvider.html);
[monaco-marker-data-provider](https://github.com/remcohaszing/monaco-marker-data-provider);
[setModelMarkers / inline errors](https://app.studyraid.com/en/read/15534/540334/displaying-inline-errors-and-warnings-in-monaco))

### 3.2 CodeMirror 6 (used by Biome)

- **Completion:** `autocompletion({ override: [myWasmSource] })`; the source `(context) => CompletionResult`
  reads `context.state.doc.toString()` + `context.pos`, calls `wasm.completions(...)`, returns
  `{ from, options }`. `CompletionResult.validFor` keeps filtering without re-calling WASM.
- **Diagnostics:** `@codemirror/lint` `linter(view => Diagnostic[])` (auto, debounced) — each
  `Diagnostic { from, to, severity: "info"|"warning"|"error"|"hint", message, actions? }`; add
  `lintGutter()`. Push externally-computed diagnostics via `setDiagnostics` / flush with `forceLinting`.
  `actions` map to analyzer auto-fixes.
- **Hover:** `hoverTooltip((view, pos, side) => Tooltip | null)` — `create(view)` builds the DOM
  (inject Markdown-rendered HTML).

([CM Autocompletion](https://codemirror.net/examples/autocompletion/);
[CM Lint](https://codemirror.net/examples/lint/);
[CM Tooltip](https://codemirror.net/examples/tooltip/);
[CM reference](https://codemirror.net/docs/ref/))

### 3.3 The critical WASM gotcha — byte offsets vs UTF-16

A Rust analyzer reports spans as **UTF-8 byte offsets**; both editors want **UTF-16 line/column**.
You must convert. Biome ships `spanInBytesToSpanInCodeUnits` for exactly this; **Ruff avoids it by
taking `PositionEncoding.UTF16`** so it returns UTF-16 ranges directly. **This is the single most
common implementation pitfall** — prefer Ruff's approach: have the analyzer emit UTF-16 ranges, or
ship a byte→UTF-16 converter.
([studyraid offset note](https://app.studyraid.com/en/read/15534/540334/displaying-inline-errors-and-warnings-in-monaco);
[@astral-sh/ruff-wasm-web](https://www.npmjs.com/package/@astral-sh/ruff-wasm-web))

### 3.4 Monaco vs CodeMirror 6

- **Bundle size — CM6 wins decisively.** Monaco ~2–5 MB optimized (Sourcegraph measured **2.4 MB**,
  ~40% of their page deps); CM6 ~**50 KB** minimal / ~300 KB fuller. Sourcegraph's Monaco→CM6 swap
  cut total JS **6 MB → 3.4 MB (−43%)**. With a multi-MB wasm blob already on the page, this matters.
  ([Sourcegraph migration](https://sourcegraph.com/blog/migrating-monaco-codemirror))
- **Custom-language ergonomics:** Monaco is batteries-included (command palette, multi-cursor) and
  familiar if you know VS Code APIs, but heavier to bend to a new language and harder to theme. CM6
  requires assembling extensions, but each feature is a small isolated callback — ideal for one bespoke
  language fed by one WASM module.
- **Field is split:** Biome/old-oxc = CM6; Ruff/new-oxc/swc = Monaco. **Neither is "wrong" — both
  consume the exact same plain WASM API.** Decide on bundle budget vs batteries-included UX.

### 3.5 Is there an "LSP-over-WASM" pattern? (Yes — but don't use it here)

`monaco-languageclient` (TypeFox) can bridge a **full LSP client** to Monaco over JSON-RPC, talking
to an **actual language server compiled to WASM** (e.g. clangd-in-wasm) or written in JS, running in
an in-process Web Worker — full LSP entirely client-side, no network.
([monaco-languageclient](https://github.com/TypeFox/monaco-languageclient);
[Language Server Examples](https://deepwiki.com/TypeFox/monaco-languageclient/4.1-language-server-examples))

**But for a single-language playground it's over-engineering.** Every Rust-analyzer playground we
surveyed skips the protocol and calls plain WASM exports directly (Ruff `workspace.check()` → Monaco
markers; Biome `workspace.pullDiagnostics()` → CM diagnostics). The analyzer and editor live in the
same JS context, so JSON-RPC is pure overhead + added bundle weight.

| | LSP-over-WASM | Direct WASM API (Biome/Ruff/oxc) |
|---|---|---|
| Protocol overhead | Full JSON-RPC marshalling | None (function call → return value) |
| Bundle weight | + monaco-languageclient + vscode-languageserver shims | Just editor + your WASM |
| Effort | Wrap analyzer as LSP server, worker transport, Monaco services | Map results to provider shapes (~tens of lines) |
| Pays off when | You already have a reusable LSP server / want full VS Code parity across many languages | — |
| Single-lang playground | Overkill | **Recommended** |

**Recommendation:** call the WASM analyzer's exported functions **directly** from the editor's
completion/hover/lint providers. Optionally run the WASM in a **Web Worker** to keep the UI responsive
on large inputs (a perf choice, not an LSP requirement — Biome/Ruff both do worker-based WASM). Reach
for monaco-languageclient only if you later want to reuse one real LSP server in both the desktop IDE
and the web.

---

## 4. Bundle size & performance budget

### 4.1 Cargo release-profile flags (set these first)

```toml
[profile.release]
lto = true            # link-time opt: smaller AND faster at runtime
opt-level = "z"       # size; ALSO try "s" — "s" can sometimes be smaller than "z"
codegen-units = 1     # better cross-fn optimization / dead-code elimination
panic = "abort"       # drop unwinding + panic-formatting machinery
strip = true          # strip debug symbols
```

- `lto` is a dual win ("smaller … AND faster at runtime").
- `opt-level`: **measure both `"z"` and `"s"`** — "surprisingly enough, `opt-level = "s"` can
  sometimes result in smaller binaries than `opt-level = "z"`."
- Source-level: avoid `format!`/`to_string` in hot paths; consider `dyn` over generics to cut
  monomorphization bloat (an analyzer is monomorphization-heavy); `wasm-snip` for provably-unreachable bodies.

([rustwasm: Shrinking .wasm Size](https://rustwasm.github.io/book/reference/code-size.html);
[Leptos binary-size guide](https://book.leptos.dev/deployment/binary_size.html);
[Yew optimizations](https://yew.rs/docs/next/advanced-topics/optimizations))

### 4.2 `wasm-opt` (Binaryen)

Re-optimizes the `.wasm` after LLVM, exploiting wasm semantics LLVM doesn't. The rustwasm book:
"can often get another **15-20% savings** on code size" plus runtime speedups. `wasm-opt -Oz`/`-Os`.
**`wasm-pack` runs it automatically on release** — tune via the metadata profile.
([rustwasm code-size](https://rustwasm.github.io/book/reference/code-size.html))

### 4.3 `twiggy` — code-size profiler

Analyzes the call graph to tell you **why** bytes are there. `twiggy top` (largest items),
`dominators` (retained size — the high-value removal targets), `paths` (why is this here?),
**`monos`** (generic-monomorphization bloat — directly relevant to an analyzer), `garbage`
(unreachable), `diff` (CI regression guard). ([rustwasm/twiggy](https://github.com/rustwasm/twiggy))

### 4.4 Allocator

- **Avoid `wee_alloc`** — O(n) first-fit (bad for an alloc-heavy analyzer) and **effectively
  unmaintained** (open leak issue, advisory). ([wee_alloc](https://github.com/rustwasm/wee_alloc);
  [nickb.dev](https://nickb.dev/blog/avoiding-allocations-in-rust-to-shrink-wasm-modules/))
- **Keep default `dlmalloc`** for v1 (correct, fast, ~10–20 KB).
- **`talc`** is the modern drop-in (`#[global_allocator]`) — "much faster than DLmalloc and much
  smaller," but the real-world delta is often small; benchmark before adopting.
  ([talc](https://github.com/SFBdragon/talc/blob/master/talc/README_WASM.md))

### 4.5 Calibration — real peer bundle sizes

`@astral-sh/ruff-wasm-web` v0.15.18 npm unpacked ≈ **10.87 MB**; `@biomejs/wasm-web` v2.5.0 ≈
**37.4 MB** (unpacked includes JS glue + `.d.ts`; raw `.wasm` smaller; brotli ~3–4× over the wire).
A formatter-only Ruff variant is ~1.72 MB — feature surface drives size.
([registry: ruff-wasm-web](https://registry.npmjs.org/@astral-sh/ruff-wasm-web/latest);
[registry: @biomejs/wasm-web](https://registry.npmjs.org/@biomejs/wasm-web/latest))
**Expect a single-digit-MB wasm for `gdscript-analyzer` — in line with Ruff, acceptable for a playground.**

---

## 5. Shipping the multi-MB `extension_api.json` to the browser

**The payload (Godot PR #82331):** `extension_api.json` **without docs ≈ 5.3 MB**, **with docs ≈
8.7 MB**; **gzipped ≈ 400 KB (no docs)** / ~1.2 MB (with docs). The uncompressed JSON is the enemy,
but it's ~**13:1** compressible and we rarely need the doc strings for static analysis.
([Godot PR #82331](https://github.com/godotengine/godot/pull/82331))

**Strategy (do all four):**

1. **Prune.** Use the no-docs dump (5.3 MB) and drop fields the analyzer never reads (descriptions,
   theme items, etc.). Biggest lever, zero runtime cost.
2. **Convert JSON → compact binary.** JSON parsing also costs CPU + transient memory at startup.
   - **`rkyv` (recommended for the in-wasm path):** total **zero-copy** — "accessing the data is just
     a pointer offset and a cast … it doesn't *scale* with our data" (constant-time, no parse, no
     parallel allocation). Ideal for a multi-MB API description loaded once and queried read-only. The
     fast path is `unsafe` (validate untrusted bytes with `bytecheck`), but since this is **our own,
     trusted, build-time asset**, ship it pre-validated and skip per-load validation. Treat the archive
     as a build artifact regenerated from the JSON (format-fragile across versions/arch).
     ([rkyv zero-copy](https://rkyv.org/zero-copy-deserialization.html);
     [rkyv is faster than…](https://david.kolo.ski/blog/rkyv-is-faster-than/); [rkyv FAQ](https://rkyv.org/faq.html))
   - **`postcard` (recommended if you want safe serde):** varint-packed, **smaller than bincode**,
     deserializes into owned structs (a one-time copy — acceptable for a few MB loaded once).
     ([benchmark](https://david.kolo.ski/blog/rkyv-is-faster-than/))
   - `bincode` (postcard beats it on size) / `flatbuffers` (larger, less ergonomic in Rust than rkyv).
3. **Brotli-compress the binary** for transport (pre-compressed `.br`, `Content-Encoding`). JSON
   compresses ~70–90%; brotli beats gzip ~15–25% on text and decompresses just as fast.
   ([Lemire JSON gzip vs zstd ~12–13:1](https://lemire.me/blog/2021/06/30/compressing-json-gzip-vs-zstd/);
   [brotli vs gzip](https://paulcalvano.com/2018-07-25-brotli-compression-how-much-will-it-reduce-your-content/))
4. **Fetch separately at runtime — do NOT `include_bytes!` into the `.wasm`.** Embedding "makes the
   final binary bigger, does not allow you to use a CDN, nor lets the browser do its cache magic," and
   every code change re-ships the unchanged data. A separate, content-hashed, immutable asset caches
   independently of code releases, downloads in parallel with wasm instantiation, and — critically —
   keeps the **compiled-wasm module cache** stable across releases (V8 re-compiles wasm if the URL
   changes). ([include_bytes vs fetch](https://emilio-moretti.medium.com/rust-wasm-downloading-files-in-runtime-instead-of-include-bytes-f8c29a958e20);
   [V8 wasm code caching](https://v8.dev/blog/wasm-code-caching))

**Concrete shape:** at build time, parse the pruned `extension_api.json` once → emit
`extension_api.<godot-version>.rkyv.br` (fall back to postcard for fully-safe code) → ship as a
content-hashed immutable CDN asset → `fetch()` + brotli-decode + hand bytes to rkyv for zero-copy
access. **~400 KB over the wire, near-zero parse cost.** (web-tree-sitter's lazy `Language.load(url)`
is the same "fetch the big data separately, on demand" pattern.)

---

## 6. WASM limitations the core MUST be designed around

### 6.1 The constraints

- **No threads by default.** On `wasm32-unknown-unknown`, `std::thread::spawn` panics. Threads require
  the WebAssembly threads proposal + `SharedArrayBuffer` (gated behind **cross-origin isolation**:
  the page must send `Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy:
  require-corp`) + Web Workers + atomics. The Rust path (`wasm-bindgen-rayon`) additionally needs a
  **nightly `build-std`** and `-C target-feature=+atomics,+bulk-memory`. The rustwasm guide calls the
  setup "wonky," and "the main thread in a browser cannot block… you can't so much as acquire a mutex."
  **→ playgrounds run single-threaded.**
  ([rustc wasm32-unknown-unknown](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html);
  [web.dev COOP/COEP](https://web.dev/articles/coop-coep);
  [rustwasm raytrace](https://rustwasm.github.io/docs/wasm-bindgen/examples/raytrace.html);
  [wasm-bindgen-rayon](https://github.com/RReverser/wasm-bindgen-rayon))
- **No filesystem.** `std::fs` "always return[s] errors" on the browser target. The playground feeds
  file **contents** in via JS — never `fs::read`. (Ruff treats a VFS as first-class precisely because
  the browser has none.)
  ([rustwasm #1727](https://github.com/rustwasm/wasm-bindgen/issues/1727);
  [Ruff discussion #9977](https://github.com/astral-sh/ruff/discussions/9977))
- **No networking/sockets** in the browser sandbox (must go through JS `fetch`/WebSocket); the core
  must not fetch config/rulesets itself.
- **Slower than native.** USENIX ATC '19 measured WASM **~45% slower in Firefox, ~55% slower in
  Chrome** (peaks ~2.5×) vs native. **→ this is exactly why the LSP uses napi (native), not wasm.**
  ([arxiv 1901.09056](https://arxiv.org/abs/1901.09056))

### 6.2 The portability RULES (so one source compiles to native + wasm)

1. **Never call `std::fs` (or `std::net`) in the core.** Take content as `&str`/`String`, or via a
   `VFS` trait you control (Biome's `biome_fs`). Each binding supplies bytes.
2. **Feature-gate fs/threads/networking** behind `#[cfg(...)]` / Cargo features so wasm builds exclude
   them entirely (`#[cfg(target_arch = "wasm32")]` / `#[cfg(not(...))]`).
   ([Rust conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html))
3. **No threads in the core hot path** (or gate behind a `parallel`/`rayon` feature, `#[cfg]`-off on wasm).
4. **Keep wasm glue in a separate thin `*_wasm` crate** that depends on the core and owns the
   `wasm-bindgen` surface — the core stays binding-agnostic.

**Both Biome and Ruff do exactly this.** Ruff's `ruff_wasm` is a separate `wasm-bindgen` crate that
**accepts source as a string** (`workspace.check(src)`), not via fs. Biome's `biome_wasm`
(`crate-type = ["cdylib","rlib"]`, `wasm-bindgen` dep) is a thin shell over the shared core crates
(`biome_service`, `biome_diagnostics`, and the **`biome_fs` filesystem abstraction** — not raw
`std::fs`).
([ruff_wasm](https://github.com/astral-sh/ruff/tree/main/crates/ruff_wasm);
[biome_wasm Cargo.toml](https://raw.githubusercontent.com/biomejs/biome/main/crates/biome_wasm/Cargo.toml);
[Biome monorepo structure](https://deepwiki.com/biomejs/biome/2.1-monorepo-structure))

---

## 7. The napi (Node native) path — the LSP binding

**napi-rs** builds the Rust core into a native **`.node`** addon loaded directly by Node via Node-API:

- **Native speed, no wasm penalty** — "executes directly within the Node.js process," avoiding the
  45–55% wasm slowdown. Real threads available (unlike browser wasm).
- **ABI-stable across Node versions** (Node-API forward-compat; Node 10–22, all major OS/arch).
- **Ergonomic:** `napi build` emits the `.node` + generated TypeScript types; no node-gyp; per-platform
  binaries shipped as optionalDependencies.

([napi.rs](https://napi.rs/); [getting started](https://napi.rs/docs/introduction/getting-started);
[napi-rs repo](https://github.com/napi-rs/napi-rs))

This is the right binding for a long-lived, CPU-bound LSP server.

### 7.1 One core → two bindings is the standard (Biome proves it)

Biome ships **both** from the same core crates:
- **Native via napi for Node:** `@biomejs/cli-*` per-platform binaries (`cli-darwin-arm64`,
  `cli-linux-x64`, …) as optional deps.
- **WASM for the browser:** the separate `biome_wasm` crate → `@biomejs/wasm-web` / `-bundler` / `-nodejs`.

Both sit on the same shared core. Ruff is identical (shared core, thin `ruff_wasm`, native CLI).
**napi = native speed for the server; wasm = portability for the browser; same core underneath.**
([Biome @biomejs packages](https://github.com/biomejs/biome/tree/main/packages/%40biomejs);
[Biome architecture](https://biomejs.dev/internals/architecture/))

---

## 8. Other-language reach (PyO3, C ABI) + the guitkx angle

### 8.1 Python — PyO3 + maturin (feasible, low cost)

`crate-type = ["cdylib"]` + a thin PyO3 wrapper over the same core; **maturin** builds shippable
**wheels** (`maturin develop` / `maturin build`) so users `pip install` without a Rust toolchain.
Cost: small — the `#[pyfunction]`/`#[pyclass]` surface + a CI wheel matrix.
([pyo3.rs](https://pyo3.rs/); [maturin](https://www.maturin.rs/); [PyO3/maturin](https://github.com/PyO3/maturin))

### 8.2 C ABI — `extern "C"` + cbindgen (feasible, broadest reach)

Expose `#[no_mangle] pub extern "C"` fns + `#[repr(C)]` types from a `cdylib`/`staticlib`; **cbindgen**
auto-generates the C/C++11 header. Any language with a C FFI (C, C++, Go via cgo, Ruby, …) can then
embed the core. Cost: moderate — design a stable C-friendly ABI (opaque pointers, manual `free`, no
panics across the boundary).
([cbindgen](https://github.com/eqrion/cbindgen); [announcing cbindgen](https://blog.eqrion.net/announcing-cbindgen/))

Both are thin wrappers over the same binding-agnostic core — no core changes, only the same
content-in/diagnostics-out discipline from §6.2. Optional future surfaces the architecture already
keeps open.

### 8.3 The guitkx angle

**guitkx's LSP runs in Node → served by the napi build** (native speed, full threads). A future
**guitkx web playground** would use the **WASM build** (single-threaded, string-fed). **Both are
served by one core crate** — the same napi+wasm topology as `gdscript-analyzer` itself. Nothing extra
is required: the portability rules in §6.2 already make the core serve Node, the browser, Python, and C.

---

## 9. Architecture summary (what the core must look like)

```
core crate (pure analysis)
  - no std::fs / std::net; content in as &str / VFS trait (cf. biome_fs)
  - threads/parallelism behind a Cargo feature, #[cfg]-off on wasm32
  - emits engine-neutral structured data; UTF-16-friendly ranges (or a converter)
        │
        ├── *_napi  crate → native .node  (Node LSP: native speed, real threads, Node-API ABI-stable)
        ├── *_wasm  crate → wasm-bindgen   (browser playground: --target web, wasm32-unknown-unknown,
        │                                   single-threaded, serde-wasm-bindgen in/out)
        ├── *_py    crate → PyO3 + maturin wheels        (optional reach)
        └── *_capi  crate → extern "C" + cbindgen header (optional reach)
```

This is the validated **Biome/Ruff topology**. Playground stack to emulate: **Ruff** (React/Vite +
Monaco + wasm-bindgen) or Biome (CodeMirror 6). Editor integration: **direct WASM API calls** from the
editor's providers (no LSP-over-WASM). Data: **pruned → rkyv/postcard → brotli → separately-fetched
content-hashed CDN asset** (~400 KB on the wire), web-tree-sitter style.

---

## Key URLs

**Toolchain**
- wasm-bindgen Guide: https://rustwasm.github.io/docs/wasm-bindgen/
- wasm-bindgen Deployment (targets): https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html
- wasm-bindgen Without a Bundler (init flow): https://rustwasm.github.io/docs/wasm-bindgen/examples/without-a-bundler.html
- wasm-bindgen Arbitrary Data with Serde: https://rustwasm.github.io/docs/wasm-bindgen/reference/arbitrary-data-with-serde.html
- wasm-pack build / --target: https://rustwasm.github.io/docs/wasm-pack/commands/build.html
- wasm-pack Cargo.toml config (wasm-opt): https://rustwasm.github.io/docs/wasm-pack/cargo-toml-configuration.html
- rustc wasm32-unknown-unknown: https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html
- rustc wasm32-wasip1: https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html
- serde-wasm-bindgen: https://docs.rs/serde-wasm-bindgen/latest/serde_wasm_bindgen/

**Precedent playgrounds**
- rust-analyzer-wasm (archived): https://github.com/rust-analyzer/rust-analyzer-wasm · demo https://ra-wasm.netlify.app/
- Biome: https://github.com/biomejs/biome · website https://github.com/biomejs/website · WASM https://deepwiki.com/biomejs/biome/7.2-javascript-api-and-wasm
- Ruff playground README: https://github.com/astral-sh/ruff/blob/main/playground/README.md · npm https://www.npmjs.com/package/@astral-sh/ruff-wasm-web
- oxc: https://github.com/oxc-project/playground · justfile https://github.com/oxc-project/oxc/blob/main/justfile · npm https://www.npmjs.com/package/@oxc-parser/wasm
- swc: https://github.com/swc-project/swc-playground · npm https://www.npmjs.com/package/@swc/wasm-web
- web-tree-sitter: https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md · npm https://www.npmjs.com/package/web-tree-sitter

**Editor integration**
- Monaco docs: https://microsoft.github.io/monaco-editor/docs.html · setModelMarkers https://app.studyraid.com/en/read/15534/540334/displaying-inline-errors-and-warnings-in-monaco · markers≈LSP https://github.com/remcohaszing/monaco-marker-data-provider
- CodeMirror 6: autocomplete https://codemirror.net/examples/autocompletion/ · lint https://codemirror.net/examples/lint/ · tooltip https://codemirror.net/examples/tooltip/ · ref https://codemirror.net/docs/ref/
- Monaco vs CM6 (bundle): https://sourcegraph.com/blog/migrating-monaco-codemirror
- monaco-languageclient (LSP-over-WASM): https://github.com/TypeFox/monaco-languageclient · examples https://deepwiki.com/TypeFox/monaco-languageclient/4.1-language-server-examples

**Bundle size & data shipping**
- rustwasm Shrinking .wasm Size: https://rustwasm.github.io/book/reference/code-size.html
- twiggy: https://github.com/rustwasm/twiggy
- wee_alloc (avoid): https://github.com/rustwasm/wee_alloc · talc: https://github.com/SFBdragon/talc
- ruff-wasm-web size: https://registry.npmjs.org/@astral-sh/ruff-wasm-web/latest · biome wasm-web size: https://registry.npmjs.org/@biomejs/wasm-web/latest
- Godot extension_api.json sizes (PR #82331): https://github.com/godotengine/godot/pull/82331
- JSON compression (Lemire): https://lemire.me/blog/2021/06/30/compressing-json-gzip-vs-zstd/ · brotli https://paulcalvano.com/2018-07-25-brotli-compression-how-much-will-it-reduce-your-content/
- rkyv zero-copy: https://rkyv.org/zero-copy-deserialization.html · benchmark https://david.kolo.ski/blog/rkyv-is-faster-than/
- include_bytes vs fetch: https://emilio-moretti.medium.com/rust-wasm-downloading-files-in-runtime-instead-of-include-bytes-f8c29a958e20 · V8 wasm cache https://v8.dev/blog/wasm-code-caching

**WASM limits & bindings**
- COOP/COEP cross-origin isolation: https://web.dev/articles/coop-coep · wasm threads https://web.dev/articles/webassembly-threads · wasm-bindgen-rayon https://github.com/RReverser/wasm-bindgen-rayon
- Wasm vs native (45–55% slower): https://arxiv.org/abs/1901.09056
- Rust conditional compilation: https://doc.rust-lang.org/reference/conditional-compilation.html
- napi-rs: https://napi.rs/ · repo https://github.com/napi-rs/napi-rs · Biome @biomejs packages https://github.com/biomejs/biome/tree/main/packages/%40biomejs · Biome architecture https://biomejs.dev/internals/architecture/
- PyO3: https://pyo3.rs/ · maturin: https://www.maturin.rs/ · cbindgen: https://github.com/eqrion/cbindgen
