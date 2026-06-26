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
- **`index.html` is a minimal, build-less playground** (a textarea + diagnostics/hover/completions),
  which also serves as the binding's usage example. It owns the **UTF-8 byte ↔ UTF-16** conversion
  (the analyzer speaks byte offsets; the editor speaks UTF-16) — see the helpers in the page.
- **Pending (needs the local wasm toolchain / a browser to validate end-to-end):** the `pkg/` build
  (below) and a polished **Monaco/CodeMirror 6** editor (Playbook §4.1 — the textarea is the
  robust-but-plain stand-in; the editor is swappable without touching the analyzer glue).

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
