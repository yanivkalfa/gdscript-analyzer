# Phase 5 · Workstream (napi) — `gdscript-ffi` + `gdscript-session` Playbook

> The Node binding that the guitkx editor extension consumes (Workstream 2). Research-validated
> (napi-rs v3 GA, 3.9.x). Mirrors the LSP/CLI playbook format.
>
> **Parents:** [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) §Prereqs +
> §Workstream 2/5, [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §4.

---

## 0. Thesis & the testability constraint

A napi `cdylib` **cannot be `cargo test`ed natively** — the test binary fails to link against the
Node-API symbols (`napi_*`), which the host Node process only resolves at load time. On Windows this
is a hard failure (the `napi-build` script panics "libnode.dll not found" at *build* time; even
`cargo check -p gdscript-ffi` fails). napi-rs's own README confirms tests are JS-side "in order to
resolve symbols."

**Therefore the load-bearing decision: all real logic lives in a pure-Rust core, `gdscript-session`,
unit-tested with plain `cargo test`; the napi crate is a thin `#[napi]` delegator.** This is exactly
oxc's split — `crates/oxc_parser` (zero napi deps, fully testable) vs `napi/parser` (the cdylib
binding). The wasm binding (`gdscript-wasm`) wraps the **same** `Session`.

```
gdscript-session  (pure Rust, wasm-clean, in default-members → xtask ci tests it)
      ├── gdscript-ffi   (#[napi] thin wrapper)        ← Node / guitkx
      └── bindings/wasm  (#[wasm_bindgen] thin wrapper) ← browser playground
```

---

## 1. `gdscript-session` — the shared core

A **URI-keyed** session over one long-lived `AnalysisHost` (so salsa's incremental cache survives
edits). It owns the URI→`FileId` interner the string-keyed clients need (the core stays `FileId`-based).

- **Lifecycle (mutating):** `open(uri, text, res_path?)` (records the `res://` path **once**, on first
  open — set-once so cross-file `preload`/`extends`/autoload resolution lights up without
  re-invalidating the path registry), `change(uri, text)` (upsert), `close(uri)` (removes; reopen ⇒
  fresh `FileId`), `set_project_config(text)` (autoload singletons).
- **Queries:** the full `Analysis` surface — `diagnostics`, `document_symbols`, `folding_ranges`,
  `inlay_hints`, `completions`, `hover`, `signature_help`, `code_actions`, `goto_definition`,
  `find_references`, `rename`, `workspace_symbols`, `syntax_tree` — each by `uri` (+ byte `offset`).
- **Cancellation:** the bindings are single-threaded (one JS thread), so no `apply_change` ever races
  a query; the `Cancellable` results never cancel and are unwrapped to their default.

---

## 2. Return-value strategy — JSON strings (decision + the validated alternative)

`Session` returns **JSON strings** of the engine-neutral `gdscript-base` POD (`"[]"`/`null`
fallbacks). The client `JSON.parse`s and maps byte offsets to its own encoding (UTF-16 in JS).

- **Why strings:** a single uniform return type across **both** bindings (napi *and* wasm are trivial
  delegators); the core POD types need only `serde::Serialize` (no `#[napi(object)]`, no POD
  re-declaration — the orphan rule forbids `#[napi(object)]` on out-of-crate types anyway); and
  `String` is the **safest** napi return type — which matters because the napi layer can't be
  validated locally (§0).
- **The research-recommended alternative (deferred, CI-validated):** napi-rs v3 implements
  `ToNapiValue for serde_json::Value` (with the `serde-json` feature) + `Env::to_js_value`, returning
  a **real JS object with no `JSON.parse`**. For our small per-keystroke payloads the perf difference
  is **negligible** (the napi-rs benchmark favoring it targeted *large* ASTs; its author's own
  guidance is "optimize for clean architecture over the micro-benchmark"). Adopting it means
  `Session` returns `serde_json::Value`, napi returns it directly, and wasm converts via
  `serde-wasm-bindgen` — a uniform-but-richer core. **Worth doing once the napi/wasm CI build lanes
  can validate the feature wiring** (the only reason it's deferred: it adds a napi feature + a wasm
  dep that can't be compile-checked on the dev box). Tracked in TECH_DEBT.

---

## 3. The napi layer (`gdscript-ffi`) — confirmed v3 specifics

- **Plain `#[napi]` impl + `#[napi]` on every exported method** (the impl-level attribute alone only
  processes annotated methods). snake_case → camelCase automatically (`open_document` → `openDocument`).
- **Synchronous methods.** Queries are sub-ms/low-ms; async (`AsyncTask`/`ThreadsafeFunction`) would
  only add tokio/libuv-pool/marshaling overhead. An LSP server is already off any UI thread. (oxc/SWC
  expose `parseSync` as first-class.)
- **No `Send`/`Sync` bound** is imposed on a `#[napi]` class; it holds the (Send, non-`Sync`) salsa
  state directly. The only place `Send` is forced is a `#[napi] async fn` holding state across
  `.await`, or capture into a `ThreadsafeFunction` — neither of which a sync binding uses.
- **`Option<String>` → JS `null`** for "nothing here" (`hover`/`signatureHelp`/`syntaxTree`);
  `Result`/`napi::Error` is reserved for genuine failures (we have none on the query path).
- **`build.rs` unchanged:** `napi_build::setup()`; `napi-build = "2"` is correct (the build crate
  stays major-2 in v3). `package.json` uses `binaryName` + a flat `targets` array (already scaffolded
  in `bindings/node`).

---

## 4. Testing

- **`gdscript-session`:** comprehensive `cargo test` (open/change/close lifecycle, unknown-URI safety,
  cross-file preload + autoload resolution end-to-end, rename envelope, reopen). Runs in `xtask ci`.
- **`gdscript-ffi`:** JS-side smoke test against the built `.node` (`napi build` provisions `libnode`;
  ava is napi-rs's default). The Rust is a thin delegation, so the core tests carry the logic
  coverage; the JS test proves the boundary wiring + camelCase surface.

---

## 5. Risks

| Risk | Sev | Mitigation |
|---|---|---|
| napi crate not `cargo test`able / not buildable on the dev box | High (process) | all logic in `gdscript-session` (testable); the `.node` build + JS test run in the napi CI lane. |
| Holding non-`Sync` salsa state in a `#[napi]` class | Low | sync methods only — no `Send` is ever forced (research-confirmed). |
| JSON-string parse cost on hot paths | Low | negligible at our payload sizes (research); `serde_json::Value` path documented as the upgrade. |
| Byte-offset vs JS UTF-16 indices | Med (client) | the binding passes **byte offsets** through; the CLIENT (guitkx adapter / playground) owns the byte↔UTF-16 map. Documented at the boundary. |
| Never-closed documents → memory growth | Low | `close(uri)` removes from host + interner; guitkx closes on `didClose`. |

**Biggest leverage:** the shared `gdscript-session` core — one tested unit serves napi *and* wasm,
and the bindings become near-trivial. **Biggest risk:** the napi layer's no-local-test gap, mitigated
by keeping it a thin delegator + the CI `.node` build.

## Sources (validated, 2026)
napi-rs v3 docs (concepts: values, async-task, reference, env; v2→v3 migration), docs.rs napi 3.9.x
(`ToNapiValue for serde_json::Value`), the napi-rs README (JS-side testing), oxc
`crates/oxc_parser` vs `napi/parser` (the core/binding split), Node "don't block the event loop".
