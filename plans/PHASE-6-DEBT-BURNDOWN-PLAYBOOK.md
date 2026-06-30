# Phase 6 · Tech-Debt Burndown Playbook — close the backlog, then freeze

> The ordered plan to drive `TECH_DEBT.md` to **zero open actionable items**, on branch
> `feat/formatter-scene-rename`, toward the one big PR. Same discipline as W8 and the formatter work:
> **research → develop → bug-hunt → gate (`cargo xtask ci`) → commit → push**, one logical unit per
> commit. Production-grade only — no bandaids, no half-measures, no patching around a problem.
>
> Ordered **easiest → hardest**. The **API freeze (W6) is LAST** and gated by everything above it (see
> the ⛔ marker at the end). A full **documentation pass over every library** is Stage 9.
>
> **Reality check (read first).** The "51 open items" is a *thorough running backlog*, not a defect
> list. Sorted: ~6 are **stale** (done, never checked off), ~8 are **deliberate deviations** (wontfix,
> documented decisions — not debt), and the remaining ~35 are **additive** (new checks / precision /
> infra / docs) — none block 1.0. Stage 0 collapses the first two buckets so the real work is visible.

---

## Stage 0 — Triage & honesty pass (½ day, trivial, do first)

The backlog is inflated by stale + by-design entries. Clear them so the remaining list is real.

- **Flip stale checkboxes** (each verified done before flipping):
  - `name_span` quote-trim — DONE in W8 (`inner_span`).
  - "Type resolution → M1", "Instanced sub-scene recursion → M1+", "script→scene reverse index → M1" —
    DONE in M1–M3.
  - Re-scan the M0/Phase-1/Phase-2 deferral sections for any other item the later milestones closed.
- **Reclassify deliberate deviations** into a new `## Deliberate deviations (wontfix — documented)`
  section so they leave the open-item count: the `is`-narrowing divergence, gdformat's BOM limitation,
  `uid://`-only resolution (user-approved), a node literally named `"."` (engine-impossible), a literal
  `/` in a node name (engine-disallowed), the fully-typed napi `.d.ts` (low value).
- **Outcome:** `TECH_DEBT.md` shrinks from 51 → ~30 genuinely-actionable items, each landing below.
- **Commit:** `docs(tech-debt): triage — flip stale items, separate deliberate deviations`.

## Stage 1 — Easy additive checks + small parser/resolver gaps (each self-contained)

Each is one `Cx::warn` site or a small localized addition, gated by the existing W1 seam; one commit
each with its own no-false-positive bug-hunt + corpus check.

1. **`SHADOWED_GLOBAL_IDENTIFIER` (extend)** — fire for a local/member shadowing a global; route
   through `gate()` as a real `WarningCode` (today only a `class_name` collision).
2. **`ASSERT_ALWAYS_TRUE` / `ASSERT_ALWAYS_FALSE`** — recover the constant bool of the `assert(...)`
   condition from the CST (or extend `Literal::Bool` to carry the value).
3. **`UNTYPED_DECLARATION` / `INFERRED_DECLARATION`** — from the binding `annotated`/`inferred_colon_eq`
   flags; add the opt-in-group `codes()` test filter so they don't pollute focused fixtures.
4. **`unescape` extend** (scene parser) — handle `\uXXXX`/`\UXXXXXX`/`\b`/`\f` in node/parent names.
5. **Cascading-dangling suppression** — track an upstream-dangling set so a node parented to an
   already-dangling sibling is not double-flagged.
6. **`project.godot` `config/features` parsing** — parse the engine version line; consume it through
   the existing `engine_version()` plumbing (currently `[autoload]`-only).
7. **Non-`*` autoload name resolution** — resolve a loaded-but-not-global autoload by name and via
   `get_node("/root/Name")` (a scene/node-path bridge).
8. **Statement-initial bare `match`** — lookahead so `match(...)`/`match.x` as an *identifier* at
   statement start isn't always the `match` statement.

## Stage 2 — Medium additive features

9. **`CONFUSABLE_*` family** (5 codes) — Unicode mixed-script/homoglyph detection via the
   `unicode-security` crate (or the Unicode confusables data); `_TEMPORARY_MODIFICATION` is master-only.
10. **`UNUSED_*` precision** — a read-vs-write split (reuse `ReferenceKind::Write` logic) to catch
    assigned-but-never-read locals; add `UNUSED_PRIVATE_CLASS_VARIABLE` on the existing `NameUses` scan.
11. **Deprecated/misuse + lifecycle checks** still deferred — `FUNCTION_USED_AS_PROPERTY` (needs the
    value-vs-call-context distinction), `MISSING_TOOL`, `REDUNDANT_STATIC_UNLOAD`, `ONREADY_WITH_EXPORT`,
    `REDUNDANT_AWAIT`, `UNSAFE_VOID_RETURN`, `UNSAFE_CAST`, `RETURN_VALUE_DISCARDED`,
    `INT_AS_ENUM_WITHOUT_MATCH`, `DEPRECATED_KEYWORD` (most need the item-tree to capture annotations —
    pairs with item 13).
12. **Hover docs (BBCode → Markdown)** — populate the `DocId`-keyed doc store from the engine doc-XML
    so `HoverResult.doc` is non-empty (signatures-only today).
13. **Annotations as first-class** — capture annotations in the item-tree (today they are sibling CST
    nodes; W8's `is_exported` reads them ad hoc). Unblocks the `@tool`/`@onready`/`@export`-dependent
    checks in item 11 and a `FuncDecl::annotations()` accessor.
14. **Property `get`/`set` parsing** — tighten the permissive accessor grammar (inline + indented).
15. **Trivia attachment** — move from the simple "flush leading trivia to the next node" model toward
    the rust-analyzer leading-vs-trailing heuristic (improves formatter fidelity; coordinate with W3).

## Stage 3 — Test / validation infra (de-risks everything else)

16. **In-repo corpus + CI gate** — vendor (or clone-in-CI) godot-demo-projects; a CI job asserting
    **0 parse errors, 0 panics** over all `.gd` + **0 panics** over all `.tscn` (today ad-hoc examples).
17. **Per-`project.godot` corpus mode** — discover every `project.godot`, one host per sub-project
    (the faithful single-project validation; `--project` merge-mode stays the robustness stress test).
18. **Grow the differential oracle** — beyond the ~14-snippet tree-sitter error-agreement: a structural
    skeleton comparison + a broader snippet set.
19. **Broaden the type-diagnostic corpus** — beyond ReactiveUI-Godot to the demo-projects set; lock a
    no-false-`TYPE_MISMATCH` / no-false-`UNSAFE_*` baseline.

## Stage 4 — Larger semantic precision (real engineering, still additive)

20. **Unify `classify` / `infer` name-lookup** — collapse the two copies of the local → member →
    inherited → global → autoload → engine precedence behind shared `def.rs` helpers (a guard test
    exists; re-validate the Phase-2 byte-identical inference guarantee).
21. **Precise find-refs referrer index** — a firewall-safe reverse index keyed on `item_tree` (not
    bodies) so `Member`/`Global` find-refs stops re-classifying every name-but-no-reference file.
22. **1-script-many-scenes union typing** — when a script attaches to >1 scene, type a `$Path` as the
    common base of the matching nodes (today first-scene-wins; the `ambiguous` flag already gates the
    no-false-positive path).
23. **Instanced sub-scene recursion hard tail** — the override-child-under-an-instance case that still
    degrades to bare `Node`.
24. **Inner-class member navigation identity** — qualify `GodotDef::Member` by the declaring inner-class
    scope so inner members rename instead of refuse (explicitly the ~multi-day item; rippling through
    `classify_decl`/`member_owner`/`resolve_name_to_def`/the collision checks).

## Stage 5 — Flow-narrowing precision (the "multi-year polish" tail — large)

25. **Assignment re-narrowing** (`x = other` → `typeof(other)`) — feed the value's inferred type back
    into the facts (flow currently runs pre-inference, so it can only invalidate on assignment).
26. **`NotNull` / `Not(T)` consumption** — wire the recorded facts into typing once a null-safety
    diagnostic exists to drive them.
27. **Loop-carried back-edge fixpoints, aliasing, narrowing through call results** (`if get_thing() is
    T:`) — the explicitly-out-of-the-1.0-cut W2 tail. **Soundness stays frozen** (widen when unsure).

## Stage 6 — Formatter parity tail (W3 — additive, regression-gated)

28. **`town_scene.gd`** — make `strip_parens` operand-aware so it keeps a redundant paren that wraps a
    tighter-precedence operand of a looser operator chain; then thread the chain's comments around them.
    **Gate against the full corpus** (this changes paren behavior — measure before/after).
29. **`os_test.gd`** — `format_index` layout for a subscript on a large array *literal* (gdformat
    explodes the array; we keep it compact). → godot 454→456 byte-exact.

## Stage 7 — Performance infra (W4 — needs CI services; mostly net-new)

30. **`fixtures/perf/{small,medium,large}`** tiered vendored corpus + a project-scale **cold** bench.
31. **CI bench-regression gate** (CodSpeed / Bencher) + a **wasm-size** byte-ceiling guard (twiggy).
32. **`dhat` memory profiling** + a documented resident ceiling.
33. **salsa-LRU** for cold-file derived data — **measure first**; only if `flow`/`infer` recompute is hot.

## Stage 8 — Distribution / clients / ops

34. **napi cross-platform matrix** — add the deferred Linux musl x2 + armv7 legs (each `experimental`
    `continue-on-error` until green); the spec is in TECH_DEBT.
35. **guitkx (ReactiveUI-Godot) integration** — the separate-repo PR: cross-file *library* go-to-def +
    `analyzerProxy.ts` e2e. **Coordinate** with the publish (it needs the published `@gdscript-analyzer/core`).
36. **LSP bounded worker pool** — replace thread-per-request with a bounded pool + Worker/LatencySensitive
    split (low criticality; salsa cancellation already makes it correct).
37. **Ops cleanups** — `cargo-deny` runnable (or a documented CI-only note), the browser demo artifact
    build, the fully-typed napi `.d.ts` (if kept — else move to deviations).

## Stage 9 — 📚 DOCUMENTATION PASS — every library, properly documented (the new directive)

A complete documentation sweep over **the entire workspace**, not just the main crate. Treated as a
real deliverable with its own gate (`cargo doc` clean; the docgen `--check` stays green).

- **Per-crate READMEs** — author the **4 missing** (`gdscript-cli`, `gdscript-ffi`, `gdscript-lsp`,
  `gdscript-session`) and bring the existing 8 to a uniform shape (what it is, where it sits in the
  layering, public surface vs internal, a runnable example).
- **rustdoc completeness** — add `#![deny(missing_docs)]` to the **contract** crates (`gdscript-ide`,
  `gdscript-base`) and `#![warn(missing_docs)]` to the rest; fill every public item; **doctest** the POD
  docs; add an **"internal — not a stable API"** banner to the non-contract crates.
- **The mdBook** (`docs/src`) — complete the `guide` / `reference` / `clients` / `consume` /
  `contributing` sections; verify `SUMMARY.md` has no gaps; lint-check every link; wire the
  playground-as-live-docs deep links.
- **Bindings** (`bindings/node`, `bindings/wasm`) — document the JS/TS surface + a consume example each.
- **xtask** — document the automation tasks (`ci`, docgen, release helpers).
- **The IDE extensions / addon** — the `ide-extensions~` clients and the guitkx Godot addon live in
  their own repos; **coordinate a docs pass there** (README + usage per editor) and link them from the
  mdBook `clients` page. *(Tracked here so it isn't forgotten; the edits land in those repos.)*
- **The W5 docs tail** (everything except the contract page, which is W6): docs.rs polish, the
  anti-drift docgen `--check` CI gate confirmation.
- **Commit cadence:** one commit per crate/area, each with `cargo doc -p <crate>` clean.

---

## ⛔⛔⛔ Stage 10 — PHASE 6 API FREEZE (W6) — DO NOT TOUCH UNTIL STAGES 0–9 ARE 100% DONE ⛔⛔⛔

> **HARD STOP. This stage is IRREVERSIBLE.** `#[non_exhaustive]` is *itself* a breaking change, so it
> must ride the **final** pre-1.0 release — after every surface-reshaping edit above has landed. Freeze
> a surface that hasn't been battle-tested and you discover the shape is wrong *after* committing to it
> → a forced, embarrassing 2.0. **Do not start any part of this stage until the entire backlog above is
> closed AND the API has been exercised by real use.**

**Pre-freeze gate (the battle-testing — this is the real blocker, not the attributes):**
- **W7 external-consumer** — at least one real adopter beyond guitkx + our own tools, exercising the
  public API in the field. Plus guitkx's integration (Stage 8 #35) landed for real.
- A burn-in period of broad 0.x usage with **no API-shape changes requested** — the signal the surface
  is proven.

**Only then, W6 (mechanical, one careful PR):**
- `#[non_exhaustive]` on all consumer-matched enums/result-structs + `Change` (+ `::new` constructors);
  keep the small value types literal-constructible on purpose.
- `cargo-semver-checks` as a **PR-time** gate (today release-only) + a baseline.
- Public `bundled_godot_version()` / `project_engine_version()` accessors.
- Freeze `Diagnostic.code: String` (joint with W1; `WarningCode::as_str()` bridges it).
- The FFI JSON schema doc + test; the API-review checklist recorded as an ADR.
- **W5 contract page** — the verbatim SemVer policy + the supported-Godot-version matrix (authored
  *with* the freeze).

**Then — and only then — tag 1.0.**

---

## Sequencing note

Stages 0–3 are quick and de-risking (do them first, in order). Stages 4–7 are the real engineering and
can run in any order (they're independent and all additive). Stage 8 coordinates with external repos.
Stage 9 (docs) can begin in parallel any time but **completes** just before Stage 10. Stage 10 is the
gate and the end. Effort is dominated by Stages 4, 5, 7, and 9.
