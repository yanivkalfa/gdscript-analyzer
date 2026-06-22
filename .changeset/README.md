# Changesets

This folder is managed by [Changesets](https://github.com/changesets/changesets).
It drives versioning and changelogs for the **npm** side of the project — the
`@gdscript-analyzer/*` packages (the napi `@gdscript-analyzer/core`, the wasm
package, and their per-platform binary sub-packages). The Rust / crates.io side
is handled separately by [release-plz](../release-plz.toml); the two registries
move in lockstep on a single shared version.

## What is a changeset?

A changeset is a small Markdown file in this folder that records **an intent to
release**: which packages changed, at what SemVer level, and a human-readable
summary that becomes the changelog entry. You add one in the same PR as the
change it describes. At release time Changesets consumes all pending changeset
files, bumps the package versions, writes the changelog, and deletes the files.

## When do I need one?

**Any user-facing change to an `@gdscript-analyzer/*` npm package requires a
changeset** — a new or changed API on the napi/wasm surface, a bug fix that
ships to npm consumers, a dependency bump that affects the published package,
etc. Internal-only changes (CI, docs, refactors with no published-surface
effect) do **not** need one.

> Note: the Rust crates derive their version bump from Conventional-Commit
> messages via release-plz, *not* from changesets. Changesets only reads the
> files in this folder, so the npm bump must be declared here explicitly.

## How do I add one?

```bash
pnpm changeset
```

The CLI prompts you to:

1. select the affected `@gdscript-analyzer/*` package(s),
2. choose a bump level — **patch** for a fix, **minor** for a backwards-
   compatible feature, **major** for a breaking change (while we are `0.x`,
   follow the same 0.x reading as the Rust side: a breaking change is a
   *minor*, a feature is a *patch*), and
3. write a one-line summary.

All `@gdscript-analyzer/*` packages are in a single `fixed` group (see
[`config.json`](./config.json)), so they always version together — selecting one
versions them all.

Commit the generated `.changeset/<name>.md` file with your PR. To preview what a
release would do without publishing, run `pnpm changeset status`.

Full docs: <https://github.com/changesets/changesets/blob/main/docs/adding-a-changeset.md>
