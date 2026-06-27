# Web playground (Phase 5)

Paste GDScript → live diagnostics / hover / completion entirely client-side, powered by the Rust
core compiled to WASM (the `gdscript-wasm` binding over the shared `gdscript-session`). No server, no
Godot, no install — the "reach" proof (modeled on Ruff's `play.ruff.rs`).

See [plans/PHASE-5-CLIENTS-AND-DISTRIBUTION.md](../plans/PHASE-5-CLIENTS-AND-DISTRIBUTION.md) §WS4 and
[plans/PHASE-5-NAPI-PLAYBOOK.md](../plans/PHASE-5-NAPI-PLAYBOOK.md) (the shared `Session` core).

## Status

- **The WASM binding is done + verified** (`bindings/wasm`, a thin `#[wasm_bindgen]` wrapper over the
  fully-tested `gdscript-session`; compiles + clippy-clean for `wasm32-unknown-unknown` in CI). It
  exposes the full URI-keyed query surface + `loadEngineApi`.
- **`index.html` is a build-less playground with a real [Monaco](https://microsoft.github.io/monaco-editor/)
  editor** (loaded from a CDN via Monaco's AMD loader — no bundler, so the GitHub-Pages deploy stays a
  static copy). It registers live **diagnostics** (inline squiggles via `setModelMarkers` + a side panel),
  a **hover** provider (the analyzer's inferred type + docs), a **completion** provider (triggered on
  `.`/`$`/`%` and Ctrl-Space), and **signature help** — all calling the WASM `Analyzer` directly. It owns
  the **UTF-8 byte ↔ UTF-16** conversion (the analyzer speaks byte offsets; Monaco speaks UTF-16) via the
  helpers in the page. The editor is swappable without touching the analyzer glue.
- **Verify in a browser** (the wasm calls are unit-tested in `gdscript-session`, but the Monaco glue —
  providers, the worker proxy, position conversion — is browser-only): build `pkg/` (below), serve, and
  exercise hover/completion/diagnostics. The live site is the Pages deploy.

## Build & serve

From the repo root (needs `wasm-pack`: `cargo install wasm-pack`):

```sh
# 1. Build the wasm package into playground/pkg/  (gdscript.js + gdscript_bg.wasm)
wasm-pack build --target web --out-dir ../../playground/pkg --out-name gdscript bindings/wasm

# 2. Provide the engine model (Button/Control/… completions). The rkyv blob ships in the repo;
#    the playground fetches ./data/extension_api.bin. (The brotli `.rkyv.br` variant — Playbook
#    §4.4 — is the size optimization; the raw .bin works for local dev.)
mkdir -p playground/data
cp crates/gdscript-api/src/engine_api.bin playground/data/extension_api.bin

# 3. Serve (any static server; ES modules need http://, not file://)
cd playground && python -m http.server 8080   # → http://localhost:8080
```

Diagnostics work without step 2; engine-class completion/hover need the engine model.

## Deploy

GitHub Pages (static): build `--profile wasm-release` (Playbook §4.5 — `opt-level="z"`, LTO,
`panic="abort"`, `strip`; `wasm-pack` runs `wasm-opt` on release), serve the data asset with
`Content-Encoding: br` + immutable cache headers. Single-threaded wasm-bindgen ⇒ **no COOP/COEP**.
