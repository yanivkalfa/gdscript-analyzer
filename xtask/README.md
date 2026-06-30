# xtask

Repository automation for **[gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer)**,
run via the cargo alias `cargo xtask <command>`. A plain Rust binary (the
[cargo-xtask](https://github.com/matklad/cargo-xtask) pattern) so the automation is
cross-platform with no `make`/`bash`/`python` dependency. **Not published** — it is a
dev-only workspace member.

| Command | What it does |
|---|---|
| `cargo xtask ci` | The one-shot local gate mirroring CI: `fmt --check`, `clippy --all-targets --all-features -D warnings`, `test`, the `wasm32` portability check, and `cargo-deny` (best-effort locally). |
| `cargo xtask codegen-api` | Regenerate `gdscript-api`'s engine model + the hover-doc store from the vendored `extension_api.json` + `doc/classes/*.xml` (`rkyv`-encoded into `engine_api.bin` / `engine_docs.bin`). |
| `cargo xtask fixtures` | Re-bless the golden `expect-test` snapshots (`UPDATE_EXPECT=1`). |
| `cargo xtask differential` | Cross-validate the parser against tree-sitter-gdscript (the oracle). |
| `cargo xtask dist` | Build the workspace + binding stubs end-to-end. |
| `cargo xtask release [--dry-run]` | Local release ergonomics (actual publishing is CI-only, via release-plz). |

Part of the gdscript-analyzer workspace — see the
[repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
for the full crate map. Licensed MIT OR Apache-2.0.
