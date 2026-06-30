# gdscript-cli

The **`gdscript` command-line tool** of [gdscript-analyzer](https://github.com/yanivkalfa/gdscript-analyzer) — a fast, embeddable GDScript (Godot 4.x) static-analysis library. **Roslyn for Godot.**

[![crates.io](https://img.shields.io/crates/v/gdscript-cli.svg?logo=rust)](https://crates.io/crates/gdscript-cli)
[![docs.rs](https://img.shields.io/docsrs/gdscript-cli?logo=docsdotrs)](https://docs.rs/gdscript-cli)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/yanivkalfa/gdscript-analyzer#license)

`check` / `lint` / `format` / `symbols` over a Godot project, for **CI and
pre-commit** — no editor, no running Godot. A single static binary built on the
[`gdscript-ide`](https://crates.io/crates/gdscript-ide) core: it is the only layer
that touches the filesystem (walks `.gd` files honoring `.gitignore`/`.gdignore`,
discovers `project.godot` + an optional `gdscript-analyzer.toml`), loads the whole
project into one analysis host, and fans the per-file reads out in parallel.

```sh
cargo install gdscript-cli      # installs the `gdscript` binary

gdscript check  path/to/project          # parse + type diagnostics (the CI workhorse)
gdscript lint   path/to/project          # the warning subset
gdscript format --check src/             # report unformatted files (exit 1 if any)
gdscript format --write  src/            # rewrite in place
gdscript symbols main.gd                 # dump document symbols as JSON
```

**Output formats** (`--format`): `human` (rustc/Ruff-style `path:line:col: severity[CODE]
message` with a caret-underlined snippet, `NO_COLOR`-aware), `json`, `github` (Actions
annotations), and `sarif` (code-scanning). **Exit codes** follow the linter convention:
`0` clean, `1` diagnostics found / would-reformat, `2` usage/config error.

Part of the gdscript-analyzer workspace — see the
[repo README](https://github.com/yanivkalfa/gdscript-analyzer#the-workspace--crates--packages)
for the full crate map. Licensed MIT OR Apache-2.0.
