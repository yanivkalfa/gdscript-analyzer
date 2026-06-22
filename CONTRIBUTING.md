# Contributing to gdscript-analyzer

Thanks for your interest in contributing! `gdscript-analyzer` is a fast,
embeddable, multi-target **GDScript static-analysis library** — "Roslyn for
Godot." It is a Rust library (not an LSP server), engine-neutral, and built to
run natively, in Node via napi, and in the browser via WebAssembly.

This guide covers everything you need to build, test, and submit a change. If
anything here is out of date or unclear, please open an issue — fixing the
contributor experience is itself a welcome contribution.

> **Project status:** Phase 0 (ecosystem & tooling). The repository is
> scaffolding that *compiles and releases* — there are no analyzer features
> yet. See [`plans/ROADMAP.md`](plans/ROADMAP.md).

---

## Code of Conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md)
(Contributor Covenant 3.0). Please read it. Report concerns to
`yanivkalfa@gmail.com`.

---

## Workspace & crate map

`gdscript-analyzer` is a flat virtual Cargo workspace (`crates/*`) with thin
language bindings split out under `bindings/`. Each crate depends **only
downward** — see [`plans/01-ARCHITECTURE.md`](plans/01-ARCHITECTURE.md) for the
authoritative layout and dependency rules.

| Crate | Role |
|---|---|
| `gdscript-base` | POD primitives (`FileId`, `TextSize`/`TextRange`, `LineIndex`). |
| `gdscript-syntax` | Lexer / lossless CST / parser (Phase 1). |
| `gdscript-api` | Engine model; the `extension_api.json` codegen output lands here. |
| `gdscript-db` | VFS / project model (Phase 3). |
| `gdscript-hir` | Semantic layer — name resolution, type inference (Phase 2/3). |
| `gdscript-ide` | **The public API** (`AnalysisHost` / `Analysis`); the wasm-check target. |
| `gdscript-scene` | `.tscn` / `.tres` parser (Phase 4). |
| `gdscript-ffi` | The single napi/wasm binding crate (`cdylib` + `lib`). |
| `gdscript-lsp` | LSP server binary — one client of the library (Phase 5). |
| `gdscript-cli` | CLI binary — `check`/`lint`/`format`/`symbols` (Phase 5). |
| `bindings/node` | napi-rs npm package (`@gdscript-analyzer/core`). |
| `bindings/wasm` | wasm-bindgen fallback package (`@gdscript-analyzer/wasm`). |
| `xtask` | The build automation binary (`cargo xtask …`). |

The public Rust API crate is **`gdscript-ide`** — that is what downstream Rust
consumers depend on, and what the WebAssembly portability guard checks.

---

## Prerequisites

| Tool | Requirement | Notes |
|---|---|---|
| **Rust toolchain** | Auto-installed from `rust-toolchain.toml` | rustup reads the pin (channel + `rustfmt`/`clippy`/`rust-src` components + `wasm32-unknown-unknown` target). Run `rustup show` to confirm. |
| **MSRV** | **1.88.0** | The whole workspace must build on this floor (napi-rs v3's minimum). CI enforces it via `cargo hack check --rust-version`. |
| **Node.js** | **>= 20** (CI tests 20 / 22 / 24) | Needed to build and test the napi binding and to run Changesets. |
| **pnpm** | latest | Package manager for the JS workspace; the lockfile is committed. |
| **`@napi-rs/cli`** | v3 | Builds the `.node` addon and the wasm binding target. |

```bash
rustup show                                   # confirms toolchain + wasm32-unknown-unknown
cargo install cargo-deny cargo-llvm-cov cargo-hack   # CI installs these; install locally as needed
npm i -g pnpm @napi-rs/cli
```

---

## The local gate: `cargo xtask ci`

`cargo xtask ci` is the **one command** that mirrors the CI core gate. Run it
before opening a PR; if it is green, the corresponding CI jobs will be too. It
runs, in order, and fails fast on the first error:

1. `cargo fmt --all -- --check` — formatting (rustfmt, `--check`).
2. `cargo clippy --all-targets --all-features -- -D warnings` — lints, **deny on warning**.
3. `cargo test --workspace` — the test suite.
4. `cargo check -p gdscript-ide --target wasm32-unknown-unknown` — the **WebAssembly portability guard** (see below).
5. `cargo deny check` — license allow-list + RustSec advisories + bans.

You can run the portability guard on its own with the convenience alias:

```bash
cargo wasm-check        # = cargo check -p gdscript-ide --target wasm32-unknown-unknown
```

---

## Building the packages

```bash
# Whole workspace
cargo build --workspace
cargo test  --workspace

# Regenerate the engine-data artifact from the vendored extension_api.json
cargo xtask codegen-api

# napi (Node) package
cd bindings/node && pnpm install && napi build --platform --release && cd ../..

# All distributable artifacts at once (napi + wasm)
cargo xtask dist

# wasm package directly
#   route A (default): the napi wasm target
napi build --platform --release --target wasm32-wasip1-threads    # run in bindings/node
#   route B (fallback): wasm-bindgen
wasm-pack build bindings/wasm --target web --profile wasm-release
```

---

## Portability rules (read this before touching a core crate)

The core crates **must compile to `wasm32-unknown-unknown`**. This is the single
most important invariant in the project and is CI-enforced on `gdscript-ide`
(the public surface) on every PR. In the core crates
(`gdscript-base`/`-syntax`/`-api`/`-db`/`-hir`/`-ide`/`-scene`):

- **No `std::fs`** (or any filesystem access). File contents are *injected* into
  the analysis host by the caller.
- **No wall-clock time** — no `std::time::Instant::now()` / `SystemTime::now()`.
  Clocks are injected.
- **No threads in the hot path**, and no APIs unavailable on
  `wasm32-unknown-unknown` (e.g. unbounded `getrandom`, blocking I/O).
- **Never leak protocol types** (e.g. `lsp-types`) into the core — results are
  engine-neutral POD structs and byte offsets; clients map them to their
  protocol.

Side-effectful behavior (filesystem, clocks, process, networking) belongs in the
client crates (`gdscript-cli`, `gdscript-lsp`, `bindings/*`), never in the
analysis core. `deny.toml` additionally bans known wasm-hostile crates. If you
introduce a dependency that breaks the wasm check, the PR that adds it is where
the breakage must be fixed.

---

## Commits, PR titles, and merging

We **squash-merge**, and the **PR title becomes the single commit on `master`**
that the release tooling reads. Therefore the **PR title must be a valid
[Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/)** — it is
linted in CI (`amannn/action-semantic-pull-request`).

- Allowed types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`,
  `ci`, `chore`, `revert`.
- Use a crate name as the scope where it helps: `feat(syntax): …`,
  `fix(ide): …`.
- Breaking changes: append `!` (e.g. `feat(ide)!: …`) or add a
  `BREAKING CHANGE:` footer. While the project is `0.x`, a breaking change is
  the `0.x` "major" bump.

Keep PRs focused and short-lived; `master` is protected and guarded by a merge
queue.

---

## Changesets — required for npm-facing changes

The Rust release derives version bumps from Conventional-Commit titles, but the
npm side is driven by **[Changesets](https://github.com/changesets/changesets)**,
which reads `.changeset/*.md` files — **not** commit messages. So:

> **If your change affects the published npm packages
> (`@gdscript-analyzer/*` — anything under `bindings/`, the binding surface, or
> their behavior/docs), you must add a changeset.**

```bash
pnpm changeset        # pick the affected packages + bump level, write a summary
```

Commit the generated `.changeset/<name>.md` with your PR. Pure-Rust changes that
do not affect any npm package do not need a changeset.

---

## Architecture Decision Records (ADRs)

Architecturally consequential changes must land with a **numbered ADR** in the
same PR, under [`docs/src/adr/`](docs/src/adr/), using the Nygard format
(Title / Status / Context / Decision / Consequences). Copy `template.md`, take
the next number, and link it from the ADR index. Examples of "consequential":
adding a major dependency, changing a public API contract, changing the crate
graph, or raising the MSRV. The already-settled foundational decisions are
recorded as ADR-0001 (Rust + library-not-server), ADR-0002 (hand-written parser,
tree-sitter as oracle), and ADR-0003 (napi-rs v3 dual-target binding).

---

## Opening issues

Use the [issue forms](.github/ISSUE_TEMPLATE/) — bug report, feature /
diagnostic proposal, or design proposal. For bugs, please include a minimal
GDScript snippet, the expected vs. actual diagnostic, the Godot version, the
analyzer version, whether you hit it via the crate or the npm package, and your
OS.

---

## License

By contributing, you agree that your contributions are dual-licensed under
**MIT OR Apache-2.0** (see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE)), matching the project license, with no
additional terms.
