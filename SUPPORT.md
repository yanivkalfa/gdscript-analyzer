# Support

Need help with `gdscript-analyzer`? Here is where to go.

> **Project status:** Phase 0 (ecosystem & tooling) — the repository compiles
> and releases, but there are **no analyzer features yet**. See
> [`plans/ROADMAP.md`](plans/ROADMAP.md) for what is coming.

## Where to ask

| You want to… | Go to |
|---|---|
| Ask a question, get usage help, or float an idea | [GitHub Discussions](https://github.com/yanivkalfa/gdscript-analyzer/discussions) |
| Report a reproducible bug | [Open a bug report issue](https://github.com/yanivkalfa/gdscript-analyzer/issues/new/choose) |
| Request a feature or a new diagnostic | [Open a feature / diagnostic issue](https://github.com/yanivkalfa/gdscript-analyzer/issues/new/choose) |
| Propose a design / architectural change | [Open a proposal issue](https://github.com/yanivkalfa/gdscript-analyzer/issues/new/choose) |
| Report a security vulnerability | **Do not open a public issue** — follow [`SECURITY.md`](SECURITY.md) |

Please use **Discussions for open-ended questions** and **Issues for actionable,
reproducible work**. This keeps the issue tracker focused on things that have a
clear next step.

## Before you ask

A little prep gets you a faster, better answer:

- Check the [documentation](https://github.com/yanivkalfa/gdscript-analyzer#readme)
  and the [`plans/`](plans/) docs.
- Search existing [issues](https://github.com/yanivkalfa/gdscript-analyzer/issues)
  and [discussions](https://github.com/yanivkalfa/gdscript-analyzer/discussions)
  — your question may already be answered.
- For anything reproducible, include: a minimal GDScript snippet, what you
  expected vs. what happened, the Godot version, the `gdscript-analyzer`
  version, whether you are using the Rust crate or the npm package, and your OS.

## What's on-topic

- Using the library from Rust (`gdscript-ide`), Node (`@gdscript-analyzer/core`),
  or the browser (`@gdscript-analyzer/wasm`).
- Bugs in parsing, analysis results, or the build/distribution tooling.
- Feature and diagnostic requests for GDScript (Godot 4.x) analysis.
- Contributing — see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## What's off-topic

`gdscript-analyzer` is an analysis **library**, not the Godot engine and not the
Godot editor. The following are out of scope here — please take them to the
relevant project:

- General Godot engine / editor usage → the
  [Godot community](https://godotengine.org/community).
- Running or debugging GDScript *at runtime* — we statically analyze code; we do
  not execute it or talk to a live engine.
- Godot 3.x / GDScript 1.x — only Godot 4.x / GDScript 2.0 is supported.

## Useful links

- [README](https://github.com/yanivkalfa/gdscript-analyzer#readme) — quickstart and overview.
- [Roadmap](plans/ROADMAP.md) — phases and exit criteria.
- [Contributing guide](CONTRIBUTING.md) — build, test, and submit changes.
- [Code of Conduct](CODE_OF_CONDUCT.md).
- [Security policy](SECURITY.md).
