# Third-Party Notices

`gdscript-analyzer` is distributed under the dual license `MIT OR Apache-2.0`
(see [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)).

This file records attribution for **non-crate inputs** — third-party material
that is vendored, derived, or bundled into this repository but does not arrive
through the Cargo dependency graph. Attribution for Cargo (crate) dependencies
is **automated in CI** (see [Cargo dependency attribution](#cargo-dependency-attribution)
below) and is therefore not duplicated here.

---

## 1. Godot Engine — `extension_api.json` and class documentation XML

The engine-data artifacts vendored under `vendor/godot/<version>/` —
specifically `extension_api.json` and any `doc/classes/*.xml` documentation —
are output produced by, and derived from, the **Godot Engine**. They are
MIT-derived Godot output and remain subject to the Godot Engine license.

- Project: Godot Engine — <https://godotengine.org>
- Source: <https://github.com/godotengine/godot>
- License: MIT (Expat)
- Default upstream source for the vendored JSON: the godot-cpp committed copy at
  `https://raw.githubusercontent.com/godotengine/godot-cpp/<TAG>/gdextension/extension_api.json`
  (godot-cpp is likewise MIT and ingests the same artifact). The matching Godot
  release binary's `--dump-extension-api` output is the fallback source. The
  exact source and tag for each vendored version are recorded in
  `vendor/godot/<version>/SOURCE.txt`.

```
Copyright (c) 2014-present Godot Engine contributors.
Copyright (c) 2007-2014 Juan Linietsky, Ariel Manzur.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## 2. tree-sitter-gdscript

The GDScript grammar from **tree-sitter-gdscript** is used as the MVP bootstrap
and as a permanent differential **test oracle** (see ADR-0002). The grammar's
`LICENSE` retains, unchanged, the original tree-sitter project template
copyright line; that line is reproduced **verbatim** below and must not be
altered or removed.

- Project: tree-sitter-gdscript (maintained by PrestonKnopp and contributors)
- Source: <https://github.com/PrestonKnopp/tree-sitter-gdscript>
- License: MIT
- Built on the tree-sitter framework — <https://github.com/tree-sitter/tree-sitter>

```
Copyright (c) 2016 Max Brunsfeld

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

> **Note (Phase 1):** when the generated `parser.c` and/or the tree-sitter
> runtime are actually bundled into a published artifact, a corresponding entry
> for the tree-sitter runtime (MIT, © Max Brunsfeld and the tree-sitter
> contributors) will be added here. As of Phase 0 no tree-sitter code is
> compiled or shipped — only this attribution is recorded in advance.

---

## Cargo dependency attribution

Attribution and license compliance for the workspace's **Cargo dependencies**
are automated in CI and are *not* hand-maintained in this file:

- **`cargo deny check`** enforces the permissive license allow-list, scans the
  RustSec advisory database, and bans disallowed/duplicate crates
  (configuration in `deny.toml`).
- **`cargo-about`** generates the aggregated per-crate license attribution
  bundle for distributed artifacts.

The npm packages declare `"license": "(MIT OR Apache-2.0)"` and ship copies of
both `LICENSE-MIT` and `LICENSE-APACHE` in each published tarball.
