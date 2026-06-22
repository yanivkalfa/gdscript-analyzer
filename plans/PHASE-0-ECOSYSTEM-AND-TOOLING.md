# PHASE 0 — Ecosystem & Tooling

> **The foundation. No analyzer features.** This phase stands up the repository, the cargo workspace, the
> build automation (`xtask`), CI, the release/versioning machinery, the engine-data pipeline, the docs &
> governance scaffold, and the Godot-sync automation. Everything in Phases 1–6 builds on what is laid down
> here. It is the owner's **top priority** ("tooling first, not phase 1").
>
> Obeys [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (crate layout, portability rules, FFI/WASM strategy) and
> [`ROADMAP.md`](ROADMAP.md) (Phase 0 deliverable + exit criteria). Evidence base:
> [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) (primary),
> [`research/01-rust-distribution-tooling.md`](research/01-rust-distribution-tooling.md),
> [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md). The Godot-sync *content*
> (codegen meaning, multi-version policy) lives in the sibling [`GODOT-SYNC.md`](GODOT-SYNC.md);
> this doc owns the **CI plumbing** that drives it.

---

## Goal & scope (Phase 0 = ecosystem foundation; no features)

**Goal.** Make `gdscript-analyzer` a *runnable, releasable, contributable* repository — with **zero analyzer
features** — such that:

- `cargo build` / `cargo test` / `cargo xtask ci` are green on Linux, macOS, and Windows.
- Every core crate compiles to `wasm32-unknown-unknown` (CI-enforced).
- A release dry-run (`release-plz`) and a changeset dry-run succeed without publishing.
- The Godot-sync workflow runs on dispatch, vendors `extension_api.json`, runs the codegen step, and opens a PR.
- A brand-new contributor can clone, build, test, and produce the napi + wasm artifacts following
  [Workstream G](#workstream-g--how-to-run-it-developer-onboarding).

**In scope:** virtual cargo workspace + all crate **stubs** (empty, compiling); `xtask` automation; GitHub
Actions (CI, release, docs, godot-sync); release-plz + Changesets + (later) cargo-dist; dual licensing +
third-party notices; mdBook + docs.rs config; governance files + ADRs; the `extension_api.json` vendor +
`xtask codegen-api` data pipeline (first checked-in version).

**Explicitly NOT in scope** (deferred to later phases): the lexer/parser (Phase 1), `extension_api.json`
*semantic* modelling (Phase 2), salsa (Phase 3), `.tscn` parsing (Phase 4), the LSP server / playground /
crates.io+npm GA (Phase 5). Phase 0 ships **stubs that compile and a pipeline that runs**, nothing more.

**Definition of "stub":** a crate whose `lib.rs` is (at most) a doc comment + a trivial smoke test, whose
`Cargo.toml` declares the correct **dependency edges** and metadata, and which participates in the workspace
build/lint/test/wasm-check. No domain logic.

---

## Prerequisites / assumptions (toolchain)

| Tool | Version / pin | Why | Where pinned |
|---|---|---|---|
| **Rust toolchain** | `stable` channel, pinned in `rust-toolchain.toml`; bump deliberately | One reproducible toolchain across dev + CI; feeds the `rust-cache` key | `rust-toolchain.toml` |
| **MSRV** | **1.88.0** (napi-rs v3's floor — [`research/01` §2.1](research/01-rust-distribution-tooling.md)) | The whole workspace must build on the binding's floor; CI asserts it | `rust-version` in `[workspace.package]` |
| **rustfmt / clippy** | components of the pinned toolchain | `cargo fmt --check`, `cargo clippy -D warnings` | `rust-toolchain.toml` `components` |
| **wasm target** | `wasm32-unknown-unknown` (browser core check) | Portability guard ([`01` §7](01-ARCHITECTURE.md)) | `rust-toolchain.toml` `targets` + CI |
| **wasm (binding)** | `wasm32-wasip1-threads` (napi-rs v3 wasm output) | Binding wasm build (stubbed until `gdscript-ffi` exists) | CI matrix only |
| **Node.js** | **20 / 22 / 24** LTS line; dev on ≥20 | napi binding build + tests; Changesets CLI | `.github/workflows`, `bindings/node/package.json` `engines` |
| **package manager (JS)** | **pnpm** (matches oxc; lockfile committed) | Per-platform npm package workspace | `bindings/node/`, `package.json` `packageManager` |
| **`@napi-rs/cli`** | v3 (`napi` CLI) | Build the `.node` addon + wasm target | `bindings/node/package.json` devDep |
| **Godot binary** | a **stable** Godot ≥ 4.x (for the dump *fallback* path) | `godot --headless --dump-extension-api` is the **fallback**; default source is godot-cpp's committed JSON (no binary needed) | Workstream E |
| **cargo plugins** | `cargo-llvm-cov`, `cargo-deny`, `cargo-hack` (MSRV), optional `cargo-msrv`, `cargo-about` | Coverage, license/advisory gate, MSRV matrix, attribution | installed in CI via `taiki-e/install-action` |
| **release tooling** | `release-plz`, `git-cliff` (embedded), Changesets, later `cargo-dist` | Versioning / changelog / publish | Workstream D |
| **docs** | `mdbook`, `mdbook-linkcheck` | Guide site | Workstream F |

**Assumptions:** repo root is `C:/Yanivs/GameDev/gdscript-analyzer`, currently empty (only `.git`, branch
`main`, no commits). **Default branch is `main`** (canonical context) — all workflows, branch protection, and
release config use `main`. Internal workspace crate names use the **`gdscript-`** prefix; the npm scope is
**`@gdscript-analyzer/*`**; license is **`MIT OR Apache-2.0`**; SemVer 0.x single shared version starting
**`0.1.0`**.

---

## Workstream A — Repo & cargo workspace skeleton

### A.1 The directory tree

Flat `crates/*` virtual workspace (matklad's "Large Rust Workspaces"), `bindings/{node,wasm}` split (swc),
per-platform npm packaging under `bindings/node/npm/*` (napi-rs), `xtask/` build system (rust-analyzer). This
is the canonical tree from [`01-ARCHITECTURE.md` §8](01-ARCHITECTURE.md), expanded.

| Path | Phase-0 state | Purpose |
|---|---|---|
| `Cargo.toml` | created | Virtual workspace manifest (no `[package]`); `workspace.dependencies`, `workspace.lints`, `workspace.package`, profiles |
| `Cargo.lock` | committed | Workspace has binaries (`xtask`, later `cli`/`lsp`) → lock is committed |
| `rust-toolchain.toml` | created | Pin channel + components + targets |
| `rustfmt.toml` | created | Formatting config |
| `clippy.toml` | created | Clippy knobs (MSRV mirror, thresholds) |
| `deny.toml` | created | `cargo-deny`: license allow-list + advisories + bans |
| `release-plz.toml` | created | Release-plz workspace + changelog config |
| `cliff.toml` | created | git-cliff config (Keep-a-Changelog groupings) — referenced by release-plz |
| `.changeset/config.json` | created | Changesets config (npm side) |
| `.cargo/config.toml` | created | `[alias] xtask = "run --package xtask --"` |
| `.gitignore` | created | `/target`, `node_modules`, `pkg/`, `*.node`, `dist/`, mdBook `book/`, etc. |
| `.editorconfig` | created | Cross-editor whitespace/charset rules |
| `crates/gdscript-base/` | **stub** | POD types (`FileId`, `TextSize`/`TextRange`, `LineIndex`) — empty |
| `crates/gdscript-syntax/` | **stub** | Lexer/CST/parser (Phase 1) — empty |
| `crates/gdscript-api/` | **stub** | Engine model + the codegen **output** lands here (Phase 2 fills it) |
| `crates/gdscript-db/` | **stub** | VFS / project model (Phase 3) — empty |
| `crates/gdscript-hir/` | **stub** | Semantic layer (Phase 2/3) — empty |
| `crates/gdscript-ide/` | **stub** | Public `AnalysisHost`/`Analysis` API; **the wasm-check target** |
| `crates/gdscript-scene/` | **stub** | `.tscn`/`.tres` parser (Phase 4) — empty |
| `crates/gdscript-ffi/` | **stub** | The only napi/wasm crate (Phase 1 wires it) — empty `cdylib`+`lib` |
| `crates/gdscript-lsp/` | **stub** | LSP server binary (Phase 5) — empty `main` |
| `crates/gdscript-cli/` | **stub** | CLI binary (Phase 5) — empty `main` |
| `bindings/node/` | **scaffold** | napi-rs npm package: `package.json`, `index.js` loader, generated `index.d.ts`, `npm/<triple>/` dirs |
| `bindings/wasm/` | **scaffold** | wasm-bindgen fallback package (`package.json`, placeholder build script) |
| `xtask/` | created | `cargo xtask` binary crate (commands: `ci`, `codegen-api`, `fixtures`, `dist`, `release`) |
| `vendor/godot/<version>/` | seeded | Vendored `extension_api.json` + doc XML per Godot version |
| `fixtures/parser/` | empty (`.gitkeep`) | Golden parse trees (Phase 1) |
| `fixtures/ide/` | empty (`.gitkeep`) | Feature scenario fixtures (Phase 2+) |
| `docs/` | scaffold | mdBook (`book.toml` + `src/SUMMARY.md` + `src/adr/`) |
| `playground/` | placeholder | Web playground (Phase 5) — `README.md` placeholder only |
| `benches/`, `fuzz/`, `tests/` | optional `.gitkeep` | criterion / cargo-fuzz / cross-crate integration (populated later) |
| `LICENSE-MIT`, `LICENSE-APACHE` | created | Dual license |
| `THIRD-PARTY-NOTICES.md` | created | Godot + tree-sitter-gdscript attribution |
| `README.md` | created | Badges, quickstart, consume-from-Rust/Node/browser |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `GOVERNANCE.md`, `SUPPORT.md` | created | Governance |
| `.github/workflows/*.yml` | created | `ci`, `release-plz`, `release-napi`, `release-wasm`, `docs`, `godot-api-sync` |
| `.github/ISSUE_TEMPLATE/*.yml`, `PULL_REQUEST_TEMPLATE.md`, `dependabot.yml` | created | Issue forms + PR template + dependabot |

```
gdscript-analyzer/
├── Cargo.toml                      # virtual workspace (no [package])
├── Cargo.lock                      # committed
├── rust-toolchain.toml
├── rustfmt.toml  clippy.toml  deny.toml
├── release-plz.toml  cliff.toml
├── .changeset/config.json
├── .cargo/config.toml              # [alias] xtask = "run --package xtask --"
├── .gitignore  .editorconfig
├── crates/
│   ├── gdscript-base/   gdscript-syntax/  gdscript-api/  gdscript-db/
│   ├── gdscript-hir/    gdscript-ide/     gdscript-scene/
│   ├── gdscript-ffi/    gdscript-lsp/     gdscript-cli/
├── bindings/
│   ├── node/                       # napi-rs package
│   │   ├── Cargo.toml  build.rs  src/lib.rs
│   │   ├── package.json  index.js  index.d.ts
│   │   └── npm/                     # per-platform optionalDependencies
│   │       ├── darwin-x64/   darwin-arm64/
│   │       ├── linux-x64-gnu/   linux-x64-musl/   linux-arm64-gnu/   linux-arm64-musl/
│   │       ├── linux-arm-gnueabihf/
│   │       └── win32-x64-msvc/   win32-arm64-msvc/
│   └── wasm/                        # wasm-bindgen fallback
│       └── Cargo.toml  src/lib.rs  package.json
├── xtask/                          # src/main.rs (ci|codegen-api|fixtures|dist|release)
├── vendor/godot/<version>/         # extension_api.json + doc/classes/*.xml
├── fixtures/{parser,ide}/
├── docs/                           # book.toml + src/SUMMARY.md + src/adr/
├── playground/                     # Phase-5 placeholder
├── benches/  fuzz/  tests/
├── LICENSE-MIT  LICENSE-APACHE  THIRD-PARTY-NOTICES.md
├── README.md  CONTRIBUTING.md  CODE_OF_CONDUCT.md  SECURITY.md  GOVERNANCE.md  SUPPORT.md
└── .github/
    ├── workflows/{ci,release-plz,release-napi,release-wasm,docs,godot-api-sync}.yml
    ├── ISSUE_TEMPLATE/{01-bug-report.yml,02-feature-or-diagnostic.yml,03-proposal.yml,config.yml}
    ├── PULL_REQUEST_TEMPLATE.md
    └── dependabot.yml
```

### A.2 Root `Cargo.toml` (virtual workspace)

```toml
[workspace]
resolver = "3"
members  = ["crates/*", "bindings/node", "bindings/wasm", "xtask"]

# Shared metadata inherited by every crate via `*.workspace = true`.
[workspace.package]
version      = "0.1.0"                     # single shared version (§D)
edition      = "2024"
rust-version = "1.88.0"                    # MSRV = napi-rs v3 floor
license      = "MIT OR Apache-2.0"
repository   = "https://github.com/yanivkalfa/gdscript-analyzer"
homepage     = "https://github.com/yanivkalfa/gdscript-analyzer"
authors      = ["Yaniv Kalfa <yanivkalfa@gmail.com>"]
categories   = ["development-tools", "parser-implementations"]
keywords     = ["gdscript", "godot", "analyzer", "lsp", "language-server"]

# Single source of truth for dependency versions; crates opt in with `{ workspace = true }`.
[workspace.dependencies]
# internal crates (path + version so they publish correctly)
gdscript-base   = { path = "crates/gdscript-base",   version = "0.1.0" }
gdscript-syntax = { path = "crates/gdscript-syntax", version = "0.1.0" }
gdscript-api    = { path = "crates/gdscript-api",    version = "0.1.0" }
gdscript-db     = { path = "crates/gdscript-db",     version = "0.1.0" }
gdscript-hir    = { path = "crates/gdscript-hir",    version = "0.1.0" }
gdscript-ide    = { path = "crates/gdscript-ide",    version = "0.1.0" }
gdscript-scene  = { path = "crates/gdscript-scene",  version = "0.1.0" }
# third-party (pinned centrally; actual use lands in later phases)
serde       = { version = "1",  features = ["derive"] }
serde_json  = "1"
# (Phase 1+) logos, cstree, rkyv, salsa, anyhow, etc. added here when first used.

# Lints applied workspace-wide; crates opt in with `[lints] workspace = true`.
[workspace.lints.rust]
unsafe_code            = "warn"            # ffi/wasm crates locally `#![allow]`
missing_debug_implementations = "warn"
rust_2024_compatibility = "warn"

[workspace.lints.clippy]
all      = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# pragmatic opt-outs:
module_name_repetitions = "allow"
missing_errors_doc      = "allow"

# ---- profiles ----
[profile.release]
lto           = true
codegen-units = 1

# A dedicated size-optimized profile for the wasm artifacts (§08). Build wasm with
# `--profile wasm-release`. Keep `release` fast for native LSP/CLI.
[profile.wasm-release]
inherits      = "release"
opt-level     = "z"        # measure "z" vs "s" per artifact (08 §4.1)
lto           = true
codegen-units = 1
panic         = "abort"
strip         = true
```

> **Note on `[lints] workspace = true`:** each crate adds a `[lints]` section pointing at the workspace so
> the `-D warnings` policy is uniform. Clippy is *denied* in CI (`-D warnings`), warned locally.

### A.3 Crate stubs (Cargo.toml + lib.rs) and the dependency edges

Dependency edges (each crate depends **only downward**, per [`01` §1](01-ARCHITECTURE.md)):

| Crate | `[dependencies]` at Phase 0 | crate-type | Phase-0 `lib.rs`/`main.rs` |
|---|---|---|---|
| `gdscript-base` | — | lib | doc comment + `#[test] fn smoke(){}` |
| `gdscript-syntax` | `gdscript-base` | lib | doc comment + smoke test |
| `gdscript-api` | `gdscript-base` | lib | doc comment; will `include!` the codegen output (`OUT_DIR` or `src/generated/`) in Phase 2 |
| `gdscript-db` | `gdscript-base`, `gdscript-syntax`, `gdscript-api` | lib | doc comment + smoke test |
| `gdscript-hir` | `gdscript-base`, `gdscript-syntax`, `gdscript-api`, `gdscript-db` | lib | doc comment + smoke test |
| `gdscript-ide` | all of the above | lib | doc comment + smoke test (**this is the wasm-check crate**) |
| `gdscript-scene` | `gdscript-base` | lib | doc comment + smoke test |
| `gdscript-ffi` | `gdscript-ide` (+ napi/wasm deps in Phase 1) | `["cdylib","lib"]` | empty re-export shell; `publish = false` |
| `gdscript-lsp` | `gdscript-ide` | bin | `fn main() {}`; `publish = false` until Phase 5 |
| `gdscript-cli` | `gdscript-ide` | bin | `fn main() {}`; `publish = false` until Phase 5 |
| `bindings/node` | `gdscript-ffi` (or `gdscript-ide`), `napi`, `napi-derive`; `napi-build` build-dep | `["cdylib"]` | `publish = false` (npm-only) |
| `bindings/wasm` | `gdscript-ide`, `wasm-bindgen`, `serde-wasm-bindgen` | `["cdylib","rlib"]` | `publish = false` (npm-only) |
| `xtask` | `anyhow`, `xshell` (or `std::process`), `serde_json` | bin | command dispatcher |

Example stub — `crates/gdscript-ide/Cargo.toml`:

```toml
[package]
name        = "gdscript-ide"
description = "Public AnalysisHost/Analysis API for the gdscript-analyzer (POD, protocol-neutral)."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true

[dependencies]
gdscript-base   = { workspace = true }
gdscript-syntax = { workspace = true }
gdscript-api    = { workspace = true }
gdscript-db     = { workspace = true }
gdscript-hir    = { workspace = true }

[lints]
workspace = true

[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]
```

Example stub — `crates/gdscript-ide/src/lib.rs`:

```rust
//! gdscript-ide — the public `AnalysisHost` / `Analysis` surface (Phase 1+).
//!
//! Phase 0: empty, compiling stub. CI-enforced wasm-buildable
//! (`cargo check -p gdscript-ide --target wasm32-unknown-unknown`).
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(2 + 2, 4);
    }
}
```

The `gdscript-ffi`, `bindings/node`, and `bindings/wasm` stubs declare `publish = false` and the right
`crate-type`; their *actual* binding surface is wired in Phase 1 (this doc only lays the structure so the CI
napi/wasm jobs have a real — if empty — target to build).

### A.4 `.cargo/config.toml`

```toml
[alias]
xtask = "run --package xtask --"

# Optional: a convenience alias mirroring CI's wasm guard.
wasm-check = "check -p gdscript-ide --target wasm32-unknown-unknown"
```

### A.5 `rust-toolchain.toml`

```toml
[toolchain]
channel    = "stable"          # bump deliberately; this string is part of the CI cache key
components = ["rustfmt", "clippy", "rust-src"]
targets    = ["wasm32-unknown-unknown"]
profile    = "minimal"
```

### A.6 `rustfmt.toml`, `clippy.toml`

`rustfmt.toml`:
```toml
edition = "2024"
max_width = 100
# (keep minimal; expand only with a recorded rationale)
```

`clippy.toml`:
```toml
msrv = "1.88.0"                 # mirror the workspace MSRV so clippy lints match the floor
```

### A.7 `.gitignore`, `.editorconfig`

`.gitignore` (essentials):
```gitignore
/target
**/node_modules
**/pkg/
*.node
/dist
/docs/book
.DS_Store
*.log
```
> **Committed (NOT ignored):** `Cargo.lock`, `bindings/node/index.d.ts` (generated but reviewed),
> `vendor/godot/**` (the vendored API data), `.changeset/*.md`.

`.editorconfig`:
```ini
root = true
[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
[*.rs]
indent_size = 4
[*.{toml,yml,yaml,json,js,ts,md}]
indent_size = 2
[*.md]
trim_trailing_whitespace = false
```

---

## Workstream B — xtask automation

**Pattern.** `xtask` is a regular binary crate in the workspace, invoked as `cargo xtask <cmd>` via the
`.cargo/config.toml` alias. It runs arbitrary Rust — no `make`/`bash`/`python` dependency, cross-platform by
construction ([`research/07` §1.3](research/07-ecosystem-and-release-tooling.md),
[`research/01` §7.3](research/01-rust-distribution-tooling.md)). `xtask/src/main.rs` matches on the first
positional arg and dispatches. Adopters: Cargo, rust-analyzer, helix.

```rust
// xtask/src/main.rs (shape)
fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "ci"          => tasks::ci(),            // run the full local CI gate
        "codegen-api" => tasks::codegen_api(),   // extension_api.json -> gdscript-api data
        "fixtures"    => tasks::fixtures(),      // (re)generate golden parser fixtures (Phase 1)
        "dist"        => tasks::dist(),          // build napi + wasm artifacts locally
        "release"     => tasks::release(),       // local release helpers / dry-run
        other         => anyhow::bail!("unknown xtask: {other:?}"),
    }
}
```

| Command | Phase-0 status | What it does |
|---|---|---|
| `cargo xtask ci` | **real** | The one-shot local gate that mirrors `ci.yml`: `cargo fmt --all -- --check` → `cargo clippy --all-targets --all-features -- -D warnings` → `cargo test --workspace` → `cargo check -p gdscript-ide --target wasm32-unknown-unknown` → `cargo deny check`. Exit non-zero on any failure. **This is the command the exit criteria reference.** |
| `cargo xtask codegen-api` | **real (pipeline), trivial transform** | Reads the newest `vendor/godot/<version>/extension_api.json`, runs the Phase-0 transform (validate JSON parses + emit a minimal versioned data artifact — see [Workstream E](#workstream-e--engine-data-bootstrap-extension_apijson)), writes it where `gdscript-api` consumes it. Phase 2 grows this into the full rkyv/serde model. Invoked by the godot-sync workflow. |
| `cargo xtask fixtures` | **stub** | Will (re)generate `fixtures/parser/*.ast` golden trees from `*.gd` inputs in Phase 1. Phase 0: prints "no fixtures yet" and exits 0 (a real no-op so CI/tests can call it). |
| `cargo xtask dist` | **real-ish** | Orchestrates local artifact builds: `napi build --platform --release` (in `bindings/node`) and the wasm build (`napi build … --target wasm32-wasip1-threads` *or* `wasm-pack build bindings/wasm --target web`). Phase 0: builds the empty binding stubs to prove the toolchain end-to-end. |
| `cargo xtask release` | **stub/helper** | Local release ergonomics: a `--dry-run` that calls `release-plz update --dry-run` and reports the would-be version bump + changelog; checks crates.io/npm version parity. Actual publishing is done by CI ([Workstream D](#workstream-d--release-versioning-changelog)), never locally. |

**Why a `dist` and a `codegen-api` command specifically:** the godot-sync workflow shells out to
`cargo xtask codegen-api`, and a contributor's local build of the npm/wasm packages must be a single command
on any OS — both are exactly the cross-platform-Rust-not-bash use cases `xtask` exists for.

---

## Workstream C — CI (GitHub Actions)

All Rust jobs install the toolchain via `dtolnay/rust-toolchain` (or read `rust-toolchain.toml`) **before**
`Swatinem/rust-cache@v2` (the cache key hashes the toolchain + `.cargo/config.toml`). Tool installs use
`taiki-e/install-action`. The lint/test core gates behind `fmt` via `needs:`.

### C.1 `ci.yml` — the core gate (runs on every PR + push to `main`)

| Job | Runner OS | Rust target | Command | `needs` |
|---|---|---|---|---|
| `fmt` | `ubuntu-latest` | stable | `cargo fmt --all -- --check` | — |
| `clippy` | `ubuntu-latest` | stable | `cargo clippy --all-targets --all-features -- -D warnings` | `fmt` |
| `test (linux)` | `ubuntu-latest` | `x86_64-unknown-linux-gnu` | `cargo test --workspace` | `fmt` |
| `test (macos)` | `macos-latest` | `aarch64-apple-darwin` | `cargo test --workspace` | `fmt` |
| `test (windows)` | `windows-latest` | `x86_64-pc-windows-msvc` | `cargo test --workspace` | `fmt` |
| `msrv` | `ubuntu-latest` | pinned `1.88.0` | `cargo hack check --rust-version --workspace` | `fmt` |
| `wasm` | `ubuntu-latest` | `wasm32-unknown-unknown` | `cargo check -p gdscript-ide --target wasm32-unknown-unknown` | `fmt` |
| `coverage` | `ubuntu-latest` | stable | `cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info` → `codecov/codecov-action@v5` | `fmt` |
| `deny` | `ubuntu-latest` | stable | `cargo deny check` (licenses + advisories + bans) | `fmt` |
| `pr-title` | `ubuntu-latest` | — | `amannn/action-semantic-pull-request` (Conventional-Commit PR titles) | — (PR events only) |

**Key per-job notes** ([`research/07` §2.2](research/07-ecosystem-and-release-tooling.md)):
- `clippy` and `fmt` are **deny-on-warning** — `-D warnings` / `--check`.
- `wasm` is the **portability guard** from [`01` §7.5](01-ARCHITECTURE.md) — the single most important Phase-0
  invariant after "it compiles." It checks `gdscript-ide` (the public surface) on the **browser** target.
- `msrv` uses `cargo-hack` to check every crate against the manifest `rust-version`.
- `coverage` via `cargo-llvm-cov` → Codecov; non-blocking at Phase 0 (no real code yet) but wired so it grows.
- Add `Swatinem/rust-cache@v2` to every Rust job; cache is keyed per OS/target.

### C.2 `release-napi.yml` — napi cross-compile matrix (structure laid out; build stubbed until `gdscript-ffi` is real)

Mirrors the napi-rs package-template + oxc-resolver matrix. **Phase 0:** the binding compiles an empty
`#[napi]`-free stub, so these jobs *build and upload artifacts of an empty addon* — proving the cross-compile
toolchain works before there is anything to bind.

| Host runner | Target triple | Build approach |
|---|---|---|
| `macos-latest` | `x86_64-apple-darwin` | native |
| `macos-latest` | `aarch64-apple-darwin` | native |
| `windows-latest` | `x86_64-pc-windows-msvc` | native |
| `windows-latest` | `aarch64-pc-windows-msvc` | native |
| `ubuntu-latest` | `x86_64-unknown-linux-gnu` | `napi build --use-napi-cross` (zig) |
| `ubuntu-latest` | `x86_64-unknown-linux-musl` | `napi build -x` (cargo-zigbuild) |
| `ubuntu-latest` | `aarch64-unknown-linux-gnu` | `--use-napi-cross` |
| `ubuntu-latest` | `aarch64-unknown-linux-musl` | `-x` |
| `ubuntu-latest` | `armv7-unknown-linux-gnueabihf` | `--use-napi-cross` |
| `ubuntu-latest` | `wasm32-wasip1-threads` | napi wasm build |

Each job uploads `bindings/node/*.node` as `bindings-<target>`; a publish job downloads all, runs
`napi artifacts`, and `npm publish --provenance` — **gated on a release event only** (§D). zig via
`mlugg/setup-zig`, `cargo-zigbuild` via `taiki-e/install-action`, ARM test emulation via
`docker/setup-qemu-action`. Triggered on `workflow_dispatch` (Phase 0) and release tags (Phase 5).

### C.3 `release-wasm.yml` — wasm npm package (stubbed until bindings exist)

One `ubuntu-latest` job. **Route A (default):** reuse the napi crate → `napi build --target
wasm32-wasip1-threads`. **Route B (fallback):** `wasm-pack build bindings/wasm --target web --profile
wasm-release`, plus `--target bundler` / `--target nodejs` variants, then `wasm-pack pack`/`publish`. Decision
deferred to Phase 5 per measured bundle size ([`01` §4](01-ARCHITECTURE.md),
[`research/08` §3](research/08-wasm-web-and-bindings.md)). Phase 0 wires both as `workflow_dispatch`-only,
building the empty stub.

### C.4 The other workflows

- **`release-plz.yml`** — Workstream D (crate Release-PR + crates.io publish via OIDC).
- **`docs.yml`** — Workstream F (mdBook → GitHub Pages).
- **`godot-api-sync.yml`** — Workstream E (the scheduled Godot watcher).
- **`dependabot.yml`** — `cargo` + `github-actions` + `npm` ecosystems, weekly.

**Merge queue + branch protection** ([Workstream D](#workstream-d--release-versioning-changelog) details the
git flow): protect `main`, require `fmt`/`clippy`/`test (*)`/`wasm`/`deny` as status checks, squash-merge only,
enable GitHub's native merge queue.

---

## Workstream D — Release, versioning, changelog

### D.1 The toolchain (one number, two registries)

| Tool | Registry | Role | Config file | Adopt? |
|---|---|---|---|---|
| **release-plz** | crates.io | Release-PR: bump `Cargo.toml`, git-cliff changelog, `cargo-semver-checks`, tag, `cargo publish` (OIDC) | `release-plz.toml` (+ `cliff.toml`) | **yes — the Rust engine** |
| **Changesets** | npm | `.changeset/*.md` intent → "Version Packages" PR → `npm publish --provenance` | `.changeset/config.json` | **yes — the npm engine** |
| **git-cliff** | — | Keep-a-Changelog from Conventional Commits | `cliff.toml` (consumed *inside* release-plz) | indirect |
| **cargo-dist (`dist`)** | GitHub Releases | Cross-platform CLI binaries + installers, keyed off the release tag | `dist-workspace.toml` (later) | **later** (only when `gdscript-cli` ships, Phase 5) |
| **SemVer 2.0.0 (Cargo 0.x reading)** | spec | Versioning contract | — | yes |
| **Conventional Commits 1.0.0** | spec | Commit/PR-title grammar → bump | — | yes |
| **Keep a Changelog 1.1.0** | spec | Changelog format | (in `cliff.toml`) | yes |

### D.2 The single-shared-version policy (crates.io + npm in lockstep)

- **One version across all crates + all npm packages**, starting `0.1.0`. Cargo and npm interpret `0.x` carets
  identically, so one number behaves the same on both registries
  ([`research/07` §3.3](research/07-ecosystem-and-release-tooling.md)).
- **release-plz is the source of truth.** It computes the bump from Conventional Commits and runs
  `cargo-semver-checks`. A **one-line sync step** copies that version into each `package.json` before npm
  publish (release-plz has no `package.json` awareness; Changesets has no `Cargo.toml` awareness — the glue is
  explicit):
  ```bash
  v=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].version')
  # in each bindings/*/npm package dir:
  npm version "$v" --no-git-tag-version
  ```
- **0.x bump mapping:** `fix` → patch; `feat` → patch *while in 0.x* (configure release-plz so `feat` does not
  force a 0.x minor); `!`/`BREAKING CHANGE` → minor (the 0.x "major"). Reach `1.0.0` when both the Rust API and
  the napi/wasm surface are stable and depended upon.

### D.3 Config files

**`release-plz.toml`** (workspace, single-version):
```toml
[workspace]
# one shared version across the workspace; release-plz updates them in lockstep
allow_dirty           = false
changelog_update      = true
git_release_enable    = true
semver_check          = true
pr_branch_prefix      = "release-plz/"
# treat `feat` as patch while we are 0.x (so feat doesn't bump the 0.x "major")
# (set per-package / via the [[package]] override or the changelog/version_group as release-plz evolves)

[changelog]
# delegate formatting to cliff.toml (Keep-a-Changelog groupings)
```

**`cliff.toml`** (Keep-a-Changelog 1.1.0 groupings, Conventional-Commit parsers):
```toml
[changelog]
header = "# Changelog\n\nAll notable changes follow Keep a Changelog and SemVer.\n"
trim = true

[git]
conventional_commits = true
filter_unconventional = false
commit_parsers = [
  { message = "^feat",     group = "Added" },
  { message = "^fix",      group = "Fixed" },
  { message = "^perf",     group = "Changed" },
  { message = "^refactor", group = "Changed" },
  { message = "^docs",     group = "Documentation" },
  { message = "^deprecat", group = "Deprecated" },
  { message = "^remove",   group = "Removed" },
  { message = "^security", group = "Security" },
  { body = ".*BREAKING CHANGE", group = "Changed" },
  { message = "^chore|^ci|^build|^test", skip = true },
]
```

**`.changeset/config.json`** (npm side; lock the binding + per-platform + wasm packages into one `fixed`
group so they version together):
```json
{
  "$schema": "https://unpkg.com/@changesets/config@3/schema.json",
  "changelog": "@changesets/changelog-github",
  "commit": false,
  "access": "public",
  "baseBranch": "main",
  "fixed": [["@gdscript-analyzer/*"]],
  "linked": [],
  "updateInternalDependencies": "patch"
}
```

### D.4 Git flow (trunk-based)

- **Protected `main`**, short-lived branches, **squash-merge with "Default to PR title for squash merge
  commits"** so the Conventional-Commit PR title becomes the single `main` commit release-plz reads.
- **PR-title linting** via `amannn/action-semantic-pull-request` (the `pr-title` CI job).
- **Conventional Commits 1.0.0** — types `feat|fix|perf|refactor|docs|test|build|ci|chore|revert`, crate names
  as scopes (`feat(syntax): …`), `!`/`BREAKING CHANGE:` for breaks.
- **GitHub native merge queue** (the "Not Rocket Science Rule" in GitHub-native form): tests each PR against
  the merged result so `main` stays green.
- **npm caveat:** Changesets reads `.changeset/*.md`, not commits — **require a changeset for user-facing npm
  changes** even though the Rust side derives bumps from commits.

### D.5 Release runbook (the concrete steps)

1. **PR opened** with a Conventional-Commit title; if it touches the npm surface, author adds a
   `.changeset/<name>.md`.
2. CI + merge queue keep `main` green → **squash-merge** (title → commit).
3. **`release-plz.yml`** on push to `main` opens/updates the **Rust Release PR** (bump + changelog +
   `cargo-semver-checks`); **Changesets action** opens/updates the **"Version Packages" PR** for npm.
4. **Merge the Release PR** → `release-plz release` tags `v<version>` + `cargo publish` to crates.io via
   **Trusted Publishing / OIDC** (no token); exposes `releases_created`.
5. **npm publish job** (gated `if: …releases_created == 'true'`): run the §D.2 sync step, then
   `release-napi.yml` + `release-wasm.yml` build artifacts and `npm publish --provenance` (OIDC).
6. **(Phase 5) cargo-dist** keyed off the same `v<version>` tag builds CLI binaries/installers onto the GitHub
   Release.
7. **One tag per release** (`v<version>`).

**Phase-0 acceptance = a *dry-run* of all of the above succeeds** (`release-plz update --dry-run` via
`cargo xtask release --dry-run`; `changeset status`) **without publishing anything.**

---

## Workstream E — Engine data bootstrap (`extension_api.json`)

> The **content/meaning** of the API delta, the multi-version policy, BBCode→Markdown doc conversion, and the
> full codegen model live in [`GODOT-SYNC.md`](GODOT-SYNC.md) and [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md).
> Phase 0 establishes the **pipeline + a first checked-in version** only.

### E.1 Vendor the initial data

Layout (versioned, per [`01` §5](01-ARCHITECTURE.md)):
```
vendor/godot/
└── 4.4.1-stable/                 # one dir per bundled Godot version
    ├── extension_api.json        # the dump
    └── doc/classes/*.xml         # per-class doc XML (hover source; BBCode)
```

**Where the JSON comes from** (two sources; the workflow uses the first, the binary path is the fallback):

- **(A, default) godot-cpp's committed copy** — no Godot binary required:
  ```
  https://raw.githubusercontent.com/godotengine/godot-cpp/<TAG>/gdextension/extension_api.json
  ```
  e.g. `…/godot-cpp/4.4.1-stable/gdextension/extension_api.json`. godot-cpp is MIT and ingests the same
  artifact ([`research/07` §8.2](research/07-ecosystem-and-release-tooling.md)).
- **(B, fallback) the Godot binary dump:** download the matching release binary and run
  `godot --headless --dump-extension-api` (and, for hover, the doc XML lives in the Godot source tree under
  `doc/classes/`). Use this when godot-cpp lags a tag or for doc XML the JSON omits.

**Phase-0 action:** check in **one** version (a current stable, e.g. `4.4.1-stable`) under
`vendor/godot/<version>/`, with the corresponding `doc/classes/*.xml`. Record the source + tag in a
`vendor/godot/<version>/SOURCE.txt`.

### E.2 The `xtask codegen-api` step

```
cargo xtask codegen-api
  ├─ locate newest vendor/godot/<version>/extension_api.json
  ├─ validate it parses (serde_json) and the version header is present
  ├─ (Phase 0) emit a MINIMAL versioned data artifact gdscript-api consumes:
  │     a small generated module (src/generated/api_meta.rs) OR a rkyv/postcard blob,
  │     carrying at least { godot_version, class_count } as a smoke artifact
  └─ write it where gdscript-api includes it (build.rs OUT_DIR or src/generated/)
```

- **Phase 0 transform is deliberately trivial** (validate + emit version/count) — it proves the
  *pipeline* end-to-end: vendored JSON → `xtask` → an artifact `gdscript-api` compiles against.
- **Phase 2** grows this into the full model (classes, inheritance, methods, properties, signals, enums,
  singletons, utility functions, builtins) serialized to **rkyv** (zero-copy, for the wasm data-shipping path)
  or serde, plus the hand-authored GDScript layer the dump omits — all per
  [`01` §5](01-ARCHITECTURE.md) / [`GODOT-SYNC.md`](GODOT-SYNC.md).
- The godot-sync workflow calls this step so a sync PR carries **both** the raw JSON delta **and** the
  regenerated artifact for review.

### E.3 The godot-sync workflow (CI plumbing)

`.github/workflows/godot-api-sync.yml` — scheduled + dispatchable; resolves the latest stable Godot tag, fetches
`extension_api.json` (source A), diffs against the vendored copy, runs `cargo xtask codegen-api`, and opens/updates
a PR via `peter-evans/create-pull-request@v8`. **Uses `base: main`** (canonical default branch). Skeleton (per
[`research/07` §8.3](research/07-ecosystem-and-release-tooling.md)):

```yaml
name: Sync Godot extension_api.json
on:
  schedule:
    - cron: '17 6 * * *'          # daily 06:17 UTC (off-peak); default-branch only; auto-disabled after 60d inactivity
  workflow_dispatch:
    inputs:
      godot_tag:
        description: 'Override Godot tag (e.g. 4.4.1-stable). Empty = latest stable.'
        required: false
        type: string
permissions:
  contents: write
  pull-requests: write
concurrency: { group: godot-api-sync, cancel-in-progress: false }
jobs:
  sync:
    runs-on: ubuntu-latest
    env: { GH_TOKEN: ${{ secrets.GITHUB_TOKEN }} }
    steps:
      - uses: actions/checkout@v7
      - name: Resolve latest stable Godot tag
        id: godot
        run: |
          set -euo pipefail
          if [ -n "${{ inputs.godot_tag }}" ]; then TAG="${{ inputs.godot_tag }}"
          else TAG="$(gh release list -R godotengine/godot --exclude-pre-releases --exclude-drafts --limit 1 --json tagName --jq '.[0].tagName')"; fi
          echo "tag=$TAG" >> "$GITHUB_OUTPUT"
      - name: Fetch upstream extension_api.json (godot-cpp @ tag)
        run: |
          set -euo pipefail
          TAG="${{ steps.godot.outputs.tag }}"
          curl -fsSL "https://raw.githubusercontent.com/godotengine/godot-cpp/${TAG}/gdextension/extension_api.json" -o new_extension_api.json
          # Fallback (B): download the Godot release binary + ./Godot --headless --dump-extension-api
      - name: Diff against committed copy, regenerate, open PR
        # diff -> if changed: cp into vendor/godot/<tag>/, run `cargo xtask codegen-api`,
        #         peter-evans/create-pull-request@v8 with base: main, branch: chore/godot-api-sync,
        #         labels: [dependencies, godot-sync], body carrying the API delta.
        run: echo "see GODOT-SYNC.md for the full body"
```

Enable repo setting *Settings → Actions → General → "Allow GitHub Actions to create and approve pull requests."*
Swap the default `GITHUB_TOKEN` for a PAT/App token if the bot PR must itself trigger CI. **Cross-reference:**
[`GODOT-SYNC.md`](GODOT-SYNC.md) (the rule file — multi-version snapping, codegen meaning, doc-XML conversion)
and [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md).

---

## Workstream F — Docs & governance scaffold

### F.1 mdBook (`docs/`)

`docs/book.toml` + `docs/src/SUMMARY.md` (the single TOC). Phase-0 skeleton `SUMMARY.md`:

```markdown
# Summary

- [Introduction](README.md)
- [User Guide]()
  - [Install](guide/install.md)
  - [Quickstart](guide/quickstart.md)
- [Consuming the Library]()
  - [From Rust](consume/rust.md)
  - [From Node](consume/node.md)
  - [From the Browser](consume/browser.md)
- [Editor / LSP Client Integration]()      <!-- grows in Phase 5; model on rust-analyzer "Other Editors" -->
  - [Overview](clients/overview.md)
- [Contributing]()
  - [Architecture](contributing/architecture.md)   <!-- links to plans/01-ARCHITECTURE.md -->
  - [Crate layout](contributing/crates.md)
  - [Build & test](contributing/build.md)
- [Architecture Decision Records](adr/README.md)
```

CI (`docs.yml`) runs `mdbook test` (validates Rust samples) + `mdbook-linkcheck`, then deploys via the official
**GitHub Pages starter** (`actions/configure-pages` → `mdbook build` → `actions/upload-pages-artifact` →
`actions/deploy-pages`, OIDC, no `gh-pages` branch).

### F.2 docs.rs config (`--cfg docsrs`)

Per crate, in each `Cargo.toml`:
```toml
[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]
```
and `#![cfg_attr(docsrs, feature(doc_cfg))]` at each crate root (already in the §A.3 stub). Preview locally:
`RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc --no-deps --all-features`. docs.rs builds in a
network-isolated sandbox — **all docs must build offline** (the vendored API data already satisfies this).

### F.3 README structure

Sections, in order: **title + one-liner**; **badges** (crates.io version, docs.rs, CI status, npm version,
license); **quickstart**; **"Consume from Rust / Node / browser"** (three short code blocks —
`cargo add gdscript-ide`; `npm i @gdscript-analyzer/core`; the wasm `import init, { … }` flow); **what it is /
isn't** (library, not a server; engine-neutral POD); **comparison + limitations**; **links** to the mdBook
guide, docs.rs, and the plan. Generatable from `lib.rs` later with `cargo-readme`.

### F.4 Governance files

| File | Phase-0 content |
|---|---|
| `CODE_OF_CONDUCT.md` | **Contributor Covenant 3.0**, with the reporting contact filled in (`yanivkalfa@gmail.com`). |
| `CONTRIBUTING.md` | Workspace map (link [`01`](01-ARCHITECTURE.md)); MSRV 1.88; the fmt/clippy/test/wasm-check gates (= `cargo xtask ci`); how to build napi + wasm; **the changeset requirement** for npm-facing changes; Conventional-Commit PR titles; the ADR process. |
| `SECURITY.md` | Supported-versions table + **Private Vulnerability Reporting** link (enable the GitHub feature); note `cargo-audit`/`cargo deny` advisory checks run in CI. |
| `GOVERNANCE.md`, `SUPPORT.md` | Decision process (lightweight; ADR-driven) + where to ask. |
| `.github/ISSUE_TEMPLATE/01-bug-report.yml` | YAML **issue form**: require a GDScript snippet, expected-vs-actual diagnostic, Godot version, analyzer version, crate-vs-npm, OS. |
| `.github/ISSUE_TEMPLATE/02-feature-or-diagnostic.yml` | Feature / new-diagnostic proposal form. |
| `.github/ISSUE_TEMPLATE/03-proposal.yml` | Design proposal form (routes to `S-needs-design`). |
| `.github/ISSUE_TEMPLATE/config.yml` | `blank_issues_enabled: false` + contact links. |
| `.github/PULL_REQUEST_TEMPLATE.md` | Checklist: tests, **changeset (if npm-facing)**, docs, `cargo xtask ci` green. |
| `.github/dependabot.yml` | `cargo` + `github-actions` + `npm`, weekly. |

Seed rust-analyzer-style labels (`C-bug/C-enhancement/C-diagnostic/C-architecture`, `E-easy/medium/hard`,
`S-actionable/needs-repro/needs-info/needs-design`, `good-first-issue`).

### F.5 ADRs (`docs/adr/`)

Nygard-format ADRs (Title / Status / Context / Decision / Consequences), numbered, with a `template.md`. The
**process**: any architecturally consequential decision lands as a numbered ADR in the same PR. Seed three from
the already-settled decisions in [`01`](01-ARCHITECTURE.md) / [`00`](00-VISION-AND-SCOPE.md):

| ADR | Title | Decision (summary) |
|---|---|---|
| **ADR-0001** | Rust + library-not-server | Build in Rust as an engine-/protocol-neutral **library** (POD + byte offsets); the LSP server is just one client. |
| **ADR-0002** | Hand-written parser, tree-sitter as oracle | Own a hand-written lossless `cstree` recursive-descent parser; use tree-sitter-gdscript only as the MVP bootstrap + permanent differential **test oracle**, never the grammar-of-record. |
| **ADR-0003** | napi-rs v3 dual-target binding | One `gdscript-ffi` binding via napi-rs v3 → both the Node `.node` addon **and** the `wasm32` target; wasm-bindgen kept as a documented fallback. |

### F.6 Licensing files

- **`LICENSE-MIT`** + **`LICENSE-APACHE`** in repo root; `license = "MIT OR Apache-2.0"` inherited via
  `[workspace.package]`; README license section.
- **`THIRD-PARTY-NOTICES.md`** — hand-maintained entries for the two non-crate inputs
  ([`research/07` §7](research/07-ecosystem-and-release-tooling.md)):
  - **Godot** (MIT/Expat) — full copyright + note that `extension_api.json` and the doc XML are MIT-derived
    Godot output.
  - **tree-sitter-gdscript** (MIT) — **retain `Copyright (c) 2016 Max Brunsfeld` verbatim** (the grammar's
    `LICENSE` keeps the unchanged tree-sitter template line) + credit the PrestonKnopp repo; include the
    tree-sitter runtime/`parser.c` entry if/when bundled (Phase 1).
- `deny.toml` enforces the permissive allow-list; `cargo-about`/`cargo-bundle-licenses` automates the *crate*
  attributions in CI. npm side: `"license": "(MIT OR Apache-2.0)"` (parentheses required) + ship both license
  files in each tarball.

---

## Workstream G — "How to run it" (developer onboarding)

The exact commands a brand-new contributor runs. (Documented verbatim in `CONTRIBUTING.md` and
`docs/src/contributing/build.md`.)

```bash
# 0. Prerequisites: rustup (the pinned toolchain + wasm32 target auto-install from rust-toolchain.toml),
#    Node >= 20, pnpm, and @napi-rs/cli. cargo plugins are auto-installed by CI; install locally as needed:
rustup show                                   # confirms toolchain + wasm32-unknown-unknown target
cargo install cargo-deny cargo-llvm-cov cargo-hack
npm i -g @napi-rs/cli pnpm

# 1. Clone
git clone https://github.com/yanivkalfa/gdscript-analyzer
cd gdscript-analyzer

# 2. Build + test the whole workspace
cargo build --workspace
cargo test  --workspace

# 3. Run the full local gate (mirrors ci.yml: fmt + clippy -D + test + wasm-check + deny)
cargo xtask ci

# 4. The portability guard on its own (the canonical wasm invariant)
cargo check -p gdscript-ide --target wasm32-unknown-unknown     # or: cargo wasm-check

# 5. Regenerate the engine-data artifact from the vendored extension_api.json
cargo xtask codegen-api

# 6. Build the napi (Node) package
cd bindings/node && pnpm install && napi build --platform --release && cd ../..
#   or, all artifacts at once:
cargo xtask dist

# 7. Build the wasm package (route A: napi wasm; route B: wasm-bindgen fallback)
napi build --platform --release --target wasm32-wasip1-threads   # route A (in bindings/node)
wasm-pack build bindings/wasm --target web --profile wasm-release # route B (fallback)

# 8. Serve the docs
mdbook serve docs        # http://localhost:3000   (mdbook test / linkcheck run in CI)
```

**Bootstrap checklist (new contributor):**
- [ ] `rustup show` lists the pinned `stable` toolchain + `wasm32-unknown-unknown`.
- [ ] Node ≥ 20, `pnpm`, `@napi-rs/cli` installed.
- [ ] `cargo build --workspace` succeeds.
- [ ] `cargo xtask ci` is green (fmt, clippy, test, wasm-check, deny).
- [ ] `cargo xtask codegen-api` produces the engine-data artifact.
- [ ] `cargo xtask dist` builds the (empty) napi + wasm stubs.
- [ ] `mdbook serve docs` renders the guide.
- [ ] Read [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md), [`ROADMAP.md`](ROADMAP.md), and the ADRs.

---

## Exit criteria (testable checklist — mirrors [`ROADMAP.md`](ROADMAP.md) Phase 0)

- [ ] **`cargo xtask ci` is green locally and in GitHub Actions** — fmt, clippy `-D warnings`, `cargo test
      --workspace` on all 3 OS, MSRV (1.88.0) check, coverage, and `cargo deny check` all pass.
- [ ] **`cargo check -p gdscript-ide --target wasm32-unknown-unknown` passes** (the portability guard), in CI
      on every PR.
- [ ] **All crate stubs compile** (`gdscript-base/-syntax/-api/-db/-hir/-ide/-scene/-ffi/-lsp/-cli`,
      `bindings/node`, `bindings/wasm`, `xtask`) with correct downward-only dependency edges.
- [ ] **A no-op release dry-run succeeds** — `release-plz update --dry-run` (via `cargo xtask release
      --dry-run`) reports the would-be bump + changelog; `changeset status` is clean; **nothing is published.**
- [ ] **The Godot-sync workflow runs on `workflow_dispatch`**, vendors `extension_api.json` into
      `vendor/godot/<version>/`, runs `cargo xtask codegen-api`, and **opens a PR** (verified against a
      synthetic API change / first import).
- [ ] **`extension_api.json` is vendored + codegen produces a `gdscript-api` data artifact** (first checked-in
      version; the pipeline runs end-to-end even though the transform is minimal).
- [ ] **Docs build & deploy** — `mdbook build` + `mdbook-linkcheck` pass; the Pages workflow publishes;
      docs.rs metadata present on every crate.
- [ ] **Governance + licensing complete** — dual `LICENSE-*`, `THIRD-PARTY-NOTICES.md`, CoC/CONTRIBUTING/
      SECURITY, issue forms, PR template, ADR-0001/0002/0003 committed; `main` is protected with the merge
      queue + required status checks.
- [ ] **A new contributor can clone, build, and run** following [Workstream G](#workstream-g--how-to-run-it-developer-onboarding).

---

## Risks & mitigations

| # | Risk | Likelihood | Mitigation |
|---|---|---|---|
| 1 | **wasm-check passes for empty stubs but rots once real deps land** (a transitive crate pulls `std::fs`/`SystemTime`/`getrandom`). | High (later) | The `wasm` CI job runs from day 1 on `gdscript-ide`; `deny.toml` bans known wasm-hostile crates; portability rules ([`01` §7](01-ARCHITECTURE.md)) are in `CONTRIBUTING.md`. Catch breakage at the PR that introduces the dep. |
| 2 | **Polyglot release drift** — crates.io and npm versions diverge (release-plz can't see `package.json`; Changesets can't see `Cargo.toml`). | Medium | The explicit one-line **version-sync step** (§D.2) keyed to release-plz as source of truth; `cargo xtask release --dry-run` asserts parity; the `fixed` Changesets group locks all npm packages together. |
| 3 | **napi cross-compile matrix is fragile** (zig/musl/QEMU toolchain churn). | Medium | Wire the matrix in Phase 0 against an **empty** binding so failures are toolchain-only, not logic — debug the pipeline before there's anything to bind. Pin action versions; cache per target. |
| 4 | **godot-sync caveats** — cron only runs on the default branch's latest commit, is delayed under load, auto-disables after 60d inactivity; default `GITHUB_TOKEN` PRs don't trigger CI. | Medium | Off-peak cron (`:17`); `workflow_dispatch` override; document the 60-day reactivation; use a PAT/App token if the sync PR must run CI; idempotent stable branch via `peter-evans/create-pull-request`. |
| 5 | **MSRV (1.88.0) vs newer-crate creep** — a dependency raises its own MSRV above ours. | Medium | The `msrv` CI job (`cargo hack check --rust-version`) fails the PR; pin via `[workspace.dependencies]`; bump MSRV deliberately with an ADR. |
| 6 | **edition 2024 / resolver 3 friction** with some tooling. | Low | Both are stable on the 1.88 toolchain; if a tool lags, the pin is one line to adjust. |
| 7 | **License-notice gaps** — tree-sitter template copyright line dropped, or a non-permissive transitive dep. | Low | `cargo deny check` (allow-list) in CI; the verbatim `Copyright (c) 2016 Max Brunsfeld` line in `THIRD-PARTY-NOTICES.md`; `cargo-about` automates crate attributions. |
| 8 | **Over-scoping Phase 0** — building features under cover of "tooling". | Medium | The "stub = compiles, no logic" rule; codegen-api transform is deliberately trivial; every feature has an owning later phase. |

---

## References

**Sibling plan docs**
- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — crate layout, public API, FFI/WASM strategy, portability rules (§7), repository layout (§8).
- [`ROADMAP.md`](ROADMAP.md) — Phase 0 deliverable + exit criteria; phase sequencing.
- [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) — the settled decisions (Rust; library-not-server).
- [`GODOT-SYNC.md`](GODOT-SYNC.md) — the engine-sync rule file: codegen meaning, multi-version snapping, doc-XML conversion (the content behind Workstream E).
- [`README.md`](README.md) — plan index.

**Research notes (evidence base)**
- [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) — **primary**: repo layout, xtask, CI matrix, release-plz/Changesets, docs, governance, licensing, godot-sync workflow.
- [`research/01-rust-distribution-tooling.md`](research/01-rust-distribution-tooling.md) — napi-rs v3 / wasm build tooling, per-platform npm packaging, MSRV 1.88, the `crates/*` + thin-bindings layout.
- [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md) — wasm build commands, the `wasm32-unknown-unknown` portability guard, size-profile flags, data-shipping (rkyv/brotli) plan.
- [`research/03-godot-api-sync.md`](research/03-godot-api-sync.md) — `extension_api.json` schema + per-version dump sources (the data side of Workstream E).
- [`research/02-parsing-strategy.md`](research/02-parsing-strategy.md) — tree-sitter-as-oracle (ADR-0002) and the cstree decision.
- [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) — the `AnalysisHost`/`Analysis` library shape (ADR-0001).
```
