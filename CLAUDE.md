# gdscript-analyzer — repo guide for contributors (incl. AI assistants)

A Rust **library** that statically analyzes GDScript (Godot 4.x) — a "Roslyn for Godot".
It is engine-/protocol-neutral: queries take byte offsets and return plain data (POD); each
client (the LSP server, CLI, WASM playground, and the `guitkx` markup toolchain) maps results to
its own protocol. **A library, not a server.** The full plan lives in [`plans/`](plans/) — start at
[`plans/README.md`](plans/README.md). Current state: **Phase 0** (ecosystem/tooling) — stubs that
compile + a pipeline that runs; no analyzer features yet.

## Golden rules
- **Conventional Commits** on PR titles. Squash-merge turns the PR title into the commit that
  `release-plz` reads, so the title drives the version bump + changelog: `feat(syntax): …`,
  `fix(ide): …`, `docs: …`; `feat!:` / `BREAKING CHANGE:` for breaks. Scope = crate name.
- **Changesets** for the npm surface: any change affecting `@gdscript-analyzer/*` needs a
  `.changeset/*.md` (`pnpm changeset`). Rust-only changes don't.
- **ADRs**: an architecturally consequential decision lands as a numbered ADR in
  [`docs/src/adr/`](docs/src/adr/) in the same PR.
- **Portability is law.** The core crates (`base, syntax, api, db, hir, ide, scene`) MUST compile
  to `wasm32` — no `std::fs`, no `Instant::now()`/`SystemTime::now()`, no threads in the hot path.
  CI enforces `cargo check -p gdscript-ide --target wasm32-unknown-unknown` on every PR.
- **Never weaken a lint or test to make CI pass.** Fix the cause. Lints are denied in CI
  (`clippy -D warnings`); whitelist domain vocabulary in `clippy.toml` (`doc-valid-idents`) rather
  than sprinkling `#[allow]`.

## Workspace
`crates/gdscript-{base → syntax → api → db → hir → ide}` depend **only downward**; plus `scene`,
the `ffi` (napi-rs) binding, and the `lsp`/`cli` clients; `bindings/wasm`; `xtask` automation.
`gdscript-ide` is the public API (`AnalysisHost` / `Analysis`). `gdscript-api` is **generated** —
never hand-edit `crates/gdscript-api/src/generated.rs` (run `cargo xtask codegen-api`).
Architecture: [`plans/01-ARCHITECTURE.md`](plans/01-ARCHITECTURE.md).

## Commands
- `cargo xtask ci` — the full local gate (fmt + `clippy -D warnings` + test + wasm-check + deny).
  Run before every PR.
- `cargo xtask codegen-api` — regenerate the engine model from the vendored `extension_api.json`.
- `cargo xtask dist` / `fixtures` / `release --dry-run`.

## Syncing with Godot
Engine knowledge comes from `vendor/godot/<version>/extension_api.json`. The `godot-api-sync`
GitHub Action watches Godot releases, vendors the new dump, runs `codegen-api`, and opens a PR.
Runbook: [`plans/GODOT-SYNC.md`](plans/GODOT-SYNC.md).

## Build-environment note (Windows)
You need a linker: either install VS "Desktop development with C++" (MSVC), or
`rustup default stable-x86_64-pc-windows-gnu` (bundled mingw linker). Build from **PowerShell**,
not Git Bash — MSYS's `/usr/bin/link` shadows the real linker and produces confusing errors.

## Branches
Default branch `master` (protected). Integration branch `dev`. Feature branches → PR into `dev`.
