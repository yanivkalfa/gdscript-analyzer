# Tech debt & follow-ups

The running backlog of deferred work, known limitations, and queued next steps. Keep it
honest — anything we knowingly defer or stub goes here with enough context to pick it up
later.

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
- [ ] **Completions are not scope-aware.** By-name completion offers *every* document
      symbol, not just names visible in the enclosing scope. Acceptable for Tier 0;
      scope-awareness comes with the HIR.
- [x] **Type inference / member completion / hover / inlay / signature help / code
      actions — DONE in Phase 2.** Goto-def / find-refs / rename remain Phase 3 (need the
      project graph); the `goto_definition` method still returns empty.
- [ ] **No salsa / incremental reparse.** Whole-file reparse + re-infer on every query (a
      plain VFS map). Adopt salsa at Phase 3 — every derived computation is already a pure
      `(text) -> value` function so the swap is localized. (Warm single-file is ~1.4 ms, so
      this is a scaling concern for large projects, not a single-file latency one.)

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
- [ ] **Member field types are seeded by a shallow first pass.** `analyze_file` infers field
      initializers once (empty `member_types`) to seed the function pass, so a field whose
      initializer references *another* field (`var b := a + 1`) sees `a` as `Variant`/seam
      rather than its real type. No fixpoint iteration — rare; revisit if it surfaces.
- [ ] **`await` and inner-class member types resolve to the seam (`Unknown`).** Conservative
      (never a false positive), but imprecise: `await sig` doesn't recover the signal's arg
      type, and `inner_instance.field` isn't typed. Refine with the project graph (P3).

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
      re-validated. A `classify`↔`infer` agreement test on the corpus would guard the duplication.
- [ ] **(M5) `Member`/`Global` find-refs scope is project-wide-candidates, not a precise referrer
      graph.** Correct (the re-resolve confirms) but does wasted `classify`s on files that name-but-
      don't-reference the symbol. A firewall-safe referrer reverse-index (keyed on `item_tree`, not
      bodies) is a perf follow-up if the large-project benchmark regresses.
- [ ] **(M5) `ReferenceKind::Write` not derived.** find-refs tags `Declaration` vs `Read` only;
      assignment-LHS `Write` is a cheap follow-up off the lowered body.
- [ ] **Scene (`.tscn`) autoloads resolve to the seam, not their scene-root type → Phase 4.** A
      `*`-autoload pointing at a `.tscn` is `Unknown` (member access is unchecked, never a false warn).
      Typing it as bare `Node` would *false-warn* on the root script's own members (`Music.play()`);
      the real fix is Phase-4 scene parsing (read the root node's `class_name` script). `.cs` autoloads
      likewise → seam (out of scope for a GDScript analyzer).
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
- [ ] **Relative `preload`/`extends` paths (`preload("sibling.gd")`) resolve to the seam.** Godot anchors
      them to the importing script's dir: `resolved = script_path.get_base_dir().path_join(p).simplify_path()`
      (CONFIRMED `reduce_preload` 4664-4667). Absolute `res://`/`user://` are handled; relative needs the
      importing file's path threaded into resolution (better done deliberately with **M5**'s file-context
      work). 0 occurrences in the reference corpus; conservative seam = no false positives.
- [ ] **Cross-*file* `preload`-const member access is the seam.** `const X = preload(…)` then `X.new()` is
      typed in the **declaring** file (the member pre-pass infers the initializer). Reading that const from
      *another* file (`other.X`) sees `script_class`'s annotation-only sig (`Variant`), because `script_class`
      is offset-free and does not infer const *initializers*. Rare; the corpus pattern is same-file.
- [ ] **Parser gaps on the broader demo-projects corpus (NEW, Phase-1 follow-up).** Project-mode
      over godot-demo-projects surfaced **307 `GDSCRIPT_SYNTAX`** errors (cascading
      "expected a declaration" — a few unhandled syntactic forms, e.g. some lambda/match/typed
      constructs the ReactiveUI-Godot corpus didn't exercise). 0 panics. Harden the parser +
      grow the differential oracle against godot-demo-projects before v1.
- [ ] **`corpus --project` is a robustness stress test, not a single-project run.** Merging many
      sub-projects into one host shares the `class_name` namespace; cross-project collisions are
      expected. A faithful per-project validation needs `project.godot`-scoped roots (M4).
