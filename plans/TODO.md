# TODO — deferred, integration-driven asks

Small follow-ups surfaced by **consumers** of `@gdscript-analyzer/core` (the napi binding). The analyzer
itself is feature-complete through Phase 6; these are additive binding/exposure gaps, parked here so we
don't reopen the analyzer mid-integration. Pick up when next touching this repo.

---

## 1. ✅ DONE (this change) — `format` / `formatRange` + `semanticTokens` exposed on the binding

Added the thin delegators over the existing engine features (`Analysis::format` / `format_range` /
`semantic_tokens` already lived in `gdscript-ide`): `Session::format` / `format_range` / `semantic_tokens`,
then `#[napi]` (node, `gdscript-ffi`) and `#[wasm_bindgen]` (browser, `bindings/wasm`) wrappers, plus the
`bindings/node/index.d.ts` declarations and README capability table (13 → 16 methods). Unblocks
analyzer-driven GDScript **formatting** + **semantic highlighting** for consumers (the ReactiveUI-Godot
guitkx LSP, both for `.gd` and embedded `.guitkx`). Ships in the next published `@gdscript-analyzer/core`.

---

## 2. (Watch) standalone `gdscript-lsp` as the primary `.gd` LSP for editors

**Why:** ReactiveUI-Godot is moving to "swap to our analyzer completely" — driving plain `.gd` files in
VS Code / VS 2022 (not just `.guitkx`-embedded), replacing `godot-tools`. That is mostly a *consumer-side*
wiring decision (documentSelector / shipping the `gdscript-lsp` binary), **no analyzer change required** —
`gdscript-lsp` already provides full `.gd` LSP incl. formatting + semantic tokens. Logged here only so the
analyzer side is aware it may become a first-class distributed artifact (packaging / per-platform binaries)
if that path is chosen. No action unless the consumer asks for a distribution change.
