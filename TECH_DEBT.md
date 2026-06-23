# Tech debt & follow-ups

The running backlog of deferred work, known limitations, and queued next steps. Keep it
honest — anything we knowingly defer or stub goes here with enough context to pick it up
later.

---

## Queued next steps (ops / repo)

- [x] **Auto-delete merged branches** — enabled (`delete_branch_on_merge: true`).
      `fix/mdbook-build` deleted (confirmed merged). **`feat/phase-0-ecosystem` NOT
      deleted:** its tip diverges from master (16 files / 346 lines of stale workflow +
      docs that were superseded by PRs #12–#14 — no unique source code). It was not
      cleanly merged, so it's left for an explicit go-ahead.
- [x] **godot-sync bot proven end-to-end** — a `workflow_dispatch` with
      `godot_tag=4.4-stable` resolved the tag, fetched + vendored the API, ran
      `codegen-api`, and opened PR #16 (`chore(api): sync to Godot 4.4-stable`). Closed
      without merging (would downgrade 4.5→4.4); branch deleted.
- [ ] **godot-sync *scheduled* run is failing.** The daily 06:17 UTC schedule run
      failed (the dispatched run succeeds). Investigate the schedule-vs-dispatch
      difference (token/permissions on `schedule`, or the "Allow Actions to create and
      approve PRs" repo setting).
- [ ] **GitHub Pages site is not serving.** The `docs` CI is fully green (mdbook build +
      `upload-pages-artifact@v5` + `configure-pages@v6` + `deploy-pages@v5` all succeed;
      the `github-pages` deployment state is `success`), yet
      `https://yanivkalfa.github.io/gdscript-analyzer/` returns 404 and the Pages
      `status` is `null`. Re-asserting `Source = GitHub Actions` via the API + re-running
      the deploy did **not** fix it. **Fix:** open **Settings → Pages** and confirm
      **Source: GitHub Actions** (the first-time UI activation), then re-run the `docs`
      workflow; it should then serve.

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
- [ ] **`PI`/`TAU`/`INF`/`NAN` modeled as distinct const token kinds.** Confirm against
      Godot's tokenizer (the differential oracle excludes them); reclassify to plain
      identifiers if the engine treats them as such. Low risk.

### IDE features (Tier 0 → Tier 1)
- [ ] **Completions are not scope-aware.** By-name completion offers *every* document
      symbol, not just names visible in the enclosing scope. Acceptable for Tier 0;
      scope-awareness comes with the HIR.
- [ ] **No type inference / member completion / hover / goto / rename.** Those are
      Phase 2+ (need the engine API model + inference). The `Analysis` methods exist on
      the surface and return empty/None.
- [ ] **No salsa / incremental reparse.** Whole-file reparse on every query (a plain
      VFS map). Adopt salsa at Phase 3 — every derived computation is already a pure
      `(text) -> value` function so the swap is localized.

### FFI ergonomics
- [ ] **Bindings return JSON strings,** not typed `#[napi(object)]` / `serde-wasm-bindgen`
      objects. Works and is minimal; consider typed results for better TS/JS DX once the
      result shapes stabilize.

### Validation
- [ ] **Differential corpus is small + error-agreement only.** The tree-sitter oracle
      checks whether both parsers consider a file well-formed over ~14 core snippets. Grow
      it (e.g. the Godot demo-projects corpus) and add a structural skeleton comparison.
