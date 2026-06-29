# Tech debt & follow-ups

The running backlog of deferred work, known limitations, and queued next steps. Keep it
honest — anything we knowingly defer or stub goes here with enough context to pick it up
later.

> **Marker legend.** `[ ]` = open + actionable work. `[x]` = done. `[~]` = done locally but
> ship-gated on an external step (e.g. a publish). **Deliberate deviations** (intentional
> non-parity / wontfix) are *not* tracked in the backlog at all — they live only in the section
> directly below, so the open-item count reflects only real, actionable work.

---

## Deliberate deviations (wontfix — documented decisions, NOT part of the backlog)

These are intentional, researched choices — recorded here only so they aren't re-litigated. They
are **not** open work and do **not** appear in the phase backlogs below.

- **`is`-narrowing is a Pyright-style value-add, not Godot parity.** Godot's `reduce_type_test`
  does no flow narrowing; ours is **widen-only** (never narrows to a type Godot would reject).
  Intentional UX non-parity. *(Phase 3 → "deferred/found".)*
- **gdformat's BOM limitation is unmatchable by design.** gdformat errors on a leading BOM and
  leaves the file unchanged, so its "gold" for a BOM file is the raw source; we reformat (BOM
  preserved) and legitimately differ. Excluded from parity counts. *(W3 formatter tail.)*
- **`uid://`-only `ext_resource` resolution is deferred (user-approved).** Near-zero real value:
  Godot 4.x writes every `ext_resource` with BOTH `path=` and `uid=`, so path-first resolution
  (implemented) handles every real case. A firewall-safe impl would need a new `uid` salsa field
  plus loader plumbing in both LSP + CLI. Bad cost/value ratio. *(Phase 4 M0.)*
- **A node literally named `"."` is engine-impossible input.** `by_path["."]` can't be returned by
  `resolve_path`; the engine never produces it. Wontfix. *(Phase 4 M0 bug-hunt.)*
- **A literal `/` inside a node name** breaks `/`-segmented path matching, but Godot disallows it
  at edit time. A hand-edited file could violate it; treated as opaque segments. Wontfix. *(M0.)*
- **The fully-typed napi `.d.ts` is intentionally `any`.** napi `serde_json::Value` types as `any`;
  real TS types would need `#[napi(object)]` POD re-declaration, trading the single-source-of-truth
  for DX. The client (guitkx) owns its own TS interfaces. Revisit only if a published `.d.ts`
  becomes a hard requirement. *(Phase 2 FFI ergonomics.)*

---

## Phase 4 — Scene awareness (in progress)

Driven by `plans/PHASE-4-SCENE-AWARENESS.md` + the fact-checked `plans/PHASE-4-M0-PLAYBOOK.md`.

### M0 — the `gdscript-scene` `.tscn`/`.tres` parser — **DONE**
A pure, wasm-clean, never-panic `parse_scene(&str) -> SceneModel` (node tree + ext/sub resources +
byte spans + `SceneProblem`s). Grounded in a primary-source research pass (Godot
`resource_format_text.cpp`/`variant_parser.cpp` + real corpora); the 12 load-bearing corrections
(C1–C12) are folded into the impl and tests. Validated: clippy `-D`, wasm32, 16 tests (the C1–C12
matrix + a vendored real-file corpus), and **524/524 godot-demo-projects scenes parse clean
(8666 nodes), 0 problems, 0 panics**. Records (does **not** resolve) the typing inputs:
`decl_type` / `script` / `instance` / `instance_is_inherited_root` / `instance_placeholder` /
`unique_name_in_owner` / `script_class`.

**M0 known limitations / deferrals (to M1+):**
- [x] **Type resolution → DONE (M1).** `$Path`/`%Unique`/`get_node` resolve to the node's concrete
      `Ty` (native class / `class_name` registry / attached-script refine). See M1 below.
- [x] **Instanced sub-scene recursion → DONE (M3).** An instanced node types as the sub-scene's root
      (`infer::instance_root_ty` follows the ext-resource path, depth-bounded). See M3 below.
- [x] **Project-wide `script→scene` reverse index + salsa caching → DONE (M1).** `script_scene_index`
      (firewalled, keyed on `SourceRoot`) + the `scene_model(db, FileText)` tracked query. See M1.
- [ ] **Inline `script = SubResource("…")` records no attachment.** An inline GDScript sub-resource
      has no external path; M0 sets `script = None` (M1 types the node by its declared `type=`). Rare.
- [x] **`name_span` quote-trim → DONE (W8).** The node `name_span` is trimmed to the bare identifier
      (quotes excluded) via `inner_span`, so a node's own declaration tags as a precise reference.
- [x] **Corpus CI gate → DONE (burndown Stage 3).** A `corpus.yml` workflow clones
      godot-demo-projects (shallow — it carries large binary assets, so it is cloned-in-CI rather than
      vendored) and runs both example harnesses in a new `--ci` mode that **exits non-zero on any
      GDSCRIPT_SYNTAX parse error or panic** (`.gd` via the `gdscript-ide` corpus runner, `.tscn` via
      `gdscript-scene/scene_corpus`). Type diagnostics (`UNSAFE_*`) don't fail the gate. Verified
      locally: **456 `.gd` + 524 `.tscn`, 0 parse errors, 0 panics.**

### M0 adversarial bug hunt (5-finder → 3-vote verify) — fixed + deferred
The post-M0 hunt (9 confirmed, 6 rejected; never-panic + UTF-8 safety signed off) fixed:
- [x] **`..`/absolute (`/root/…`) parent paths false-flagged `DanglingParent`.** Spec §5/§7 say these
      degrade silently. `walk_path` now returns a 3-state `Walk { Resolved | Escaped | Missed }`;
      only a genuine `Missed` is a candidate dangling. (Found 4× independently.)
- [x] **`instance_is_inherited_root` set on spurious extra roots** in a `MultipleRoots` scene — now
      gated on being THE chosen root.
- [x] **Duplicate sibling names: `by_path`/`resolve_path` now first-wins** (`or_insert`), matching
      `unique_nodes`; `children_of` still lists both.

Deferred (low / cosmetic / engine-impossible):
- [x] **`unescape` Unicode + control escapes → DONE (burndown Stage 1).** The scene-string
      `unescape` now decodes `\uXXXX` (4-hex UTF-16, with surrogate-pair combining), `\UXXXXXX`
      (6-hex code point), and the `\a \b \f \v` control escapes, mirroring Godot's
      `variant_parser.cpp` / tokenizer. Invalid / empty escapes fall back safely (lone surrogate →
      U+FFFD). 6 unit tests.
- [x] **Cascading dangling → DONE (burndown Stage 1).** A node parented *through* an already-dangling
      ancestor (whose own parent was missing, so it was never indexed) is no longer double-flagged: the
      parser tracks the intended full paths of detached nodes (`dangling_subtrees`) and suppresses a
      miss whose parent path lies within one — only the root-cause node is flagged. Two siblings that
      independently miss the same parent are each still flagged (not a cascade of one another).

### M1 — scene-aware node-path typing — **DONE**
`$Path` / `%Unique` / `@onready var x := $Path` / `get_node("literal")` resolve to the node's concrete
type (the 90% slice): an attached script's own `class_name` (most specific) wins, else the declared
`type=` (native class or `class_name` registry). Computed `get_node(var)`, an unresolvable path, or
no owning scene all degrade to `Object(Node)` with **no false warning** (the engine floor). Wiring:
salsa `scene_model(db, FileText)` + the firewalled project-wide `script_scene_index(db, root)` (a
`.gd` body edit never invalidates it); `scene_context(db, file)` recovers the owning scene + attach
node (via `self_ty` = the file's own `ScriptRef`, no extra `FileId` threading). `.tscn` is ingested
through the normal `apply_change` path (a `FileText` with a `.tscn` `res://` path). Hover/inlay show
the resolved type automatically. Validated: `xtask ci` green + 7 new typing tests + a public-API
end-to-end inlay test.

**M1 deferrals (→ M2+):**
- [ ] **1-script-many-scenes = first scene wins** for *typing*. `script_scene_index` keeps the first
      attaching scene (now also flagging the attachment `ambiguous`, which M2 uses to suppress false
      `INVALID_NODE_PATH`); the common-base union *typing* policy (Playbook §6.3) is later.
- [x] **`.tscn`-autoload sharpening — DONE (post-LSP tech-debt pass).** A `*`-autoload pointing at a
      `.tscn` now resolves to the scene root's **attached-script `ScriptRef`** (`resolve_scene_autoload`
      in `resolve.rs`, reusing `scene_model` + `res_path_registry`), so `Music.play()` checks the real
      script — no false `UNSAFE`. A script-less root (whose native `type=` would need the engine API
      that `resolve_external` doesn't carry) stays the conservative seam.

### M2 — scene-aware diagnostics & navigation — **DONE**
Built on M1's resolution: **go-to-definition** on a node-path jumps into the owning `.tscn`'s
`[node …]` line (`def::node_path_target` → a `NavTarget` at the node's `name=` span); the
**`INVALID_NODE_PATH`** warning fires on a genuinely-absent in-scene node; **node-path completion**
offers a `$`-path prefix's child node names (typed by their `type=`). The `INVALID_NODE_PATH`
**no-false-positive contract** (4 locked tests): warns only when the path genuinely misses *and* the
script attaches to exactly one scene — silent on `..`/absolute escapes, misses that descend into an
instanced sub-scene, and ambiguous multi-scene attachments (`SceneModel::classify_path_from` returns
the 3-state `NodePathResolution`; `SceneAttach::ambiguous` guards the multi-scene case).

### M3 — instanced sub-scene recursion — **DONE**
An instanced node (`instance=ExtResource("sub.tscn")`, no own `type=`/script) now types as the
**instanced sub-scene's ROOT** node, resolved recursively, so the root's own script / `type=` /
nested instance all flow through (`$Enemy` → `enemy.tscn`'s root class, e.g. `$Enemy.hp()` resolves
the cross-file method). `infer::instance_root_ty` follows the ext-resource path through
`res_path_registry` → `scene_model`, depth-bounded (≤16) against an instancing cycle.

### M1–M3 adversarial bug hunt (5-finder → 3-vote, 3 lenses) — fixed
The post-M3 hunt confirmed **3 distinct false-positive bugs** (all `INVALID_NODE_PATH` / completion
violations; `rejected: []`), each verified end-to-end and now fixed + regression-tested:
- [x] **`%Name/Child` subpath false-warned.** `classify_unique`/`resolve_unique` did a single
      bare-map lookup of the whole joined path (`"Box/Btn"`), missing → false `INVALID_NODE_PATH`,
      though `%Box/Btn` (resolve the unique node, then walk `/Btn`) is idiomatic Godot.
- [x] **`$"%Name"` / `get_node("%Name")` string forms false-warned.** The `%` lived *inside* the
      string (`unique:false`), so it was looked up as a child literally named `"%Name"` → miss →
      false warning.
- [x] **Node-path completion hijacked inside string literals/comments.** `dollar_path_prefix` is a
      pure byte scan; a `$x/` inside `"…"` or `#…` would offer scene node names.

The fix unifies the first two: the path walk (`resolve_path_from` / `classify_path_from`) is now
**`%`-segment-aware** — a `%X` segment resolves scene-wide via `unique_nodes` (the `step_segment`
helper), so leading **and** mid-path `%` work everywhere; `resolve_unique`/`classify_unique` mark the
sigil form's head segment and delegate. The completion fix guards on the `ast::token_at` kind
(`String`/trivia → bail). The bare `$Panel/` completion still works (`Dollar`/`Ident`/`Slash`
tokens); the quoted `$"…"` completion was never byte-scannable, so nothing is lost.

**M2/M3 deferrals (→ later):**
- [x] **Paths *into* an instance — DONE (Phase-5 hardening).** `$Enemy/Sprite` (a node *inside* the
      instanced sub-scene) now types as the inner node's real type, not bare `Node`:
      `SceneModel::resolve_into_instance` returns `(instance_node, tail)` at the boundary and
      `infer::resolve_into_instance_ty` walks the tail from the sub-scene's root, recursing through
      nested instance boundaries (depth-bounded ≤16). A genuinely-absent tail stays `Node` with no
      false `INVALID_NODE_PATH`. An override child *under* an instance (mapping back into the
      sub-scene tree) stays `Node` — the rare remaining tail. Test:
      `path_into_an_instanced_subscene_types_the_inner_node`.
- [x] **`self.get_node("…")` — DONE (post-LSP tech-debt pass).** Explicit `self.get_node("…")` now
      types like the bare form (`self` = the attach node). A *foreign* `obj.get_node("…")` stays a
      normal call → `Node` (correct — its path is relative to a node we can't resolve here).
- [x] **`%Unique` completion — DONE (Phase-5 hardening).** `%Name` is disambiguated from `a % b`
      (modulo) by the parsed token: the byte scan locates the leading `%`, then we confirm its token's
      parent is `UniqueNodeExpr`, not `BinExpr`. A bare `%` offers every unique node in the owning
      scene; `%Box/` resolves `Box` scene-wide and offers its children. Tests:
      `unique_node_path_completion_offers_children`, `bare_percent_offers_all_unique_nodes`,
      `percent_modulo_is_not_hijacked_as_a_unique_path`.
- [x] **Scene-aware rename → DONE (W8, `feat/formatter-scene-rename`).** Renaming a node in a `.tscn`
      (or from a `$Path` in a script) now rewrites the node `name=`, every child `parent=` segment,
      every `[connection] from`/`to` segment, and every `$Path`/`%Unique`/`get_node("…")` segment in
      scripts; renaming a connected method/signal rewrites its `[connection]`; renaming an `@export`
      var rewrites its `[node]` property key. `GodotDef::SceneNode` (identity = scene + full node path)
      classifies a node from both ends; the new scene-parser data is `SceneModel.connections` +
      `SceneNode.{parent_span, properties}`. Correct-or-refuse (WouldCollide on a sibling name; refuse
      on an ambiguous multi-scene script, a `.gd`-string the analyzer can't attribute, an
      un-attributable connection). Validated: ~13k renames across the godot-demo-projects subprojects
      (9 projects) — **0 panics, 0 corrupting edits** (apply + re-parse). See
      `plans/PHASE-6-W8-SCENE-RENAME-PLAYBOOK.md`.

---

## Repo / ops state

- **Branch protection:** `dev` and `master` are governed by the **"Protect dev + master"**
  ruleset (PR-only, required status checks incl. `pr-title`, restrict deletions,
  non-fast-forward). **`delete_branch_on_merge` is OFF.** GitHub's auto-delete-on-merge
  *bypasses* the ruleset's deletion rule and had silently deleted `dev` when the
  `dev → master` PR merged (it deletes the PR's head branch). Disabling it keeps `dev`
  permanent; merged **feature** branches are cleaned up manually instead.

---

## Phase 1 — deferred / known limitations

### Build & CI
- [ ] **napi `.node` is CI-built only.** napi-rs v3 needs `libnode.dll` on Windows
      (provisioned by `@napi-rs/cli` on CI runners, not plain `cargo`), so the Node
      addon and `bindings/node/hello.mjs` were never run locally. The `bindings` CI job
      is `continue-on-error` until the Phase-5 cross-platform publish matrix is wired.
- [ ] **cargo-deny not run locally.** It won't compile under the local windows-gnu
      toolchain, so license/advisory policy is only enforced in CI. New transitive deps
      may need an entry in `deny.toml`'s `allow` list — watch the `cargo-deny` job.
- [ ] **Browser demo artifact not produced locally.** `bindings/wasm` is verified to
      compile to `wasm32-unknown-unknown`, but `wasm-pack build --target web` (the JS
      glue for `playground/hello.html`) wasn't run locally (no wasm-pack installed). The
      `bindings` CI job builds it.

### Parser / syntax
- [ ] **Trivia attachment is the simple model — DEFERRED (formatter-entangled, marginal now).** The
      tree sink flushes leading trivia into the *following* node; it does not implement
      rust-analyzer's `n_attached_trivia` leading-vs-trailing heuristic (same-line trailing comment,
      blank-line breaks, doc-comment pull). It is lossless (round-trip is byte-exact regardless of
      grouping). The motivating consumer was the formatter — but the formatter already reaches
      **99.6 % gdformat parity** via its own comment-threading tuned to the *current* model, so
      re-attaching trivia now would risk regressing that solid component for a marginal fidelity gain.
      The right time is a coordinated W3 formatter-fidelity pass with full corpus re-validation, not a
      standalone refactor — so it "harms more than helps" right now. *(Deferred with rationale.)*
- [x] **Annotations as first-class → DONE (burndown Stage 2).** The item tree now lifts decorator
      annotations onto each item: `FuncItem`/`VarItem`/`ConstItem`/`SignalItem` carry
      `annotations: Vec<AnnotationItem>` (name + range, source order), and `ItemTree` carries the
      class-level ones (`@tool`/`@icon`/`@static_unload`/`@abstract`). `VarItem::is_exported` is now
      derived from them. `item_tree::has_annotation` is the accessor. (The CST still holds them as
      sibling nodes — losslessly — but consumers no longer re-walk siblings ad hoc.)
- [x] **Property accessor (`get`/`set`) parsing tightened → DONE (burndown Stage 2).** The accessor
      keyword must now be exactly `get` or `set` — a different identifier (a typo) is a parse error
      with recovery, not a silently-accepted setter; and only `set` consumes a `(value)` parameter (a
      `get(x)` is rejected). Valid inline (`get = f, set = f`) and indented blocks are unchanged; corpus
      (2d/3d/gui): 0 new `GDSCRIPT_SYNTAX`.
- [x] **Soft-keyword identifiers — `match`/`when` supported.** Godot's `is_identifier()`
      whitelist is `match, when, PI, TAU, INF, NAN`; the parser now accepts `match`/`when`
      as names (declaration / parameter / identifier expression) and the full
      `is_node_name()` keyword set after `.` (verified against `gdscript_tokenizer.cpp`).
      The four math constants stay literal tokens (their near-universal use), so
      `var PI = …`-style shadowing of a constant isn't modeled — a deliberate choice, not
      a gap.
- [x] **Statement-initial bare `match` lookahead → DONE (burndown Stage 1).** `stmt()` now treats a
      statement-initial `match` used as an *identifier* as an expression, not the match statement:
      `match.x` (member), `match = …` / `match += …` (assignment), and `match(x)` / `match[i]` whose
      bracket group is NOT colon-terminated (a parenthesised/array match *subject* `match (x):` stays a
      statement). The disambiguation (`Parser::match_begins_statement`) reads the raw token buffer
      directly (fuel-free, unbounded-safe); the keyword reading is the safe default for any ambiguity.
- [x] **UTF-8 BOM at file start — FIXED.** A leading `U+FEFF` is now lexed as a dedicated
      `Bom` trivia token (not `Whitespace`, so it does not mis-count the first line's indent;
      not `Error`, since the file is valid GDScript). It round-trips byte-for-byte and the
      first declaration parses clean. Regression test:
      `leading_utf8_bom_is_trivia_not_an_error`. (Real: some editors save `.gd` with a BOM —
      one file in the ReactiveUI-Godot corpus did, and it now analyzes clean.)

### IDE features (Tier 0 → Tier 1)
- [x] **Scope-aware completions — DONE (Phase-5 hardening).** By-name completion now offers a
      parameter / local `var`/`const` ONLY inside its owning function; class members stay visible
      everywhere. The enclosing function is found by an **indentation scan** (`enclosing_func_offset`
      in `features.rs`), NOT the CST `FuncDecl` range — that range stops at the last body token, so
      typing on a fresh empty line at the end of a body (the common case) is *past* it and a range
      test would wrongly HIDE the body's own params/locals (the prior attempted-and-rejected fix).
      Tests: `completion_is_scope_aware_for_locals_and_params`,
      `completion_at_class_level_offers_members_not_locals`.
- [x] **Type inference / member completion / hover / inlay / signature help / code
      actions — DONE in Phase 2; goto-def / find-refs / rename / workspace symbols — DONE in
      Phase 3 M5** (cross-file, resolve-don't-string-match; `goto_definition` returns real targets).
- [x] **salsa / incremental reparse — DONE in Phase 3 M0.** The plain VFS map was replaced by a
      salsa query graph (`FileText` inputs, tracked `parse`/`item_tree`/`analyze_file`, real
      cancellation, the body-edit firewall). Every derived computation stayed a pure `(text) -> value`
      function, so the swap was localized + byte-identical to Phase 2.

---

## Phase 2 — deferred / known limitations

### Deliberately phased (NOT shortcuts — scoped per the roadmap)
- [~] **guitkx adapter migration to the typed binding — MIGRATED + VALIDATED locally; ship gated on
      publish.** The adapter (`…/ReactiveUI-Gadot/ide-extensions/lsp-server/src/analyzerAdapter.ts`)
      was migrated to the typed contract: dropped the four `JSON.parse(...)` calls
      (`completions`/`hover`/`diagnostics`/`gotoDefinition`), now reads the result's `d.uri` field,
      and **deleted** the `fileIds`/`nextId`/`track()` id↔uri mirror (`docs` collapsed to `uri→text`).
      Validated by `npm link`-ing the locally-built `.node` into the LSP server: `tsc` clean + all
      **32 tests green**, incl. the cross-file-goto test (now driven by the binding's `uri`, not a
      mirror). **Only the SHIP is gated:** committing it needs `@gdscript-analyzer/core` published at
      the new version + a `package.json` dep bump, else a clean `npm install` of guitkx pulls the old
      0.2.x and the typed-contract adapter crashes. So: hold the guitkx commit until this branch
      merges → releases → publishes, then bump the dep and land the (already-validated) adapter diff.
- [x] **Cross-file resolution → DONE (Phase 3).** `class_name` globals, autoloads, `preload`,
      script `extends`, and `as`/`is` against user types now resolve through the reimplemented
      `resolve_external` (M1–M4). See the Phase 3 section.
- [x] **Scene-aware node typing → DONE (Phase 4).** `$Node` / `%Unique` / `get_node()` resolve to
      the concrete child type via `.tscn` parsing (M1–M3). See the Phase 4 section.
- [ ] **Full 48-warning set + project-settings gating + real CFG narrowing → Phase 6.**
      Phase 2 ships the MVP subset (INFERENCE_ON_VARIANT, TYPE_MISMATCH, NARROWING_CONVERSION,
      INTEGER_DIVISION, UNSAFE_PROPERTY/METHOD_ACCESS); `is`-narrowing is lexical/syntactic,
      not a real control-flow graph; `@warning_ignore` gating is not applied.
- [ ] **Hover docs are signatures-only — BLOCKED ON A DATA DECISION (needs the user).** The
      `DocId`-keyed doc store is wired into the model but not populated. The pipeline (a BBCode→Markdown
      converter + a codegen doc-XML reader + hover wiring) is straightforward, BUT it needs the Godot
      `doc/classes/*.xml` corpus, which is **deliberately not vendored** (`vendor/godot/4.5-stable/
      SOURCE.txt`: ~900 files / ~6–8 MB) — and baking the descriptions into the bundled engine blob
      grows the **wasm bundle** the playground downloads. So enabling hover docs is a repo-size + wasm-
      size tradeoff the team explicitly avoided in Phase 0; **reversing it should be an explicit
      decision**, not a unilateral burndown edit. (Deferred here, surfaced to the user — not skipped.)

### Genuine workarounds to revisit (flagged honestly)
- [x] **Lambda-call parser bug — FIXED at the root.** A multi-line lambda followed by a line
      that starts with `(` (e.g. `var cb := func(): …` then
      `(loop as SceneTree).process_frame.connect(cb, …)`) used to be mis-parsed: the `(` on the
      next logical line was absorbed as a *postfix call on the lambda*. The fix is in the parser
      (`grammar.rs`): `block()` now reports whether it parsed an *indented* (multi-line) body, and
      `lhs()` does **not** run a postfix chain after a block-body lambda — its trailing `DEDENT`
      terminates the expression, so the `(` line starts its own statement. The inference rule
      (calling an arbitrary expression yields the seam, not `Variant`) was kept on its own merit
      — it now only covers genuine `Callable`-value invocation, not a parser artifact. Regression
      tests: `multiline_lambda_does_not_absorb_following_paren_line`,
      `inline_lambda_still_chains_postfix` (parser), `multiline_lambda_then_paren_line_no_false_warning`
      (hir).
- [x] **Member field types — bounded fixpoint (W2-MEMBER-FIXPOINT).** `analyze_file` Pass 1 now
      re-infers every field initializer against the prior round's `member_types` until the map
      stops changing or 4 rounds elapse (cheap, deterministic, throwaway probe rounds — only the
      converged round's units/diagnostics are kept). A field whose initializer references an
      *earlier* field (`var a := 1` then `var b := a + 1`) now types `b` as `int` instead of
      seeing `a` as `Variant`/seam — no false `INFERENCE_ON_VARIANT`. Tests:
      `field_inferred_from_earlier_field_is_typed`, `field_forward_reference_is_seamed_not_warned`,
      `standalone_inferred_field_unchanged` (no-regression).
- [x] **`await` of a coroutine call recovers its return type — DONE (Phase-5).** `await` is now
      *identity* on a non-signal operand (`await f()` for `func f() -> int` is `int`), recovered in
      `infer.rs`. Still the seam (deliberately): **`await sig`** (the signal's emitted payload needs the
      Phase-3+ signal-signature table) and **inner-class `inner_instance.field`** types. Tests:
      `await_a_coroutine_call_recovers_its_return_type`, `await_a_signal_stays_the_seam`.

### Validation
- [ ] **Type-diagnostic corpus is one project.** Validated on ReactiveUI-Godot (89 `.gd`):
      **0 panics, 0 false `TYPE_MISMATCH`**; total diagnostics 446→57 after hardening. The 2
      residual `INFERENCE_ON_VARIANT` are *true* positives (an explicit `-> Variant` return; an
      untyped operand) and the 53 `UNSAFE_*` are the intended value-prop warnings the engine
      ignores by default (§5). Broaden to the Godot demo-projects corpus before v1.

### FFI ergonomics
- [x] **Bindings return native JS values, not JSON strings — DONE (Phase 3, `feat/w1-warnings`).**
      `gdscript-session` now returns `serde_json::Value` (was a JSON `String`); the napi binding
      converts it directly via the `serde-json` feature and the wasm binding via
      `serde_wasm_bindgen` (`Serializer::json_compatible()` — REQUIRED, else `Value::Object`
      serializes as a JS `Map`, breaking `result.field`). No client-side `JSON.parse`. The single
      source of truth stays the `gdscript-base` POD (no `#[napi(object)]`/POD re-declaration in the
      binding crates — the `Value` route keeps them trivial delegators). Verified locally: 15
      `gdscript-session` unit tests + the wasm32 build/clippy + the full `xtask ci` gate, **plus an
      end-to-end napi run**: the `.node` builds with the MSVC toolset and `bindings/node/hello.mjs`
      confirms native-object returns + a cross-file goto target carrying its `uri`. (A generic
      contributor still needs the VS C++ workload to build the `.node` locally — otherwise it is
      CI-built; `hello.mjs` runs in the CI node-smoke job.)
  - [x] **Mirror-free navigation.** The session injects a `"uri"` next to every `"file"` id in a
        serialized result (a generic walk over `NavTarget`/`Reference`/`FileEdit`/`WouldCollide`),
        so a client (guitkx) resolves cross-file targets without maintaining its own `FileId`→URI
        mirror. Zero false-positive surface (every `gdscript-base` `file` field is a `FileId`).

### Validation
- [x] **Differential oracle grown → DONE (burndown Stage 3).** The tree-sitter error-agreement set
      went from ~16 to **28** core-GDScript snippets (default params, static funcs, compound assign,
      `await`, annotations-with-args, multi-line strings/dicts, `@tool`, `is`/`not`, …) + more
      both-reject broken cases, and a new **structural skeleton cross-check** (`top_level_function_count
      _agrees`) asserts both parsers see the same number of top-level functions — beyond pure
      error-agreement. The broad per-file corpus gate (456 demo files, 0 parse errors) is the
      complementary breadth check. (tree-sitter's missing-`:` leniency stays in `KNOWN_DIVERGENCES`.)
      *(The parser is now also exercised by `cargo run -p gdscript-ide --example corpus --
      <dir>` against real projects — the ReactiveUI-Godot codebase parses **88/89 files
      clean, 0 panics**; the one remaining diagnostic is the BOM item above.)*

---

## Phase 3 — progress & findings

### Done (branch `feat/phase-3`)
- **M0 — salsa substrate + VFS migration.** salsa 0.27.1 (wasm32-clean, getrandom-free),
  `FileText`/`SourceRoot` inputs, tracked `parse`/`item_tree`/`analyze_file`, real cancellation,
  the body-edit firewall CI gate. Byte-identical to Phase 2.
- **M1 — global `class_name` resolution.** `global_registry` (offset-free `file_class_name`
  projection → firewalled), `Ty::ScriptRef` activated (member access, is_assignable, hover label).
  ~85% of real demand. Project-mode corpus 54→57 = 3 *true-positive* `INFERENCE_ON_VARIANT`
  (cross-file untyped returns) the seam previously hid; 0 false positives.
- **M2 — base-chain inheritance.** `script_class` records its `extends` base; member lookup walks
  own → user base (`ScriptRef`) → engine base (API table), depth-bounded. Validated on
  **godot-demo-projects (456 `.gd`): 0 panics**; cross-file adds only +14 diags over per-file
  (+1 `TYPE_MISMATCH` = a cross-*project* `class_name` collision artifact of merging ~30 demos,
  not a real bug).
- **M3 — `preload`/`load` const-aliasing + `res://` path map.** `res_path` is a new `MEDIUM`-durability
  field on the `FileText` salsa input (salsa tracks input fields *individually* — verified against
  `salsa-0.27.1/src/input.rs` `revisions[field_index]` + its own `expect_reuse_field_x…field_y` test —
  so it backdates across `text` keystrokes, same firewall as `file_class_name`). `res_path_registry`
  (path → `FileId`, keyed on `SourceRoot`) mirrors `global_registry`; `preload("res://x.gd")` and
  `extends "res://x.gd"` resolve through it to the declaring file's `ScriptRef` (reusing `script_member_walk`
  — no new meta-type variant, since the analyzer already collapses meta-vs-instance like a bare `class_name`).
  Resolution is by **path**, so a script with *no* `class_name` is still preloadable (`reduce_preload` does
  the same). `load("…")` was corrected from `Variant` → **`Unknown` (the seam)** so `var r := load(…)` no
  longer false-warns and is never aliased to `preload` (Godot: `load` is a runtime call returning an opaque
  `Resource`). Validated: reference corpus **57 → 57** (zero regression, paths layout-verified), 2nd corpus
  **456 files, 0 panics**; an end-to-end public-API test proves a real `const M = preload(…); M.new().parse()`
  yields a typed `: int` inlay. The loader supplies paths via `Change::set_file_path` (on add only — a
  keystroke must omit it, since salsa bumps a field's revision on *every* set, even an identical value).
- **M4 — autoloads + `is`/`as` user narrowing.** `project.godot` is injected as raw text into a new
  `ProjectConfig` salsa input (MEDIUM, mirrors `SourceRoot`/`res_path`); a line-oriented
  `project::parse_autoloads` (NOT a full ConfigFile/Variant port) feeds `autoload_registry`
  (`*`-singletons only — `Name="*res://…"`, `*` stripped per `project_settings.cpp` `begins_with("*")`
  + `substr(1)`; non-`*` = loaded-but-not-global). `resolve_external(Autoload)` resolves a `.gd`
  singleton by **path** → its `ScriptRef` (so a `class_name`-less autoload still resolves + members
  walk); the autoload tier sits after `class_name` in `resolve_name`. `is`/`as` over user types was
  found to **already work** (the `!is_uninformative` guard never blocked the informative `ScriptRef`)
  — M4 only added the **widen-only** refinement (`is_subtype` composing the script `extends` chain
  with engine `is_subclass`): `if d is Base` where `d: Derived` keeps `Derived`. Validated: reference
  corpus **57 → 57** (additive, 0 autoloads there); a real autoload subproject (godot-demo-projects
  `2d/physics_tests`, `Log`/`System` singletons) **0 panics**; an end-to-end public-API test resolves
  `Audio.volume()` (a no-`class_name` `*`-autoload) to a typed `: int` inlay.
- **M5 — cross-file navigation (find-refs, rename, workspace symbols, goto-def) — EXITS PHASE 3.**
  New `gdscript-hir/src/def.rs`: `GodotDef` (stable identity — `class_name` global → decl file;
  member → owner file + name; local → body + decl range; autoload; engine) + `classify(db, pos)`,
  the inverse of inference. `gdscript-ide/src/navigation.rs`: the four features with rust-analyzer's
  **resolve-don't-string-match** discipline (word-boundary pre-filter → re-`classify` each candidate
  → keep iff it equals the cursor's `GodotDef`). **Rename is correct-or-refuse** (zero false edits):
  refuses on an autoload (its `project.godot` key isn't rewritten), on a method/var/signal whose name
  appears as a project string literal (possible `connect`/`Callable`/scene-`[connection]` ref),
  collisions (`WouldCollide`), invalid identifiers, and engine symbols. A `class_name` rename
  **proceeds** (research finding: `.tscn`/`project.godot` reference scripts by *path*, the `.godot`
  cache is *derived*). `SourceChange` became multi-file (`Vec<FileEdit>`); `goto_definition` now
  returns `Vec<NavTarget>` (was a stub). No persisted reverse-index — on-demand folds over the
  memoized queries (no new tracked query / invalidation edge). Found + fixed 3 real `classify` bugs
  (decl/ref range consistency, the leading-whitespace `name_range` quirk, `self.member`). 5 def + 11
  navigation + 1 e2e tests (incl. the adversarial same-name set). Reference corpus 57 → 57.

### Deferred / found
- [x] **`global_registry` first-wins SILENTLY → collision/shadowing diagnostic — DONE (W2).** A new
      `class_name_collisions` tracked query mirrors `global_registry`'s firewall (offset-free
      `file_class_name`, names declared by >1 file); `analyze_file` emits ONE
      `SHADOWED_GLOBAL_IDENTIFIER` Warning at the `class_name` NAME range when the name is a
      cross-file duplicate, shadows an engine/native class or builtin/utility/global
      (`resolve::resolve_global`), or shadows a `*`-autoload singleton. Conservative: no source root
      (single-file) or no `project.godot` ⇒ the seam, no warning. `file_class_name` stays the
      firewall projection.
- [x] **`extends "res://path.gd"` + `preload` need a `res://` → `FileId` map — DONE in M3** (above).
      `load(var)`/`load("lit")` stay opaque by design (D5).
- [x] **(M5) Scene rewriting → DONE (W8).** `.tscn` scenes are ingested (Phase-4 scene crate) and a
      rename now rewrites scene references: method/signal `[connection]`s and `@export`-var `[node]`
      properties are **attributed + rewritten** (resolve-confirmed: the connection/property node must
      attach this exact script), closing the old probabilistic gap (a scene-only `[connection]` was
      previously invisible → a method rename silently missed it). The `.gd`-string refuse is retained
      only for an *un*attributable string (a dynamic `connect`/`Callable`/`get_node(var)`).
      `project.godot` `[autoload]` is still read-only to rename (an autoload-name rename refuses).
- [ ] **(M5) `classify` duplicates `infer.rs`'s name-lookup order.** Two copies of the local → member
      → inherited → global → autoload → engine precedence (one returns a `Ty`, one a `GodotDef`).
      Unify behind shared `def.rs` helpers once the Phase-2 byte-identical inference guarantee can be
      re-validated. **Guard added (Phase-5):** `classify_and_infer_agree_on_local_shadowing_a_member`
      (gdscript-ide) locks in that goto-definition (classify) and hover (infer) resolve a use to the
      SAME declaration under local-over-member shadowing — so a future drift fails CI. The full
      unification behind shared helpers is still TODO.
- [ ] **(M5) `Member`/`Global` find-refs scope is project-wide-candidates, not a precise referrer
      graph.** Correct (the re-resolve confirms) but does wasted `classify`s on files that name-but-
      don't-reference the symbol. A firewall-safe referrer reverse-index (keyed on `item_tree`, not
      bodies) is a perf follow-up if the large-project benchmark regresses.
- [x] **(M5) `ReferenceKind::Write` — DONE (Phase-5 hardening).** find-refs now tags a write when the
      reference is the direct LHS operand of an assignment `BinExpr` — a bare `NameRef` (`x = …`,
      `x += …`) or the member of a `FieldExpr` (`self.x = …`, `a.b = …`). Conservative: a receiver
      (`a` in `a.b`), an index target (`arr[i] = …`), `==` (EqEq), and `:=` declarations stay `Read`.
      Test: `find_refs_distinguishes_writes_from_reads`.
- [x] **Scene (`.tscn`) autoloads → root script — DONE (post-LSP tech-debt pass).** A `*`-autoload
      pointing at a `.tscn` now resolves to its root node's attached-script `ScriptRef` (Phase-4 scene
      parsing unblocked it — `resolve_scene_autoload`), so `Music.play()` checks the real script. A
      script-less root or a `.cs` autoload stays the seam (the latter out of scope).
- [x] **Non-`*` autoload via `get_node("/root/Name")` → DONE (burndown Stage 1).** The
      `AutoloadRegistry` now also tracks loaded-but-not-global autoloads (`resolve_any_path`), and an
      absolute `/root/<Name>` node path resolves to the autoload's type (singleton or not — both live
      at `/root/Name`) via `resolve::resolve_autoload_any`. Bare-name resolution is unchanged (a non-`*`
      autoload is still NOT a global). A deeper tail (`/root/Name/Child`) stays the `Node` seam.
- [x] **`project.godot` `config/features` parsing → DONE (Phase-5 hardening §6).** The engine version
      line is parsed into the `engine_version()` salsa query + `project_engine_version()` plumbing
      (informational until Phase-6 multi-version API bundling). `[autoload]` is no longer the only line read.
- [x] **Per-`project.godot` corpus mode → DONE (burndown Stage 3).** The corpus runner gained
      `--per-project`: it discovers every `project.godot` under the root and analyzes each sub-project
      in its OWN host — the faithful single-namespace validation (one `project.godot`, one `class_name`
      namespace, cross-file resolution active). On godot-demo-projects: **138 projects, 456 files, 0
      parse errors, 0 panics** — and only **120** type diagnostics vs **1973** in per-file mode (full
      project context resolves the seam-induced `UNSAFE_*`). Wired into the corpus CI gate; the merged
      `--project` mode stays the cross-project robustness stress test.
- [x] **Relative `preload`/`extends` paths (`preload("sibling.gd")`) — DONE.** Anchored to the importing
      script's dir (`get_base_dir().path_join(p).simplify_path()`) via `resolve::anchor_res_path`, then
      resolved through the `res://` path map. Absolute + relative both handled (`anchor_res_path` tests).
- [x] **Cross-*file* `preload`-const member access — DONE (Phase-5, the firewall path).** `const X =
      preload("res://x.gd")` read from *another* file (`other.X`) now resolves to the preloaded script's
      `ScriptRef`. The preload path is **signature-level** (a const decl is not a function body), so
      `ItemTree::ConstItem` records `preload_path` and `script_class` resolves it via
      `resolve_external(Preload)` — without breaking the body-edit firewall (123 hir tests incl. the
      firewall tests stay green). Test: `cross_file_preload_const_member_resolves`.
- [x] **Parser gaps on the broader demo-projects corpus — DONE (Phase-5 hardening): 307 → 0.**
      Project-mode over godot-demo-projects (456 `.gd`) surfaced **307 `GDSCRIPT_SYNTAX`** errors
      (0 panics), almost all cascading from THREE unhandled-but-valid forms, now fixed:
      (1) a statement-level annotation inside a function body (`@warning_ignore("…")`) — `stmt()` now
      parses a leading `@` as a sibling `Annotation`; (2) a multi-line lambda passed as a call
      argument with the closing `)` on its own dedented line, indented BETWEEN the lambda header and
      its body (the tween demo) — the prepass closes such a body by BRACKET DEPTH when a line leads
      with a closer; (3) a multi-line lambda whose single-statement body is followed by `, more_args`
      on the same line — a bare `,` at the lambda's enclosing bracket depth ends the body. Result:
      **godot-demo-projects parses with 0 `GDSCRIPT_SYNTAX` errors, 0 panics**. Tests:
      `statement_level_annotation_in_a_body`, `multiline_lambda_arg_with_dedented_closer`,
      `multiline_lambda_body_ending_at_a_comma`. **Still open:** grow the *differential* (tree-sitter)
      oracle + a CI gate that clones godot-demo-projects and asserts 0 parse errors (the run is ad hoc
      via `cargo run -p gdscript-ide --example corpus -- <dir>` today).
- [x] **`corpus --per-project` faithful run → DONE (burndown Stage 3).** `--project` (merge) stays
      the cross-project robustness stress test; `--per-project` is the faithful `project.godot`-scoped
      validation (one host per project). See the per-project entry above.

### Post-M5 bug hunt (adversarial 6-finder + 3-vote-verify pass over all Phase-3 code)

**Fixed in this pass** (11 confirmed defects — find-refs/rename correctness, the no-false-positive
seam, and rename identifier hygiene; all with regression tests):
- [x] **`classify` missed `extends Base`** (bare `Ident`, not a `Name`/`TypeRef` node) → a
      `class_name` rename left `extends ThatClass` stale (incomplete, corrupting edit). Fixed:
      `cst::extends_head_token` + a classify branch resolving the extends head as a type name.
- [x] **Member `name_range` carried leading whitespace** (the `Name` CST node absorbs the
      inter-token space) → off-by-one focus ranges + a member's own declaration mis-tagged `Read`.
      Fixed at the root in `item_tree::name_range` (trim to the bare identifier).
- [x] **Inner-class member over-rename (CRITICAL).** `GodotDef::Member` identity is `(file, name)`
      with no inner-class discriminator, so an inner `class Inner: func update` shared identity with
      a top-level `func update` → rename rewrote BOTH (cross-class corruption). Fixed: `classify`
      returns `None` for a declaration nested in an `InnerClassDecl` (correct-or-refuse). Full
      inner-class navigation identity is deferred (see below).
- [x] **Local in a `get`/`set` accessor (or class-level-lambda) body mis-classified as a Member.**
      The discriminator only checked for a `FuncDecl` ancestor; broadened to `Getter`/`Setter`/
      `LambdaExpr` too.
- [x] **`resolve_name_to_def` picked the first same-named binding (scope-unaware)** → a shadowed
      local reference resolved to the wrong binding (e.g. a param instead of the shadowing local),
      conflating two distinct locals in find-refs / rename. Fixed: pick the nearest-PRECEDING
      declaration (greatest start `<=` the reference offset = lexical shadowing).
- [x] **`match`-pattern `var` captures were invisible to navigation** (never recorded as bindings)
      → a capture reference mis-resolved to a same-named member, corrupting its rename. Fixed:
      `MatchArm` binds now carry a range, infer records a `BindingKind::MatchBind`, and `classify`
      routes a `PatternBind` decl to a local.
- [x] **Rename of an inner-class / named-enum member was a partial edit** (its `var x: Inner` /
      `: MyEnum` type-annotation uses aren't resolvable by `classify_type_name`). Fixed: refuse
      renaming a `Member` of kind `Class`/`Enum`.
- [x] **`is_valid_ident` accepted reserved words as the new name** (`assert`, `namespace`, `yield`)
      and the math-constant tokens (`PI`/`TAU`/`INF`/`NAN`) → a rename could write invalid code.
      Added them to the keyword reject set.
- [x] **Global rename collision ignored engine/native class names and autoload singletons.**
      `class_name Widget` → `Node` (or an autoload name) passed the collision check. Fixed: also
      reject when the new name resolves to an engine global or an autoload singleton.
- [x] **`preload`/`extends "res://…"` of a non-`.gd` resource could resolve to a script `ScriptRef`.**
      `resolve_res_path` returned a `ScriptRef` for any registered path; a future scene-ingesting
      loader would mis-type `preload("res://x.tscn")` (accepting bogus `.new()`/member access).
      Gated `resolve_res_path` on `.gd` (latent today — only `.gd` is indexed — but defensive).
- [x] **Global `WouldCollide` reported the colliding symbol at byte `(0,0)`** instead of its real
      `class_name` declaration range. Fixed via `class_decl_target`.

**Second fix pass** (the high-confidence deferrals, fixed with regression tests):
- [x] **Aliased `self` false `UNSAFE` — FIXED.** `self` is now typed as the file's *own* `ScriptRef`
      (`ClassScope::self_ty`, set by `analyze_file` from the `FileId`), not just its engine base — so
      `var me := self; me.own_method()` resolves the script's own members instead of false-warning.
      Uniform for engine-base *and* user-base files (a user-base file's aliased `self` previously
      pointed at the *base*, missing own members). Safe by construction: `is_assignable` treats
      `ScriptRef → Object` as `Ok` (no new `TYPE_MISMATCH`); direct `self.member` keeps the precise
      own-member fast path; member completion now walks a `ScriptRef`'s own + base-chain members.
      Reference corpus **57 → 57** (no regression — the pattern doesn't occur there; the fix is
      proven by a unit test, and demo-projects 456 files stays at 0 panics).
- [x] **Member-rename inherited-collision — FIXED.** `collision_check` now walks the user `extends`
      chain (`user_base_member_decl`), so renaming `Derived.own → shared` where `shared` is on the
      user base `Base` is refused (`WouldCollide`). Engine-base members stay out of scope.
- [x] **Anonymous-enum variant navigation — FIXED.** An anon-enum variant (`enum { FIRE }`) now
      classifies to a `Member` identity (`member_owner` / `classify` consult the anon-enum
      flattening), so find-refs, goto-definition, and rename reach it; its declaration is located by
      a parse scan (`anon_enum_variant_target`) since `item_tree` drops per-variant ranges.

**Deferred** (verified real, but needing an AST-layer change or pairing with later inner-class work):
- [ ] **Inner-class member navigation identity is not modeled — considered + DEFERRED (Phase-5).**
      Inner members refuse rather than corrupt (safe today). A full fix qualifies `GodotDef::Member` by
      the declaring inner-class scope and resolves against the inner `ItemTree`, rippling through
      `classify_decl` / `member_owner` / `resolve_name_to_def` / the rename collision checks — a
      deliberate ~multi-day project, not a quick win, so explicitly deferred in the hardening pass.
- [x] **Symbols named with soft keywords (`match`/`when`) — DONE (Phase-5 hardening).** `Name::text()`
      and `EnumVariant::text()` now read the grammar's `at_name` whitelist (`Ident | MatchKw | WhenKw`)
      via a `name_token_text` helper, so such symbols reach item_tree / hover / completion. `classify`
      treats a soft keyword as a symbol only in a name position (`Name`/`NameRef` parent), so a bare
      `match` *statement* keyword stays a non-symbol. Tests:
      `soft_keyword_names_are_not_dropped` (item_tree), `soft_keyword_named_member_is_navigable` (nav).
- [x] **`extends "res://base.gd".Inner` (string + dotted) — DONE (correct-or-refuse).** Was resolving
      the base to the OUTER script (wrongly accepting its members). `parse_extends_tokens` now detects the
      trailing dotted selector and yields the new `ExtendsRef::ScriptPathInner`, which `resolve_base` routes
      to the seam (`Unknown`) — never the outer script. The full inner-class resolution still pairs with
      inner-class modeling. Test: `extends_script_path_with_inner_class_is_distinguished`.

---

## Phase 5 — Clients & Distribution

### Done
- [x] **Standalone LSP `gdscript-lsp` whole-project loading.** On `initialized` the server walks the
      workspace to `project.godot`, loads every `.gd` + `.tscn` (with `res://` paths) + the project config
      into one host — so class_name / autoloads / preload / scene typing work, and nav/rename span the
      whole project (not just open docs). A canonical-path VFS interner layers an open overlay over the
      disk layer (no double-load / false collision). `workspace/didChangeWatchedFiles` keeps it in sync
      with external edits. (`project.rs`, `vfs.rs`, `lib.rs`; tests
      `whole_project_loads_and_resolves_cross_file_without_collision`,
      `watched_file_creation_lights_up_cross_file_resolution`.)
- [x] **CLI rustc-style human output (annotate-snippets) + config discovery.** `--config`/`--no-config`
      were dead flags; now a `gdscript-analyzer.toml` is discovered (walk-up), an explicit file / inline
      `key=value` override / `--no-config` are honored, carrying `error_on_warning` (the option set is
      intentionally minimal — the warning taxonomy is Phase 6). `CLICOLOR=0` honored. Dropped the unused
      `anstream`/`anstyle` deps.
- [x] **napi win-arm64 (`aarch64-pc-windows-msvc`)** added to the publish matrix (a native MSVC cross).
- [x] **Web playground = a real Monaco editor** (CDN AMD loader, build-less) with live diagnostics
      (`setModelMarkers`) + hover/completion/signature-help providers over the wasm `Analyzer`.
- [x] **wasm bundle size:** `wasm-opt -Oz` via `[package.metadata.wasm-pack.profile.release]` (the
      `wasm-release` cargo profile isn't reachable through `wasm-pack --release`).

### Done — Phase-5 hardening pass (branch `feat/phase5-hardening`)
The "do 1–8" follow-up batch. Each is documented in full under its own Phase section above; in brief:
- [x] **§3 IDE completion/naming:** scope-aware completion (indentation scan), `%Unique` completion
      (token-context modulo disambiguation), soft-keyword-named symbols (`match`/`when`).
- [x] **§6 engine version:** parse `project.godot` `config/features` → `engine_version()` salsa query
      + `project_engine_version()` plumbing (informational until Phase-6 multi-version bundling).
- [x] **§4b scene tail:** `$Enemy/Sprite` paths INTO an instanced sub-scene now type the inner node.
- [x] **§8 find-refs:** `ReferenceKind::Write` derivation + a 2nd classify/infer agreement guard.
- [x] **§5 LSP debounce:** `didChange` diagnostics are coalesced (150 ms quiescence) via a `select!`
      timer arm — text still commits immediately; a burst of keystrokes recomputes once.
- [x] **§2 parser hardening:** **307 → 0** `GDSCRIPT_SYNTAX` errors on godot-demo-projects.

### Deferred (Phase-5)
- [ ] **LSP read dispatch is thread-per-request** (`std::thread::spawn` per read), not a bounded pool.
      Fine for an editor (request rate is editor-bounded); under adversarial load it could spawn many
      threads. A bounded worker pool + a Worker/LatencySensitive split is the follow-up. **Low
      criticality, no correctness gain** (salsa cancellation already makes thread-per-request correct);
      deferred deliberately in the hardening pass rather than risk the snapshot-lifetime subtleties.
- [ ] **napi cross-platform matrix is 6/10 triples** (mac x2, win x64 + arm64, linux gnu x2). DEFERRED:
      this is release-pipeline CI that **cannot be verified on the local Windows-gnu box** (no Linux
      cross-toolchain, no zig/cross/WASI-SDK), and pushing unverified cross-compile + publish YAML into
      the release pipeline is riskier than the documented gap. **Ready-to-execute spec:** add to the
      `release-napi.yml` matrix + `bindings/node/package.json` `napi.targets` (in lockstep), each as a
      separate leg marked `experimental` with `continue-on-error: ${{ matrix.settings.experimental }}`
      so a not-yet-green leg can't fail a release: `x86_64-unknown-linux-musl` + `aarch64-unknown-linux-musl`
      (`npm run build -- --target <triple> --use-napi-cross`; napi-cross bundles zig+musl),
      `armv7-unknown-linux-gnueabihf` (`--use-napi-cross`). Then trigger `release-napi` via
      `workflow_dispatch` (build-only) to confirm each leg builds before removing `experimental`. The
      `wasm32-wasip1-threads` WASI fallback stays separately deferred (emnapi runtime + WASI-SDK link
      step, genuinely unproven). Until these land, `npm i` on Alpine / an unlisted platform won't
      resolve a binary.
- [ ] **Distribution polish (DEFERRED — unverifiable locally):** a `twiggy` wasm size-regression CI
      guard (needs `wasm-pack` on CI, not installed locally); empirically validate the CLI's SARIF
      output against GitHub code-scanning's ingester (needs an actual code-scanning upload). *(The
      engine model is ALREADY brotli-handled on the wasm path — `AnalysisHost::set_engine_api` decodes
      a `fetch`ed brotli blob — so the "content-hashed `.rkyv.br`" item is partly done; the remaining
      piece is the native/bundled side, low value.)*
- [ ] **§7 guitkx (ReactiveUI-Godot) client integration — DEFERRED to a separate repo PR (Phase-5).**
      It lives in a DIFFERENT repository (`ReactiveUI-Godot`, not the analyzer), so it is naturally its
      own PR. Two items: (a) **cross-file *library* go-to-definition** (e.g. `use_ref` → `core/hooks.gd`)
      — a regression vs the old Godot proxy; the embedded-analyzer adapter loads only the single virtual
      `.gd` doc, not the referenced library files. Needs a runtime-model pass (where `use_ref`/`V`/`Hooks`
      resolve from) before loading the libraries into the handle. Same-file goto works and the seam
      prevents false positives, so the current state is safe. (b) **`analyzerProxy.ts` end-to-end
      validation** — needs the napi `.node` build (`libnode.dll`, CI-only), so it is CI-gated. Best done
      AFTER the analyzer's `@gdscript-analyzer/core` `^0.2.0` is published and the guitkx dep is bumped.

---

## Phase 6 — v1.0 (in progress, branch `feat/phase6`)

Driven by `plans/PHASE-6-EXECUTION-OVERVIEW.md` + the seven workstream playbooks. Done so far:
W1 M0 (the `WarningCode` emit-then-gate seam), W2 M0–M2 (the CFG narrowing dataflow + checker
wiring + short-circuit), W1 M1 (a self-contained-check subset). W6 (the `#[non_exhaustive]` freeze
+ 1.0 tag) is held for last, by design.

### §1 hardening pass (bug-hunt + a tech-debt batch) — DONE

A pre-1.0 **adversarial bug-hunt** (8 lenses × 3-vote verify) over the W1 gate / W2 flow / W3
formatter, plus an **empirical** sweep over 545 real `.gd` files (ReactiveUI-Gadot + godot-demo-
projects: **0 panics**). The W2-flow-soundness and panic-safety lenses found **nothing** (the
narrowing dataflow + panic-safety are clean). **5 confirmed defects, all fixed + regression-tested**:
- [x] **`INT_AS_ENUM_WITHOUT_CAST` false positive** — a class enum *member* typed as bare `int`
      while its annotation typed as `Ty::Enum`, so `var m: C.E = C.MEMBER` false-warned.
      `class_enum_value` now returns the declaring enum type; `is_assignable` routes a *different*
      enum to `IntAsEnum` (not a hard `TYPE_MISMATCH`). **Demo-projects: 124 → 1** (the 1 is a real
      bare-int case); `TYPE_MISMATCH` unchanged.
- [x] **Formatter indentation corruption** after a comment-only line (mistaken for a bracket
      continuation → invalid GDScript the token-equality net can't see). `reindent` now tracks
      bracket depth; the safety net gained a **parse-validity recheck** (input clean ⇒ output clean).
      *Known cosmetic limitation:* a comment that is the **first line of a block** lands at column 0
      (the prepass emits `Indent` only at the first code line) — valid, but not re-indented.
- [x] **`exclude_addons` over-match** (`contains("/addons/")`) → only the root `res://addons/` now.
- [x] **`@warning_ignore_start` stacked per code** (leaked to EOF) → overwrites per code like Godot.
- [x] **`@warning_ignore` one-shot** now covers the whole physical line (`;`-joined statements).

Tech-debt items completed in the same pass:
- [x] **W1 `SHADOWED_VARIABLE`** — a local `var`/`const` shadowing a param or own value-member
      (var/const/signal/anon-enum). Sound (function-body-only, value-members-only); verified on the
      corpus (2 real shadows / 89 files, 0 false positives). `SHADOWED_VARIABLE_BASE_CLASS` (needs
      the base item-tree) stays deferred below.
- [x] **W5 `examples/analyze.rs`** — a documented public-API tour (diagnostics / hover / symbols /
      format), CI-verified via clippy `--all-targets`.

### W1 — warning set: checks deferred from the M1 cut (machinery is DONE; these are additive)

The `WarningCode` catalog + `gate()` + per-code settings already support **all 48** codes; the
following just need their detection wired (each is purely additive — a new `Cx::warn(...)` site or a
file-level scan, gated by the existing seam). Landed in M1: `EMPTY_FILE`, `UNUSED_VARIABLE`,
`UNUSED_LOCAL_CONSTANT`, `UNUSED_PARAMETER`, `STANDALONE_EXPRESSION`, `STANDALONE_TERNARY`,
`INT_AS_ENUM_WITHOUT_CAST`, `INCOMPATIBLE_TERNARY`, `UNREACHABLE_CODE` (consumes W2 reachability).
Landed in the §1 pass: `SHADOWED_VARIABLE` (param/value-member shadow).

**Deferral policy (pre-freeze):** each remaining check is purely additive (a new `Cx::warn` site on
the existing seam), so it ships as a clean 0.x **PATCH** *after* the freeze with no API churn. The
ones below were deliberately **not** rushed into the 1.0 cut because they carry a false-positive risk
(uncertain Godot-parity semantics) that the §1 pass exists to eliminate — better landed individually,
each with its own bug-hunt, than batched in under freeze pressure. Sequenced by value.

- [x] **`UNTYPED_DECLARATION` / `INFERRED_DECLARATION` — DONE (burndown Stage 1).** Emitted from the
      binding flags: a `var`/parameter with no `: T` and no `:=` → `UNTYPED_DECLARATION`; a `var :=` →
      `INFERRED_DECLARATION` (locals + params; `const`/`for`/pattern-bind excluded — value-fixed). To
      keep them from polluting every fixture, the infer `codes()`/`file_codes()` helpers filter them,
      and — because they fire on essentially every untyped/inferred local — they are **excluded from
      the `--strict`/standalone auto-promotion** (`WarningCode::promoted_by_strict`): they require an
      explicit per-code `project.godot` setting, matching Godot's "most users never enable them".
- [x] **`SHADOWED_VARIABLE` — DONE (§1 pass).** A local `var`/`const` shadowing a param or own
      value-member (var/const/signal/anon-enum). **`SHADOWED_VARIABLE_BASE_CLASS` — DONE engine-base
      (Phase 1):** a local shadowing a value member of the resolved ENGINE base
      (`engine_base_has_value_member`), silent on an unresolved base. The **user-base** slice (a
      member shadowing a base declared by another script) stays deferred — the cross-file `MemberSig`
      is lossy (no kind/params), so a sound user-base walk needs it enriched first.
- [x] **`SHADOWED_GLOBAL_IDENTIFIER` (extend) — DONE (burndown Stage 1).** Now also fires (gated, as
      a real `WarningCode::ShadowedGlobalIdentifier`) for a parameter / local `var`/`const` / `for` /
      pattern-bind / member `var`/`const`/`signal` whose name collides with a project/engine global
      (built-in type/function, native class, engine singleton, project `class_name`, `*`-autoload),
      mirroring `gdscript_analyzer.cpp`'s `is_shadowing`. A global shadow takes precedence over a
      variable/base-class shadow (no double-warn). Conservative: bare pseudo-constants (`PI`) / global
      enums excluded → only under-warns vs Godot, never a false positive. The pre-existing `class_name`
      collision stays its own ungated file-level diagnostic (closer to Godot's hides-global error).
- [x] **`ASSERT_ALWAYS_TRUE` / `ASSERT_ALWAYS_FALSE` — DONE (burndown Stage 1).** `Literal::Bool`
      now carries its value, so `Stmt::Assert` can fire `ASSERT_ALWAYS_TRUE` / `ASSERT_ALWAYS_FALSE`
      when the condition is a constant literal whose booleanization is known (a bool literal, or
      `null` = false), mirroring Godot's `resolve_assert`. Named-constant / arithmetic folding is
      deliberately not attempted (sound under-warn — no false positive on a runtime condition).
- [x] **`CONFUSABLE_IDENTIFIER` → DONE (burndown Stage 2).** UTS #39 restriction-level detection via
      the `unicode-security` crate: a non-ASCII identifier that mixes scripts in a spoofable way (≥
      `MinimallyRestrictive`, e.g. a Latin name carrying a Cyrillic/Greek homoglyph) warns; pure-ASCII
      and legitimate single-script / CJK+Latin names never do. Checked on members, locals/params, and
      `class_name`. **Prerequisite also fixed:** the lexer was ASCII-only (`[A-Za-z_]…`) — a
      parse-correctness gap on valid Godot code — now accepts UAX #31 / XID identifiers (`\p{XID_Start}
      \p{XID_Continue}*`), a strict superset on ASCII (tokenization byte-identical there).
- [ ] **The 4 flow/scope confusables** (`CONFUSABLE_LOCAL_DECLARATION` / `_LOCAL_USAGE` /
      `_CAPTURE_REASSIGNMENT` / `_TEMPORARY_MODIFICATION` — the last master-only) — these are **not**
      the Unicode-table check above; they need use-before-declaration shadowing analysis + lambda-
      capture tracking (a distinct flow-analysis effort, low value / niche).
- [x] **Deprecated-misuse trio — `PROPERTY_USED_AS_FUNCTION` / `CONSTANT_USED_AS_FUNCTION` DONE
      (Phase 1, `feat/w1-warnings`).** Calling a statically-resolved engine property/const as a
      function, guarded against Callable/Signal/uninformative members. **`FUNCTION_USED_AS_PROPERTY`
      stays deferred** — a bare `obj.method` is an idiomatic `Callable` reference (every signal
      `.connect`), indistinguishable from a misuse without call-context, so it would false-positive
      everywhere. Needs the value-vs-call-context distinction the current `as_method` flag can't make.
- [x] **`NATIVE_METHOD_OVERRIDE` — DONE engine-base (Phase 1), conservative.** Warns (ERROR-default)
      only on a *definite type clash* at an overlapping typed param (both resolve to known engine
      types, mutually non-assignable, neither an enum). Arity/defaults/vararg/variance + the
      **user-base** override (needs the lossy cross-file `MemberSig` enriched with is_virtual/params)
      under-warn — deferred.
- [x] **`STATIC_CALLED_ON_INSTANCE` + `ENUM_VARIABLE_WITHOUT_DEFAULT` — DONE (Phase 1).** Static-on-
      instance fires only for a typed local instance (skips a type-aliased local). Enum-without-
      default fires for a local OR member field.
- [x] **Annotation-dependent lifecycle checks → DONE (burndown Stage 2, on first-class annotations).**
      **`ONREADY_WITH_EXPORT`** (`@onready` + `@export` on one member), **`REDUNDANT_STATIC_UNLOAD`**
      (`@static_unload` with no `static var`), and **`MISSING_TOOL`** (a non-`@tool` class extending a
      `@tool` user-script base — firewall-safe via the base's `item_tree`). Corpus: 0 false positives.
- [ ] **Non-annotation W1 checks still deferred:** **`REDUNDANT_AWAIT`**, **`UNSAFE_VOID_RETURN`**,
      **`UNSAFE_CAST`**, **`RETURN_VALUE_DISCARDED`**, **`INT_AS_ENUM_WITHOUT_MATCH`**,
      **`DEPRECATED_KEYWORD`** (`yield` — the parser must surface it), **`FUNCTION_USED_AS_PROPERTY`**
      (needs value-vs-call context). Each is an independent additive check, not annotation-blocked.
- [x] **`UNUSED_SIGNAL` — DONE (Phase 1, same-file).** A signal never referenced anywhere in its own
      file (a whole-file `NameUses` identifier + string-literal scan). Same-file only, like Godot — a
      signal connected purely from a scene/other file is invisible (the Godot-parity limitation).
      **`UNUSED_PRIVATE_CLASS_VARIABLE`** still deferred (the same `NameUses` scan now exists to build
      it on).
- [x] **`UNASSIGNED_VARIABLE` — DONE (Phase 2, `feat/w1-warnings`).** A read of a typed-no-init local
      not definitely assigned on every path, via a new `flow::analyze_assigned` definite-assignment
      lattice (grow-only, intersect-at-merge, params seeded, lambda bodies unchecked) consulted at each
      read in `resolve_name` (excluding the assignment LHS). Matches Godot's may-unassigned; verified 0
      false positives on 545 real `.gd` (the 20 demo hits are genuine read-before-assign). The cosmetic
      **`_OP_ASSIGN`** variant stays deferred — compound-assign collapses to `BinOp::Assign` in lowering,
      so it can't be distinguished without the un-collapsed CST op.
- [x] **`UNUSED_*` precision → DONE (burndown Stage 2).** `used_locals` now records a *read* only —
      the bare LHS of an assignment (`x = …`) is excluded (a compound `x += …` still reads via its RHS;
      a receiver / index target reads the base), so an assigned-but-never-read local is correctly
      `UNUSED_VARIABLE`. Also added **`UNUSED_PRIVATE_CLASS_VARIABLE`** (a `_`-prefixed, non-`@export`
      member var never referenced in the file — same-file scan like `UNUSED_SIGNAL`; exported vars are
      excluded to stay no-false-positive). Corpus (2d/3d/gui/audio): 0 false positives.

### W2 — narrowing: deferred precision (post-1.0 quality, MINOR/PATCH not API breaks)

- [ ] **Assignment re-narrowing** (`x = other` → x: typeof(other)). M1 made flow authoritative, and
      flow runs pre-inference so it can only *invalidate* on assignment (the sound 1.0 floor), not
      re-narrow to the assigned value's type. Re-narrowing needs the value's inferred type fed back
      into the facts — a post-1.0 precision item.
- [x] **`UNREACHABLE_PATTERN` — DONE (Phase 2).** `body.rs` `MatchArm` now carries a `range` +
      `is_catch_all` (`arm_is_unconditional_catch_all`: sole top-level `_`/`var x`, no `when` guard);
      `flow.rs` records every arm after a catch-all; `infer.rs` emits it. Conservative (a multi-pattern
      `1, _:`, a nested `_`, and a guarded arm are NOT catch-alls — under-warn, 0 false positives on
      545 files). **`match`-arm scrutinee narrowing — N/A (not deferred, removed from scope):** GDScript
      `match` has **no type patterns** (patterns are literals/constants, `_`, `var x`, array, dict), so
      `match x: Node2D:` matches `x == Node2D` (a value compare), not `x is Node2D` — there is no
      scrutinee type to narrow, and doing so would be an *incorrect* (false) narrowing.
- [ ] **`NotNull` / `Not(T)` consumption** — recorded by the flow pass but not used for typing in
      1.0 (no null-access diagnostic to drive `NotNull`; `Not(T)` has no positive type). Wire when a
      null-safety check lands.
- [ ] **Loop-carried back-edge fixpoints, aliasing, narrowing through call results
      (`if get_thing() is T:`)** — explicitly out of the 1.0 cut (per the W2 playbook §1 tail).

### W3/W4/W5 — Phase-6 deferrals (infra needing CI services or measurement-first decisions)

- [ ] **W3 — formatter reflow / gdformat parity.** Landed: the `gdscript-fmt` crate — a
      safe-by-construction whitespace + **block-indentation** normalizer (re-emits the pre-pass
      token stream, every significant token verbatim, with a re-parse significant-token-equality
      fallback **plus** a parse-validity recheck — both added/hardened in the §1 pass), wired into
      `Analysis::format` + the CLI (`format --check/--write`/stdout) + LSP `textDocument/formatting`.
      - [x] **Increment A — intra-line spacing — DONE (Phase 4, `feat/formatter-scene-rename`).** One
        space around binary operators / assignments / `->` / `:=` / keyword operators, after `,` and a
        type-annotation/dict `:`, hugged brackets (`f(x, y)`, `[1, 2]`), tight member access (`a.b`),
        tight unary (`-x`), tight call/lambda parens (`f(`, `func(`, `preload(`, `assert(`). Purely
        local (prev significant token + innermost bracket); the genuinely ambiguous contexts are left
        verbatim/tight: a **slice colon** `arr[a:b]` (and GDScript has no slice syntax anyway, so it
        never appears in valid code), and **node-path sigils** `$Node/Path` / `%Unique` via a small
        state machine that keeps them *verbatim* — never collapsing a spaced `$A / b` (a division)
        into a path, the one meaning-change the token-equality net cannot catch. Gated behind
        `FmtConfig::normalize_spacing` (default on). **Verified** over the **godot-demo-projects +
        ReactiveUI-Gadot corpus (502 clean-parsing files), safe_mode OFF: 0 token-sequence changes,
        0 idempotence breaks.** The lambda-`func()` paren bug was found there and fixed.
      - [x] **Lambda-in-brackets indentation — FIXED (Phase 4C, `f909a08`).** A multi-line lambda
        body passed as a call argument is a block *inside* brackets; the prepass re-emits synthetic
        layout for it, but the indenter handled the header (a `NewlinePhys` continuation) and the body
        (a synthetic `Newline`) inconsistently → non-parsing output on 2 corpus files. Fixed by treating
        a synthetic `Newline` inside brackets as a continuation (keep the interior verbatim). Corpus
        (544 files, safe_mode OFF): non-parsing outputs 2 → **0**.
      - [x] **Increment B — blank-line policy — DONE (Phase 4, same branch).** Collapses runs of
        blank lines (max 2 at top level, max 1 inside a block — capped against the *next* line's depth,
        since the `Dedent` lands after the blanks) and strips leading blank lines. Done at the **token**
        level (buffered blank-line counter flushed on the next content line), so a `\n` inside a
        `"""..."""` multi-line string is never mistaken for a blank line. Gated behind
        `FmtConfig::collapse_blank_lines` (default on).
      - [x] **Increment C (whitespace half) — DONE (Phase 4C, `2bced9e` + `5ab7238`).** Validated
        against a **live `gdformat` oracle** (`uvx --from gdtoolkit gdformat`) over the corpus.
        (a) **Blank-line insertion** — 2 blanks around top-level defs, 1 around nested ones; attached
        comment/annotation prefixes move with the def; gated behind `FmtConfig::insert_blank_lines`.
        (b) **Block-boundary comment indentation** — a comment is re-indented to its intended depth
        (authored indentation clamped to the surrounding structure, compared by indentation length so
        it is indent-width agnostic), fixing the old "comment at column 0" limitation. (c) **EOL
        preservation** — LF stays LF, CRLF stays CRLF (a deliberate deviation from gdformat's
        platform-normalisation; see `crates/gdscript-fmt/DEVIATIONS.md`). **gdformat differential**
        (godot-demo-projects, EOL-normalised): byte-exact 14% → **45%**; `format(gold)==gold` for
        **426/455** token-compatible files (was 70). Corpus safety (544 files, safe_mode OFF): 0
        non-parsing, 0 token changes, 0 idempotence breaks.
      - [x] **Length-driven line reflow — DONE (Phase 4C, same branch).** A single-line statement that
        exceeds `line_width` and contains a bracketed group is wrapped flat → compact → exploded via a
        small `Doc`-IR (`FmtConfig::reflow`, default on). **Token-preserving** (no trailing comma added);
        byte-identical to gdformat on the corpus for the cases it handles. Only single-physical-line
        statements are reflowed (already-wrapped statements preserved → idempotent). Differential after
        reflow: byte-exact **51%** (godot) / **33%** (ReactiveUI), EOL-normalised; corpus safety (544
        files, safe_mode OFF): 0 non-parsing, 0 token changes, 0 idempotence breaks.
      - [x] **CST-driven wrapping port + token-mutating layout + comment-threading — DONE
        (`feat/formatter-scene-rename`).** A faithful port of gdformat 4.5's `expression.py`
        (`src/wrap.rs`) plus the token-mutating set (magic trailing comma, operator-chain paren
        injection, string-quote normalisation, dot-chain leading-dot, lambdas, multi-line strings) and
        **comment-threading through any reshape** (block / operator-chain / call-arg, incl. a standalone
        comment that hangs off a lambda *body* at a deeper indent, a block-start blank strip, a
        block-boundary comment placed at its block depth, a trailing comment that never forces a wrap,
        and a column-0 comment amid a function body). Each is guarded by the meaning-equivalence net +
        a dedicated **comment-multiset net** (any unplaceable comment → verbatim fallback). **gdformat
        differential (EOL-normalised): byte-exact godot 454/456 (99.6%), ReactiveUI-Gadot 88/88 (100%);
        `format(gold)==gold` 455/456 / 88/88; 110 unit tests. Corpus safety (544 files, safe_mode OFF):
        0 non-parsing, 0 token changes, 0 idempotence breaks.** Every behaviour + remaining gap is
        catalogued in **`crates/gdscript-fmt/DEVIATIONS.md`**. **`format_range`** (LSP range formatting)
        is also DONE (`fmt::format_range` → `ide` → LSP `textDocument/rangeFormatting`).
      - [ ] **The last 2 byte-exact godot misses — deep wrap-choice nuances (low value, regression-risky;
        single-file, valid, ≤-width, meaning-preserving — tracked, not blocking).**
        - **(a) `town_scene.gd` — redundant sub-expression parens in an operator chain.** gdformat
          **keeps** the source parens in `(A) or (B)` (where each operand is an `and`-chain) and threads
          the chain's standalone comments around them; our `strip_parens` removes the (precedence-
          redundant) parens and re-wraps, so the comment placement + paren shape diverge. A fix must make
          paren-stripping **operand-aware** — keep a redundant paren that wraps a tighter-precedence
          operand of a looser chain — which is a behaviour change with corpus-wide regression risk, so
          deferred until measured against the full corpus.
        - **(b) `os_test.gd` — subscript on a large array literal.**
          `["…", …, "…"][DisplayServer.screen_get_orientation()]`: gdformat explodes the array
          one-element-per-line and keeps the `[index]` on the close line; we keep the array compact and
          drop the subscript onto its own line below. A `format_index` layout nuance for an indexed
          *literal* (an indexed *call* chain is already handled via `format_dot_chain`).
- [ ] **W4 — perf infra tail.** Landed: a warm-keystroke incremental bench (`crates/gdscript-ide/benches/analysis.rs`, ~2ms for ~300 loc — confirms the W1 gate-downstream + W2 flow-inside-`analyze_file` keep incrementality flat). Deferred: a tiered `fixtures/perf/{small,medium,large}` vendored corpus + project-scale cold bench; a **CI bench-regression gate** (CodSpeed / Bencher — needs the CI service + a baseline); `dhat` memory profiling + a documented resident ceiling; a salsa-LRU for cold-file derived data (measure first — only if `flow`/`infer` recompute shows hot); the `wasm-opt -Oz` + twiggy wasm-size CI guard (overlaps §1, needs `wasm-pack` on CI).
- [ ] **W5 — docs tail.** Landed: the generated Warning Reference (anti-drift test in `cargo test`) + the Configuration page + **`crates/gdscript-ide/examples/analyze.rs`** (a CI-built public-API tour — added in the §1 pass). Deferred: the W6 **contract page** (authored *with* the freeze — it embeds the verbatim semver policy + the Godot-version matrix, so it is W6's job by definition); the docs.rs polish pass (`deny(missing_docs)` on the public crates, doctest the POD docs, "internal — not stable" banners on the non-contract crates — **W6-entangled**, since which crates are "contract" vs "internal" is the freeze decision); playground-as-live-docs deep links.
- [x] **CLI `--strict` / `--engine-defaults` override — DONE (Phase 1, `feat/w1-warnings`).** A plain
      (non-salsa) `WarningOverride` field on the `Db` (read only by the downstream `type_diagnostics`,
      so the W1 firewall holds), `WarningSettings::with_strict_opt_in` flipping only the opt-in
      promotion (an explicit project per-code level still wins), `AnalysisHost::set_warning_override`,
      and mutually-exclusive `--strict` / `--engine-defaults` CLI flags.
