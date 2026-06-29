# Phase 6 · W8 — Scene-Aware Rename Playbook

> The write-side counterpart to Phase 4's read-side scene awareness (typing / goto / completion /
> diagnostics). Renaming a **script symbol** updates its **scene references** (`[connection]`s, exported
> properties), and renaming a **scene node** updates its **`$Path`/`%Unique`/`get_node` references** in
> scripts plus the dependent in-scene paths. Additive (ships MINOR under W6's `#[non_exhaustive]`); does
> **not** move the 1.0 cut. Built to the same **correct-or-refuse** contract as M5 cross-file rename —
> never a partial / corrupting edit.
>
> **Grounded against the tree** (`feat/formatter-scene-rename`, 2026-06-29). Reuses the Phase-4 scene
> model (`gdscript-scene`), `script_scene_index` / `scene_context` / `scene_model` / `res_path_registry`
> (`gdscript-hir/queries.rs`), `node_path_target` (`def.rs`), the 3-state `NodePathResolution`, the
> cross-file `SourceChange`/`FileEdit`, and `workspace_edit` (FileId→URI, already `.tscn`-capable).

## Scope — both directions, all cases

| # | Case | Direction | Example |
|---|---|---|---|
| A1 | method connected via `[connection method=…]` | `.gd`→`.tscn` | rename `_on_pressed` → rewrite the connection line |
| A2 | signal connected via `[connection signal=…]` | `.gd`→`.tscn` | rename `health_changed` → rewrite the connection |
| A3 | `@export var` set as a node property | `.gd`→`.tscn` | rename `speed` → rewrite `speed = 5.0` in `[node]` |
| B1 | scene node referenced by `$Path`/`%Unique`/`get_node("…")` | `.tscn`→`.gd` | rename node `StartButton` → rewrite every `$Panel/StartButton`, `%StartButton` |
| B2 | dependent in-scene paths | `.tscn`→`.tscn` | rewrite child `parent="…/StartButton"`, `[connection] from/to` |

A1/A2 also **fix a latent correctness bug**: today a method connected *only* via `.tscn` (no `.gd`
string) renames the `.gd` and **silently misses** the connection (`project_has_string_literal` scans
`.gd` only — TECH_DEBT M5 note).

## What exists vs net-new

**Reuse as-is:** `scene_model` (firewalled parse), `script_scene_index` (script→scene + `ambiguous`),
`scene_context`, `res_path_registry`, `source_root().files()` (all `.gd`+`.tscn`), `node_path_target`
(`$Path`→exact `name_span`), `NodePathResolution`, `SourceChange`/`FileEdit`, `workspace_edit`, the
correct-or-refuse rename skeleton.

**Net-new:** `[connection]` + node-property capture in the parser; `GodotDef::SceneNode`; scene-side
`classify`; scene-scanning `find_references`; path-segment edit construction; the LSP accepting a
`.tscn` rename position.

## Milestones (each = one gated commit; investigate → develop → test/bug-hunt → gate → commit → push)

- **M1 — Parser: capture `[connection]`.** `SceneModel.connections: Vec<SceneConnection>` `{ signal,
  signal_span, from, from_span, to, to_span, method, method_span, header_span }`, filled from the
  existing `Some("connection" | …)` branch. *Gate:* 524 demo-projects scenes parse 0-panic, every
  connection captured, spans byte-exact.
- **M2 — Parser: capture node property keys.** `SceneNode.properties: Vec<NodeProp { key, key_span }>`
  (keys only; values skipped). *Gate:* corpus re-parse 0 regressions.
- **M3 — Half A: method/signal rename rewrites connections.** For a `Member` of kind method/signal,
  scan project scenes; a method matches a connection iff `method == name` **and** the `to` node attaches
  **this exact script** (`node.script` → `res_path_registry` → this `FileId`); a signal iff `signal ==
  name` **and** the `from` node attaches this script. Add the `.tscn` edit; keep refusing only on an
  *un*attributable same-named `.gd` string.
- **M4 — `GodotDef::SceneNode { scene, path }` + bidirectional classify.** Identity = (scene file, full
  root-relative name-path; unique within a scene, stable pre-edit). `.gd` `$Path`/`%Unique`/`get_node`
  segment → `SceneNode`; `.tscn` `name="…"` → `SceneNode`. Classify to `None` (→ no rename) on ambiguous
  attach, computed `get_node(var)`, `%`-modulo, `..`/absolute escape.
- **M5 — `find_references` for a `SceneNode`.** Scene side: own `name_span` + dependent `parent=` /
  `[connection] from/to` **segment** sub-ranges. `.gd` side: every file whose `scene_context.scene ==
  scene`, scan `GetNode`, resolve, emit the **segment** sub-range within the `$Path` token (from
  `BodySourceMap::expr_range` + path split, past the sigil/quote).
- **M6 — Half B: rename a `SceneNode`.** Assemble edits; refuse the whole op on any ambiguous `.gd`
  context, uncertain resolution, or a `/`-containing node name. LSP/`ide::rename` accept a `.tscn`
  position **and** a `$Path`.
- **M7 — A3: exported var as a node property.** Rewrite a node property `key_span` only when `name` is
  provably an `@export var` of the script **and** not an engine property of the node's `type=`; else
  leave (var still renames in `.gd`).
- **M8 — Adversarial bug-hunt + corpus validation.** Multi-finder / 3-vote over the new edit paths,
  focus **zero false edits**: rename across 524 scenes + ReactiveUI, 0 panics, every emitted edit set
  re-parses both `.tscn` and `.gd` clean + idempotent.
- **M9 — Docs/tech-debt.** Flip the four `Scene-aware rename → Phase 6` TECH_DEBT items to done.

## Cross-cutting rules

- **Salsa firewall:** connection/property data rides `scene_model` (firewalled to the `.tscn` text); the
  cross-scene aggregation is an on-demand fold over `source_root().files()` like `find_references` — no
  new salsa input, no `.gd`-body-edit invalidation.
- **Correct-or-refuse everywhere:** every new branch resolves provably or contributes a refuse;
  `NodePathResolution` + `ambiguous` are the existing genuine-reference signals — never guess.
- **No `.tscn` reformatting:** only identifier-like spans are replaced (`name=`, `method=`, `signal=`,
  path segments, property keys); the rest of the scene is byte-for-byte untouched.

## Risk ranking

1. **M5/M6 segment rewriting** — partial-token edits inside `$A/B/C`; mitigated by computing sub-ranges
   from the parsed path and refusing when uncertain.
2. **M7 export-var property** — engine-property name collisions; gated by provable-`@export`-and-not-engine.
3. **M3 attribution** — match a connection to the *right* script via resolved node→script→FileId, not name.
