# Tech debt & follow-ups

The running backlog of deferred work, known limitations, and queued next steps. Keep it
honest — anything we knowingly defer or stub goes here with enough context to pick it up
later.

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
- [ ] **Type resolution is M1.** M0 only records `type=`/`script=`/`instance=`; mapping to a `Ty`
      (native class / `class_name` registry / attached-script refine) is `gdscript-hir` M1.
- [ ] **Instanced sub-scene recursion → M1+ (hard tail).** An instanced node records `instance`
      (an `ExtId`); following it into the sub-scene's root type needs the cross-file VFS/project graph.
- [ ] **Project-wide `script→scene` reverse index + salsa caching → M1.** M0's `node_with_script`
      answers the *per-scene* half only; the cross-project map and the `scene_model(db, FileId)`
      tracked query live in `gdscript-db`/`gdscript-hir`.
- [ ] **`uid://` resolution → DEFERRED (Phase-5, user-approved rationale).** M0 records `uid`;
      resolving a uid-*only* `ext_resource` would need a project UID map. **Near-zero real value:** in
      Godot 4.x every `ext_resource` is written with BOTH `path=` and `uid=`, so path-first resolution
      (already implemented) handles every real case — a uid-only resource essentially never occurs. A
      firewall-safe impl needs a new `uid` field on the `FileText` salsa input (a `uid` derived from
      `scene_model` would couple the registry to body-text edits and break the cross-file firewall)
      plus loader plumbing in BOTH the LSP and CLI. Deferred: bad cost/value ratio. M0 prefers `path=`.
- [ ] **Inline `script = SubResource("…")` records no attachment.** An inline GDScript sub-resource
      has no external path; M0 sets `script = None` (M1 types the node by its declared `type=`). Rare.
- [ ] **`name_span` includes the surrounding quotes** (the `name="…"` value span). Fine for coarse
      go-to-def; trim if a precise highlight is needed.
- [ ] **A literal `/` inside a node name** would break `/`-segmented path matching (Godot disallows
      it at edit time; a hand-edited file could violate it). Treated as opaque segments.
- [ ] **No in-repo full corpus.** 5 representative real fixtures are vendored under
      `crates/gdscript-scene/tests/corpus/`; the broad robustness run is ad hoc via
      `cargo run -p gdscript-scene --example scene_corpus -- <dir>` (not in CI).

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
- [ ] **`unescape` drops `\uXXXX`/`\UXXXXXX`/`\b`/`\f`** → a name with such an escape mis-decodes
      (e.g. `A` → `u0041`). Cosmetic *and consistent* (applied to both `name=` and `parent=`, so
      path matching still works); display/go-to-def only. Rare. Extend `unescape` if it surfaces.
- [ ] **Cascading dangling:** a node parented to a sibling whose own parent dangled is itself
      flagged. Secondary effect; rare. Track an "upstream-dangling" set to suppress the secondary.
- [ ] **A node literally named `"."`** makes `by_path["."]` that `resolve_path` can't return —
      engine-impossible input; **wontfix**.

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
- [ ] **Scene-aware rename → Phase 6.** Renaming a node in a `.tscn` and updating `$Path`s (or vice
      versa) is deferred per the plan; M2 ships the read-side features (type/goto/complete/diagnose).

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
- [ ] **Trivia attachment is the simple model.** The tree sink flushes leading trivia
      into the *following* node; it does not implement rust-analyzer's full
      `n_attached_trivia` leading-vs-trailing heuristic (blank-line breaks, doc-comment
      pull). Lossless, but attachment isn't ideal for formatting fidelity — refine before
      shipping a formatter.
- [ ] **Annotations are sibling nodes,** not children of the declaration they decorate.
      `document_symbols` is unaffected; an AST `FuncDecl::annotations()` accessor would
      need a preceding-sibling walk.
- [ ] **Property accessor (`get`/`set`) parsing is permissive.** Inline and indented
      forms are accepted loosely; tighten when accessor semantics matter (Phase 2).
- [x] **Soft-keyword identifiers — `match`/`when` supported.** Godot's `is_identifier()`
      whitelist is `match, when, PI, TAU, INF, NAN`; the parser now accepts `match`/`when`
      as names (declaration / parameter / identifier expression) and the full
      `is_node_name()` keyword set after `.` (verified against `gdscript_tokenizer.cpp`).
      The four math constants stay literal tokens (their near-universal use), so
      `var PI = …`-style shadowing of a constant isn't modeled — a deliberate choice, not
      a gap.
- [ ] **Statement-initial bare `match` is always the `match` statement.** `match(...)` /
      `match.x` used as an *identifier* at statement start isn't handled (needs lookahead
      in `stmt`). Rare; not seen in real corpora (member/`func` uses are handled).
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
- [ ] **guitkx napi `analyzerProxy.ts` validation (Playbook §5.1, conditional).** The
      end-to-end check that the napi build answers guitkx's embedded-GDScript
      completion/hover with no Godot editor running. Needs the napi `.node` build
      (`libnode.dll`, CI-only — see Phase 1) + the guitkx LSP server at
      `…/ReactiveUI-Gadot/ide-extensions/lsp-server`. Everything it depends on (the
      analyzer answering completion/hover headless) is built and proven against the corpus;
      this is the integration wiring, deferred to the Phase-5 client work.
- [ ] **Cross-file resolution → Phase 3.** `class_name` globals, autoloads, `preload`,
      script `extends`, and `as`/`is` against user types all funnel through
      `resolve_external() -> Ty::Unknown` (the seam). Correct + non-cascading today; Phase 3
      reimplements only that one function.
- [ ] **Scene-aware node typing → Phase 4.** `$Node` / `%Unique` / `get_node()` are always
      `Object(Node)` (never the concrete child); `.tscn` parsing narrows them in Phase 4.
- [ ] **Full 48-warning set + project-settings gating + real CFG narrowing → Phase 6.**
      Phase 2 ships the MVP subset (INFERENCE_ON_VARIANT, TYPE_MISMATCH, NARROWING_CONVERSION,
      INTEGER_DIVISION, UNSAFE_PROPERTY/METHOD_ACCESS); `is`-narrowing is lexical/syntactic,
      not a real control-flow graph; `@warning_ignore` gating is not applied.
- [ ] **Hover docs are signatures-only.** The `DocId`-keyed doc store is wired into the
      model but not populated (the BBCode→Markdown doc-XML pipeline is deferred, Playbook
      §4.6), so `HoverResult.doc` is empty and hover shows the inferred type / signature only.

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
- [ ] **Bindings return JSON strings,** not typed `#[napi(object)]` / `serde-wasm-bindgen`
      objects. Works and is minimal; consider typed results for better TS/JS DX once the
      result shapes stabilize.

### Validation
- [ ] **Differential corpus is small + error-agreement only.** The tree-sitter oracle
      checks whether both parsers consider a file well-formed over ~14 core snippets. Grow
      it (e.g. the Godot demo-projects corpus) and add a structural skeleton comparison.
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
- [ ] **(M5) Scene/config rewriting deferred → rename refuses.** `.tscn`/`.tres` are not ingested
      (scene crate is a Phase-4 stub) and `project.godot` is read-only to rename. Method/signal/
      exported-var renames **refuse** on a detected same-named project string literal; autoload-name
      renames refuse. **Known probabilistic gap:** a scene `[connection method="…"]` we cannot see
      makes a method rename *appear* safe — we mitigate by refusing on any same-named `.gd` string,
      but a pure-scene reference is invisible. Scene-aware rename = Phase 4/6.
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
- [ ] **Non-`*` autoloads are not resolvable by name (nor via `get_node("/root/Name")`).** We seed
      globals only for `*`-singletons (matches the engine: no `*` ⇒ not a global constant). The
      `/root/Name` node-path access is Phase-4 scene/node work. No false positives, just imprecision.
- [ ] **`is`-narrowing is a deliberate divergence from upstream Godot.** Godot's `reduce_type_test` does
      **no** flow narrowing (CONFIRMED against `gdscript_analyzer.cpp`); our `is`-narrowing is a Pyright-style
      UX value-add, kept **widen-only** (never narrows to a type Godot's checker would reject). Intentional
      non-parity.
- [ ] **`project.godot` parsing is `[autoload]`-only.** `config/features` (the human engine version) is
      not yet parsed/consumed; API-version selection from it is Phase-5 (`ApiInput`) work.
- [ ] **No per-`project.godot` corpus mode yet.** M4 was validated on a single autoload subproject
      (faithful: one `project.godot`, one namespace). A `--multi-project` harness mode (discover every
      `project.godot`, one host per sub-project) is the exhaustive demo-projects gate — deferred; the
      merged `--project` mode remains the panic/robustness stress test. (Supersedes the M2 stress-test note.)
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
- [ ] **`corpus --project` is a robustness stress test, not a single-project run.** Merging many
      sub-projects into one host shares the `class_name` namespace; cross-project collisions are
      expected. A faithful per-project validation needs `project.godot`-scoped roots (M4).

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
