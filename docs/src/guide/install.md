# Install

> **Status: Phase 0.** The packages described here are not yet published. The
> commands below show the intended install story for the `0.x` line; they are
> documented now so the surface is fixed before the first release. See
> [`plans/ROADMAP.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/ROADMAP.md).

## Toolchain (contributors)

You do **not** need to manage the Rust toolchain by hand. The repository pins
everything in `rust-toolchain.toml` (channel, components, and the
`wasm32-unknown-unknown` target), so with [rustup](https://rustup.rs) installed,
the correct toolchain — including the wasm target used by the portability
guard — is fetched automatically the first time you build:

```bash
rustup show   # confirms the pinned stable toolchain + wasm32-unknown-unknown
```

The minimum supported Rust version (MSRV) is **1.88.0** — the floor required by
napi-rs v3. CI enforces it across the whole workspace.

## From Rust (crates.io)

The public Rust crate is **`gdscript-ide`** — the `AnalysisHost` / `Analysis`
surface that external Rust consumers depend on:

```bash
cargo add gdscript-ide   # coming in the 0.x line
```

The lower-level crates (`gdscript-base`, `gdscript-syntax`, `gdscript-api`, …)
are published too, but most consumers only need `gdscript-ide`. See
[Consuming from Rust](../consume/rust.md).

## From Node (npm)

The napi-rs native addon is published under the `@gdscript-analyzer` scope:

```bash
npm i @gdscript-analyzer/core    # coming in the 0.x line
# pnpm add @gdscript-analyzer/core
```

Per-platform prebuilt binaries are delivered automatically via
`optionalDependencies` (`@gdscript-analyzer/core-linux-x64-gnu`,
`-darwin-arm64`, `-win32-x64-msvc`, …), so there is no native build step for
consumers. See [Consuming from Node](../consume/node.md).

## From the browser (WASM)

A WebAssembly build ships as a separate npm package
(`@gdscript-analyzer/wasm`) for in-page analysis (playgrounds, web editors). See
[Consuming from the Browser](../consume/browser.md).

## Versioning

crates.io and npm move in **lockstep on a single shared version**, starting
`0.1.0`. While in `0.x`, a breaking change bumps the *minor* and a new feature
is a *patch* (Cargo's 0.x SemVer reading). The contract every consumer builds on
is the `gdscript-ide` API surface.
