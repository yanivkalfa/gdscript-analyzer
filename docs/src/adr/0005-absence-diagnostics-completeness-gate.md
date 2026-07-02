# ADR-0005: Absence-based diagnostics gate on a loader-asserted complete workspace

- **Status:** Accepted
- **Date:** 2026-07-02

## Context

The analyzer's "Unknown seam" deliberately silences every unresolved name so that cross-file
symbols the host has not loaded never false-flag. The cost: calling a genuinely undeclared
function (`usseState(0)`) produces **zero** diagnostics — Godot itself errors on it, and the
`.guitkx` LSP built on this analyzer cannot catch obvious typos live.

Reporting "defined **nowhere**" is an absence proof: it requires seeing **everywhere** a
definition could live. No signal the database already had can establish that:

- `source_root().is_some()` is true after one lone file is opened;
- `project_config().is_some()` is true for a single-file CLI run (the `project.godot` walk-up
  discovery finds it) while only one file is loaded;
- per-name registry hits (`global_registry`) prove presence, never absence.

Options considered: (a) emit whenever a `project.godot` is present (unsound — the single-file
case above false-flags every cross-file `class_name`); (b) heuristics on the name shape (rejected
outright — fragile); (c) an explicit, loader-owned completeness assertion.

## Decision

We will add a **`complete: bool` field on the `SourceRoot` salsa input** — a claim only the
*loader* can truthfully make — plumbed as `Change::set_workspace_complete` /
`setWorkspaceComplete` through the session and the napi/wasm bindings, and gate the new
`UNDEFINED_FUNCTION` / `UNDEFINED_IDENTIFIER` codes (ERROR-default: they are compile errors in
Godot) on it plus per-emission guards: a top-level script class, a fully engine-native base
chain, a project engine version not newer than the bundled model, and a per-name miss of every
resolution tier (locals, members, engine base, engine globals, `class_name` registry, autoload
registry).

The CLI earns the claim by loading the **whole project root as context** (targets keep exclusive
reporting) with a **Godot-faithful walk** — `.gitignore` deliberately not honored, `.gdignore`
treated as Godot's directory marker, dot-directories skipped — and withholds it whenever any
filesystem target resolves to a different project root, stdin is involved, any file fails to
read, or the project contains `.gdextension`/C# sources (runtime-registered classes are
invisible to the analyzer).

Rejected alternative: project-config presence as the gate — demonstrated unsound on the
single-file invocation; validated instead against all 138 godot-demo-projects (216 first-run
false positives driven to 0 by root-cause fixes, none by weakening the gate).

## Consequences

Easier: the guitkx LSP (which feeds the whole project) can arm live undefined-symbol detection by
one call; any host that cannot honestly claim completeness gets exactly the old silent-seam
behavior — soundness by default. Harder: the claim is trust-based — a host that lies gets false
positives (documented on every plumbing surface); deep CLI loads read every project `.gd` even
for one target (the documented load→fan-out design, and per-file commands use a shallow load);
and the bundled engine model must track the latest stable Godot or newer-engine projects are
gated off (multi-version bundling is the standing GODOT-SYNC plan).
