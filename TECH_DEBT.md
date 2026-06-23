# Tech debt & follow-ups

The running backlog of deferred work, known limitations, and queued next steps. Keep it
honest — anything we knowingly defer or stub goes here with enough context to pick it up
later.

---

## Queued next steps (ops / repo)

- [ ] **Auto-delete merged branches.** Enable "Automatically delete head branches" in
      the repo settings (Settings → General → Pull Requests), and delete the already-
      merged `feat/phase-0-ecosystem` and `fix/mdbook-build` branches.
- [ ] **Verify the docs site is live.** Confirm the GitHub Pages URL renders the mdBook
      guide (Settings → Pages).
- [ ] **Prove the godot-sync bot end-to-end.** Actions → *Sync Godot extension_api.json*
      → Run with `godot_tag: 4.4-stable` → confirm it vendors the API and opens a sync
      PR → **close the PR without merging** (don't actually downgrade).

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
- [ ] **Lambda body inside an open bracket.** The indentation pre-pass suppresses
      indentation inside `()[]{}`, so a multiline lambda body living inside an open
      bracket (e.g. `arr.sort_custom(func(a, b):\n\treturn a < b\n)`) gets no
      `Indent`/`Dedent` markers. Godot re-enables indentation there via a stack-of-stacks
      (`saved_stacks: Vec<Vec<u32>>`); wire it. Top-level / ordinary nested lambdas are
      fine. (Documented in `crates/gdscript-syntax/src/prepass.rs`.)
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
