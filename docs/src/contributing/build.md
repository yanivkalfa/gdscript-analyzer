# Build & test

The exact commands a brand-new contributor runs. These mirror Workstream G of
[`plans/PHASE-0-ECOSYSTEM-AND-TOOLING.md`](../../../plans/PHASE-0-ECOSYSTEM-AND-TOOLING.md)
and the `CONTRIBUTING.md` at the repo root.

## 0. Prerequisites

You need [rustup](https://rustup.rs), Node ≥ 20, [pnpm](https://pnpm.io), and
the napi-rs CLI. The pinned toolchain and the `wasm32-unknown-unknown` target
auto-install from `rust-toolchain.toml` on first build. The cargo plugins are
auto-installed by CI; install them locally as needed:

```bash
rustup show                                   # confirms toolchain + wasm32 target
cargo install cargo-deny cargo-llvm-cov cargo-hack
npm i -g @napi-rs/cli pnpm
```

The MSRV is **1.88.0** (the napi-rs v3 floor); CI checks the whole workspace
against it with `cargo hack check --rust-version`.

## 1. Clone

```bash
git clone https://github.com/yanivkalfa/gdscript-analyzer
cd gdscript-analyzer
```

## 2. Build + test the workspace

```bash
cargo build --workspace
cargo test  --workspace
```

## 3. Run the full local gate

`cargo xtask ci` is the one-shot gate that mirrors `ci.yml` exactly:
`cargo fmt --check` → `cargo clippy -D warnings` → `cargo test --workspace` →
the wasm portability check → `cargo deny check`. **This is the command the exit
criteria reference** — it must be green before you open a PR.

```bash
cargo xtask ci
```

## 4. The portability guard (on its own)

The single most important invariant after "it compiles": the public surface must
build for the browser target.

```bash
cargo check -p gdscript-ide --target wasm32-unknown-unknown   # or: cargo wasm-check
```

## 5. Regenerate the engine-data artifact

```bash
cargo xtask codegen-api    # reads vendor/godot/<version>/extension_api.json
```

## 6. Build the napi (Node) package

```bash
cd bindings/node && pnpm install && napi build --platform --release && cd ../..
# or build all artifacts at once:
cargo xtask dist
```

## 7. Build the wasm package

```bash
# route A (primary): napi-rs wasm target, in bindings/node
napi build --platform --release --target wasm32-wasip1-threads
# route B (fallback): wasm-bindgen
wasm-pack build bindings/wasm --target web --profile wasm-release
```

## 8. Serve the docs

```bash
mdbook serve docs        # http://localhost:3000
# CI additionally runs `mdbook test` (validates Rust samples) + mdbook-linkcheck
```

## Bootstrap checklist

- [ ] `rustup show` lists the pinned `stable` toolchain + `wasm32-unknown-unknown`.
- [ ] Node ≥ 20, `pnpm`, `@napi-rs/cli` installed.
- [ ] `cargo build --workspace` succeeds.
- [ ] `cargo xtask ci` is green (fmt, clippy, test, wasm-check, deny).
- [ ] `cargo xtask codegen-api` produces the engine-data artifact.
- [ ] `cargo xtask dist` builds the napi + wasm stubs.
- [ ] `mdbook serve docs` renders this guide.
- [ ] You have read [Architecture](./architecture.md),
      [`plans/ROADMAP.md`](../../../plans/ROADMAP.md), and the
      [ADRs](../adr/README.md).

## Conventions

- **Conventional Commits** for PR titles (`feat(syntax): …`, `fix(ide): …`,
  `!`/`BREAKING CHANGE:` for breaks). Squash-merge uses the PR title.
- **Changesets** are required for user-facing `@gdscript-analyzer/*` npm
  changes (`pnpm changeset`) — the Rust side derives its bump from commits, but
  the npm side reads `.changeset/*.md`.
