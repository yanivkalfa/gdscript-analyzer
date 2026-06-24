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
- [ ] **UTF-8 BOM at file start is not skipped.** A leading `U+FEFF` is lexed as an
      unknown token, so the first declaration errors (`expected a declaration` at 1:1).
      The fix needs a dedicated BOM trivia token (folding it into `Whitespace` miscounts
      the indent column by its 3 bytes) plus `column`/`diagnose_indent` handling. Real:
      some editors save `.gd` with a BOM (one file in the ReactiveUI-Godot corpus).

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
