# Phase 6 · Workstream 5 — Documentation Completeness Playbook

> Research-backed build plan for finishing the docs surface to a 1.0 standard:
> the mdBook user guide, the **generated** per-warning reference, the "add a client" guide, the CLI
> reference, the contract page, polished docs.rs, the playground wired as live docs, and CI-built
> `examples/`. **Grounded against the code as it exists on `cleanup_and_upgrades` HEAD** — verified, not
> restated, with the divergences from the plan called out.
>
> **Parents:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 5,
> [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) §5
> (docs.rs metadata, mdBook + `SUMMARY.md`, the "add a client" page).
> **Depends on:** W1 (the `WarningCode` table — the warning reference is *generated* from it),
> W6 (the contract-page prose: the semver policy statement + supported-Godot matrix). Both consumed,
> not authored, here.

---

## 0. Thesis

Docs are **not a writing task, they are a wiring task.** The scaffold from Phase 0 already exists and is
deployed (mdBook at `/`, playground at `/playground/`, docs.rs metadata on every crate). The 1.0 job is to
(a) **close the structural gaps** in `SUMMARY.md` (Configuration, Warning Reference, CLI Reference, the
per-editor client pages + "add a client" guide, the contract page), (b) make the highest-drift surface —
the **per-warning reference** — *generated from `WarningCode` (W1)* so it provably cannot diverge from the
implementation, (c) turn the existing playground into **live docs** (every warning page deep-links a
prefilled playground), and (d) add a **CI-built `examples/`** tree so Rust/Node/browser/CLI snippets can't
rot. The one real engineering deliverable is the **docgen xtask** (#0 risk: the warning reference drifting);
everything else is content + CI plumbing over machinery that already works.

---

## 1. Goal — the 1.0 cut vs the deferred tail

### 1.0 cut (ships at the tag)

1. **`SUMMARY.md` completed** — every section in the plan §5.1 skeleton exists and is written: Install,
   Quickstart, Consume-from-{Rust,Node,Browser}, **Configuration**, **Warning Reference** (generated),
   **Editor/LSP Client Integration** (Overview + VS Code + Neovim + Godot-external-editor + **"Adding a new
   client"**), **Reference** (CLI + config-schema + LSP capabilities + the **contract page**), Contributing.
2. **The warning reference is generated** from the `WarningCode` table (W1) by a `cargo xtask docgen`
   task, checked in, and **CI-verified up-to-date** (a `--check` mode fails the PR if regen would change the
   tree). One page per warning: code, default level, engine message, a triggering example, suppression
   (`@warning_ignore` + the `project.godot` key), the documented divergence for `UNSAFE_PROPERTY_ACCESS` /
   `UNSAFE_METHOD_ACCESS`, and a **live-playground deep link**.
3. **docs.rs polished for `gdscript-ide`** — the contract crate has module-level docs on every public type,
   a doctested top-level usage example, and the public-vs-internal boundary stated. (Metadata already
   present on all crates — §2 below.)
4. **The playground is live docs** — each warning page + the narrowing pages link a prefilled playground via
   URL state (`?code=…&warning=…`); the page boots Monaco with the snippet and shows the diagnostic live.
5. **`examples/` built in CI** — a minimal Rust embedder, a Node embedder, a browser snippet, and a
   CLI-in-CI snippet, all compiled/run by a CI job so they break loudly when the API moves.
6. **CI gates green** — `mdbook test` (validates Rust samples) + `mdbook-linkcheck` on every PR (already
   wired — §2), plus the new docgen-`--check` and examples-build jobs.

### Deferred tail (explicitly out of 1.0 — track in W7's public roadmap)

| Deferred | Why |
|---|---|
| A VitePress/Astro front-site with i18n + a fancier playground shell | research/07 §5.3: "start mdBook, wrap with a VitePress front site + playground later if wanted." mdBook is the 1.0 bar; the marketing site is post-1.0 optional. |
| Per-editor screencasts / GIFs | content polish, not a 1.0 correctness gate. |
| Auto-generated CLI reference from clap (`clap-markdown`) | nice-to-have; 1.0 ships a hand-written-but-tested CLI page (the flag set is small and stable — see §5.5). Generating it is a post-1.0 dedup. |
| Translations | post-1.0; no demand signal yet. |
| docs.rs deep docs on the **internal** crates | the public-vs-internal boundary is the point — internal crates get a one-paragraph "not a stable surface" banner, not full coverage (§2.2). |

---

## 2. Current state — what EXISTS vs the gap

Verified against `docs/`, `.github/workflows/docs.yml`, the crate `Cargo.toml`s, `playground/`, and the
`gdscript-ide`/`gdscript-base`/`gdscript-ffi` sources.

### 2.1 What already exists (do not rebuild)

| Surface | State | Path / evidence |
|---|---|---|
| **mdBook scaffold** | exists, deployed | `docs/book.toml`, `docs/src/SUMMARY.md`; deployed by `docs.yml` to Pages at `/` |
| **`docs.yml` Pages workflow** | exists + correct | one Pages site, two sources: mdBook → `/`, playground → `/playground/`; runs `mdbook test` + `mdbook build` on every PR, deploys on push to `master` |
| **mdbook-linkcheck** | wired | `[output.linkcheck]` in `book.toml` (`warning-policy = "error"`, `follow-web-links = false`); installed in CI via `taiki-e/install-action` |
| **docs.rs metadata** | on **every** crate | `[package.metadata.docs.rs] all-features = true; rustdoc-args = ["--cfg","docsrs"]` in all 8 published crates' `Cargo.toml`; `#![cfg_attr(docsrs, feature(doc_cfg))]` at each crate root (confirmed in `gdscript-ide/src/lib.rs:17`, `gdscript-ffi/src/lib.rs:19`) |
| **Module docs on `gdscript-ide`** | partial-good | `lib.rs` has a solid crate-level `//!` doc; `features.rs` has one. Public **types** (Diagnostic, etc.) are defined in `gdscript-base` and re-exported. |
| **Consume pages** | drafted (Phase-0 vintage) | `consume/{rust,node,browser}.md` exist but are marked "**Status: Phase 0 / forthcoming**" and describe a *fixed surface* — they must be de-staged + reconciled with the real Phase-5 API. |
| **Clients overview** | seed only | `clients/overview.md` is the "seed for the add-a-client guide," marked forthcoming, no per-editor pages. |
| **Playground** | working, deployed | `playground/index.html` (Monaco, build-less, real diagnostics/hover/completion/signature-help over the wasm `Analyzer`); `playground/pkg/`; engine blob copied from `crates/gdscript-api/src/engine_api.bin`. |
| **README** | strong, badges + playground link | root `README.md` has crates.io/docs.rs/CI/npm/license badges + a "Try it live" playground link. |
| **ADRs** | 3 written + template | `docs/src/adr/0001..0003` + `template.md`, in `SUMMARY.md`. |

### 2.2 The gap (what 1.0 must add) — and where the plan diverges from the code

**`SUMMARY.md` is missing five whole branches.** Current TOC (verified `docs/src/SUMMARY.md`) has: Intro,
User Guide (Install, Quickstart), Consuming (Rust/Node/Browser), Editor/LSP (Overview **only**), Contributing
(Architecture/Crates/Build), ADRs. It is **missing**: **Configuration**, **Warning Reference**, the
**per-editor client pages + "Adding a new client"**, the **Reference** branch (CLI / config-schema / LSP
capabilities / **contract page**).

**Plan-vs-code divergences to correct in the playbook (do not blindly follow the plan):**

1. **"Generated from the `WarningCode` table" presumes a `WarningCode` enum that does not exist yet.**
   Verified: `gdscript-ide/src/features.rs:37` emits `code: "GDSCRIPT_SYNTAX".to_owned()` — `Diagnostic.code`
   is a **`String`**, and there is **no `WarningCode` enum** anywhere in `crates/` (grep for
   `WarningCode|setting_name|default_level` → no matches). So the generator is a **hard dependency on W1**;
   it cannot be built before W1 lands the enum + its `default_level()`/`setting_name()`/`message()`/`since()`
   methods (plan §1.1). The docgen task is *designed* here and *stubbed against a tiny hand-rolled table*
   so it can be merged and CI-wired in parallel, then re-pointed at W1's real enum (§3.1, §7).

2. **The contract types live in `gdscript-base`, not `gdscript-ide`.** Plan §5.2/§6 say "`gdscript-ide`'s
   public surface is the contract." Verified: `Diagnostic`, `Severity`, `CompletionItem`, `HoverResult`,
   `CodeAction`, `TextRange`, etc. are **defined in `gdscript-base`** and **re-exported through
   `gdscript-ide`** (`gdscript-ide/src/lib.rs:21-24`, `features.rs:6-9`). The docs.rs polish (§4 below) and
   the contract page must therefore say: *the contract is `gdscript-ide`'s public surface **including the
   POD types it re-exports from `gdscript-base`**.* docs.rs deep docs belong on **both** `gdscript-ide`
   (the API) and the `gdscript-base` POD structs (the result shapes) — not `gdscript-ide` alone.

3. **docs.rs metadata is already on the internal crates too** (`gdscript-hir`, `gdscript-db`,
   `gdscript-syntax`, `gdscript-api`, `gdscript-scene`, `gdscript-base`). The plan says internal crates get
   "lighter docs"; the *metadata* is fine to keep (offline-buildable docs are good hygiene), but the 1.0
   action is to add a **one-paragraph "⚠ not a stable public surface — see the contract page" banner** to
   each internal crate's root `//!`, so a docs.rs visitor isn't misled into depending on them.

4. **mdBook is pinned to 0.4.40 in CI, but `book.toml` comments reference 0.5.** Verified:
   `docs.yml` installs `mdbook@0.4.40,mdbook-linkcheck@0.7.7` with the comment "mdbook 0.5 breaks
   mdbook-linkcheck 0.7.7; 0.4.40 is supported," while `book.toml` line 18 references "mdbook 0.5's Font
   Awesome 6 helper." **Reconcile**: the running version is 0.4.40 — fix the stale 0.5 comment in
   `book.toml`, or (preferred for 1.0) move linkcheck off the deprecated `[output.linkcheck]` backend if we
   want 0.5, but **do not** churn this in W5 unless it blocks; note it as a known inconsistency.

5. **No `examples/` directory exists** (verified: `examples/` absent at repo root). The plan's "examples
   built in CI" is greenfield — §6 designs it from scratch (Cargo `examples/` for Rust + a tiny Node/browser
   harness under `examples/node`, `examples/browser`).

6. **No docgen xtask exists.** `xtask` dispatches `ci|codegen-api|fixtures|differential|dist|release`
   (`xtask/src/main.rs:12-18`) — **no `docgen`**. §3.1 adds it.

---

## 3. Design

Three deliverables carry real design: (3.1) the **docgen xtask** (the anti-drift engine), (3.2) the
**playground-as-live-docs** URL contract, (3.3) the **examples** harness. The prose pages (Configuration,
client pages, contract page) are content over known facts and are specified in §5.

### 3.1 The docgen xtask — `cargo xtask docgen` (the #0 deliverable)

**Why a generator, not hand-written pages:** the plan's exit criterion is "Generated from the `WarningCode`
table (single source of truth) so it can't drift from the implementation." 48 warnings × {code, default
level, message, suppression key, example, divergence note} hand-maintained against an evolving enum is a
guaranteed drift source. The generator reads W1's `WarningCode` and emits the Markdown.

**Wiring** (mirror the existing dispatch in `xtask/src/main.rs`):

```rust
// xtask/src/main.rs — add to the match
"docgen" => tasks::docgen(&args[1..]),   // `--check` = verify up-to-date, fail if stale
// usage line gains: <ci|codegen-api|fixtures|differential|docgen|dist|release>
```

```rust
// xtask/src/tasks.rs (sketch — matches the existing task style)
pub fn docgen(args: &[String]) -> anyhow::Result<()> {
    let check = args.iter().any(|a| a == "--check");
    let out_dir = repo_root().join("docs/src/reference/warnings");
    let summary_frag = repo_root().join("docs/src/reference/warnings/SUMMARY.gen.md");

    // SOURCE OF TRUTH: W1's enum. Until W1 lands, a 7-entry stub table here (the Phase-2 curated
    // subset) keeps the generator + CI live; swap the `for code in WarningCode::ALL` loop in.
    let pages = render_warning_pages();          // Vec<(filename, markdown)>
    let summary = render_summary_fragment();     // the `- [CODE](warnings/code.md)` list

    if check {
        verify_matches_disk(&pages, &summary, &out_dir, &summary_frag)?;  // diff; bail! on mismatch
    } else {
        write_all(&pages, &summary, &out_dir, &summary_frag)?;
    }
    Ok(())
}
```

**Per-warning page template** (one `render_warning_page(code)` → Markdown; everything comes from
`WarningCode` methods so it can't drift):

```markdown
# `{CODE_SYMBOLIC}`            ← code.to_string()  e.g. UNSAFE_METHOD_ACCESS
**Default level:** {WARN|IGNORE|ERROR}            ← code.default_level()
**`project.godot` key:** `debug/gdscript/warnings/{setting_name}`   ← code.setting_name()
**Since:** Godot {4.3 | master}                   ← code.since()  (master-only badge if applicable)

{One-line description.}                            ← a `code.doc()` blurb (W1 adds, or a docgen-side table)

## Engine message
> {verbatim engine string}                         ← code.message(&example_args)

## Example that triggers it
` ` `gdscript
{snippet}                                          ← from docs/fixtures/warnings/{setting_name}.gd
` ` `
[▶ Open in the playground](…/playground/?warning={setting_name}&code=BASE64SNIPPET)   ← §3.2

## How to suppress
- One statement: `@warning_ignore("{setting_name}")`
- A region: `@warning_ignore_start("{setting_name}")` … `@warning_ignore_restore("{setting_name}")`
- Project-wide: set `debug/gdscript/warnings/{setting_name} = 0` in `project.godot`

{IF code ∈ {UNSAFE_PROPERTY_ACCESS, UNSAFE_METHOD_ACCESS}:}
## ⚠ Where we differ from the engine
The Godot checker emits this even inside a proven `if x is T:` guard ([#93510]).
gdscript-analyzer narrows the receiver inside the guard, so the access is provably safe and **we emit
nothing**. This is a deliberate, documented divergence (see the [Narrowing](…) page). [#93510]: …
```

**Example snippets are real fixtures, not inline strings.** Reuse the testing-strategy corpus
`fixtures/warnings/<setting_name>.gd` (plan §Testing #1 — already the differential-harness corpus). The
generator `include_str!`-style reads the fixture so the doc example is *the same source the test asserts on*
— a third anti-drift guarantee (doc == test == implementation).

**CI verification** (the anti-drift gate): a job runs `cargo xtask docgen --check`; if the committed tree
differs from regen, it fails with a "run `cargo xtask docgen`" message. Add to `xtask ci` so it runs locally
too. This is the same pattern the repo already uses for `codegen-api`/`fixtures`.

**`SUMMARY.md` inclusion:** the generated per-warning list is emitted to a fragment
(`reference/warnings/SUMMARY.gen.md`) and the hand-written `SUMMARY.md` references the **index page**
(`reference/warnings/index.md`, hand-written intro) which links the generated list — keeping `SUMMARY.md`
itself hand-owned (mdBook requires a single literal `SUMMARY.md`; we don't generate *it*, only the
per-warning pages + an index body). The index page can be a generated table (category → code → level) from
the same source.

### 3.2 The playground-as-live-docs URL contract

The playground already exists and runs Monaco + the wasm `Analyzer` client-side (verified
`playground/index.html`, `playground/README.md`). Live docs = **deep-link prefilled state**, no new
analysis engine.

**URL state contract** (a tiny, documented, stable query-param schema the docgen emits and the playground
reads on boot):

| Param | Meaning | Encoding |
|---|---|---|
| `code` | the GDScript source to load into Monaco | URL-safe base64 of UTF-8 (robust to `&`,`#`,newlines) or LZ-compressed base64 if size matters (Ruff/Biome use LZ-string-style) |
| `warning` | optional — the `setting_name` to scroll/highlight to | plain string |

**Playground change (small, additive):** on load, `index.html` parses `location.search`; if `code` present,
`atob`-decode → set the Monaco model text (instead of the default sample); if `warning` present, after the
first diagnostics pass, reveal/flash the matching marker. This is ~15 lines added to the existing boot path;
the analyzer glue is untouched. Document the schema in `contributing/` so other doc pages (narrowing pages,
the README) can build links too.

**Size guard:** base64 of a ~10-line snippet is well under any URL limit; if a future page wants a large
sample, fall back to LZ compression (note it, don't pre-build it). Keep the deep link **stateless** (no
server, no shortener) — it must work on the static Pages deploy.

### 3.3 The examples harness (`examples/`, built in CI)

Four minimal, **buildable** consumers — they double as copy-paste starting points *and* a compile-time
contract test (if the public API moves, an example fails to build, loud in CI).

```
examples/
├── rust-embed/            # a Cargo example or a tiny member crate
│   └── main.rs            # AnalysisHost::new() → apply_change → analysis().diagnostics(file) → print
├── node-embed/
│   ├── package.json       # "@gdscript-analyzer/core": "workspace:*" (or the published version in CI)
│   └── index.mjs          # new AnalysisHandle(); openDocument(uri, src); JSON.parse(diagnostics(uri))
├── browser-embed/
│   └── index.html         # <script type=module> import init, { Analyzer } from the wasm pkg; analyze a string
└── cli-in-ci/
    └── check.sh           # gdscript check fixtures/ ; assert exit code; the pre-commit/CI snippet
```

**Build-in-CI strategy** (each cheap, none requires a GUI):
- **rust-embed** — a Cargo `examples/` target on `gdscript-ide` (or a workspace member): `cargo build
  --example rust-embed` in CI; even better, run it and assert output (it's a 20-line program).
- **node-embed** — `cargo xtask`/CI builds the napi addon, then `node examples/node-embed/index.mjs`
  asserts a known diagnostic. Gate behind the napi-build job (it needs the `.node` binary).
- **browser-embed** — headless smoke: reuse the playground's wasm `pkg/`; a Playwright/headless-chrome step
  loads the page and asserts the diagnostic count (the plan's "smoke test loads the wasm + pruned API asset
  and analyzes a snippet in-browser (headless)" — Testing #6 — *is* this example).
- **cli-in-ci** — `gdscript check examples/.../sample.gd` and assert the exit code (trivial; the CLI exists
  from Phase 5).

The Rust + CLI examples are the cheap must-haves; node/browser examples piggyback on the existing
napi/wasm CI jobs (don't stand up new toolchains). Each example's `main`/`index` is `include_str!`/`mdbook
{{#include}}`-pulled into the relevant consume page so the **doc snippet == the compiled example** (anti-drift
again).

---

## 4. docs.rs polish for the contract crate(s)

The metadata is already correct everywhere (§2.1). The 1.0 work is **content + boundary clarity**:

1. **`gdscript-ide`** — every `pub` item reachable from the root gets rustdoc. The crate `//!` already
   explains `AnalysisHost`/`Analysis`/`Cancellable`; **add** a top-level `## Example` in the crate doc that
   is a **doctest** (`cargo test --doc -p gdscript-ide` must pass — this is also Testing #7's "doctest of the
   documented public-API usage example"):

   ```rust
   //! ## Quick start
   //! ` ` `
   //! use gdscript_ide::{AnalysisHost, Change};
   //! # use gdscript_base::FileId;
   //! let mut host = AnalysisHost::new();
   //! let file = FileId(0);
   //! let mut change = Change::new();
   //! change.change_file(file, "func f():\n\treturn 1\n");
   //! host.apply_change(change);
   //! let diags = host.analysis().diagnostics(file).unwrap();
   //! assert!(diags.is_empty());
   //! ` ` `
   ```
   (Reconcile the exact constructor/`FileId` shape against `lib.rs` at write time — verified shapes:
   `AnalysisHost::new()`, `Change::new()`, `Change::change_file(FileId, impl Into<Arc<str>>)`,
   `apply_change(Change)`, `analysis().diagnostics(FileId) -> Cancellable<Vec<Diagnostic>>`.)

2. **`gdscript-base` POD types** — because the contract *includes* the re-exported result structs (§2.2 #2),
   `Diagnostic`, `Severity`, `CodeAction`, `CompletionItem`, `HoverResult`, `TextRange`, etc. each need a
   doc line: what the field means, that ranges are **UTF-8 byte offsets** (UTF-16 conversion is the client's
   job), and a link to the contract page. When W1 lands, the `code: String` field doc points at the Warning
   Reference; when W6 marks types `#[non_exhaustive]`, note it in the rustdoc.

3. **Internal crates** — add a banner to each root `//!`:
   `//! ⚠ **Not a stable public surface.** This crate is an implementation detail of `gdscript-ide`; only `gdscript-ide` (and the POD it re-exports) is semver-stable. See the [contract page]. `
   (`gdscript-hir`, `gdscript-db`, `gdscript-syntax`, `gdscript-api`, `gdscript-scene`.)

4. **Offline-build check** — preview locally with
   `RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc --no-deps --all-features -p gdscript-ide` (research/07
   §5.1); optionally add a CI job that builds docs with `-D warnings` (rustdoc lints: broken intra-doc
   links, missing-docs on `gdscript-ide` public items). `#![deny(missing_docs)]` on `gdscript-ide` is the
   strongest anti-drift lever for the API surface — propose it (it forces a doc on every new `pub`).

---

## 5. The pages — content spec (`SUMMARY.md` target tree)

Target `SUMMARY.md` (the literal mdBook TOC; the per-warning leaves under Warning Reference are the only
generated part):

```markdown
# Summary
- [Introduction](README.md)
- [User Guide]()
  - [Install](guide/install.md)
  - [Quickstart](guide/quickstart.md)
  - [Configuration](guide/configuration.md)                 # NEW
- [Consuming the Library]()
  - [From Rust](consume/rust.md)                            # de-stage, reconcile to Phase-5 API
  - [From Node](consume/node.md)                            # de-stage
  - [From the Browser](consume/browser.md)                  # de-stage
- [Editor / LSP Client Integration]()
  - [Overview](clients/overview.md)
  - [VS Code](clients/vscode.md)                            # NEW
  - [Neovim](clients/neovim.md)                             # NEW
  - [Godot external editor](clients/godot.md)               # NEW
  - [Adding a new client](clients/adding-a-client.md)       # NEW (the rust-analyzer "Other Editors" model)
- [Warning Reference](reference/warnings/index.md)          # NEW — index hand-written, leaves GENERATED
  - [... 48 generated per-warning pages ...]                # from `cargo xtask docgen` (SUMMARY.gen fragment)
- [Reference]()
  - [CLI](reference/cli.md)                                 # NEW
  - [Configuration schema](reference/config-schema.md)      # NEW
  - [LSP capabilities](reference/lsp-capabilities.md)       # NEW
  - [Stability & supported Godot versions](reference/contract.md)  # NEW (the contract page — W6 prose)
- [Contributing]()
  - [Architecture](contributing/architecture.md)
  - [Crate layout](contributing/crates.md)
  - [Build & test](contributing/build.md)
  - [Playground deep-link schema](contributing/playground-links.md)  # NEW (the §3.2 URL contract)
- [Architecture Decision Records](adr/README.md)
  - [...existing ADRs...]
```

### 5.1 Configuration (`guide/configuration.md`)
The user-facing config story, grounded in W1 + the formatter (W3) + the CLI (Phase 5):
- **Warning settings** — the `debug/gdscript/warnings/*` keys (master `enable`, per-code level
  `Ignore(0)/Warn(1)/Error(2)`, `treat_warnings_as_errors`, scope: `exclude_addons` / `directory_rules`),
  mapped to the `project.godot` they come from (plan §1.3). State the **analyzer-default deviation**
  (`--engine-defaults` vs `--strict`) verbatim (plan §1.3).
- **`@warning_ignore[_start|_restore]`** — the suppression annotations (plan §1.4), names = the lowercase
  setting names; link each to its Warning Reference page.
- **Formatter config** — `gdscript-analyzer.toml` / `[tool.gdformat]`-compat section: `line_width` (100),
  `indent` (tabs), `indent_size` (4), `safe_mode` (true) (plan §W3.2). *Gate the formatter rows on W3
  landing; if W3 slips, mark "forthcoming."*

### 5.2 Consume pages (de-stage + reconcile)
The three `consume/*.md` are Phase-0-vintage and stamped "forthcoming." Remove the status banners, and
reconcile each to the **real shipped surface**:
- **Rust** (`consume/rust.md`) — already accurate to `AnalysisHost`/`Analysis`; verify the method list
  against `lib.rs` (it currently shows `analysis.completions(pos)?` etc. — confirm names match the real
  `Analysis` methods at write time; the `diagnostics(FileId)` signature is verified). Replace the
  `cargo add gdscript-ide` "coming in 0.x" with the 1.0 version.
- **Node** (`consume/node.md`) — point at the real `AnalysisHandle` surface (verified in
  `gdscript-ffi/src/lib.rs`: `new AnalysisHandle()`, `openDocument`/`changeDocument`/`closeDocument`,
  `setProjectConfig`, query-by-URI returning **JSON strings** the client `JSON.parse`s). Pull the snippet
  from `examples/node-embed`.
- **Browser** (`consume/browser.md`) — the wasm `Analyzer` + the `loadEngineApi` data-asset fetch
  (`./data/extension_api.bin`); pull the snippet from `examples/browser-embed`. Note the byte↔UTF-16
  conversion the client owns.

### 5.3 Client pages (the "add a client" guide is the headline)
Model the **"Adding a new client"** page on rust-analyzer's *Other Editors* (research/07 §5.4). It must cover:
finding the `gdscript-lsp` binary on `PATH`; transport (stdio — verify against the Phase-5 LSP); **all
settings via LSP `initializationOptions`** with a documented JSON schema + a default example; the advertised
capabilities (cross-link to `reference/lsp-capabilities.md`); enabling tracing for client authors; and the
**embed-the-library alternative** (the guitkx path → `consume/rust.md` / docs.rs). The per-editor pages
(VS Code, Neovim, Godot-external-editor) are concrete setup snippets pointing at the Phase-5 server binary.

### 5.4 The contract page (`reference/contract.md`) — **content owned by W6**
This page **embeds W6's two verbatim blocks**: the *1.0 stability policy statement* (plan §6.2) and the
*supported-Godot-version matrix* (plan §6.3). W5 owns the page's existence, placement, cross-links (from the
Warning Reference's divergence notes, from the consume pages, from docs.rs), and the framing prose; **W6 owns
the normative text.** Critical correction to carry: the contract is *"`gdscript-ide`'s public surface
**including the POD result types it re-exports from `gdscript-base`**"* (§2.2 #2), not `gdscript-ide` alone —
state this explicitly so consumers know `gdscript-base` POD shapes are also frozen.

### 5.5 CLI reference (`reference/cli.md`)
Hand-written (small, stable surface), grounded in `PHASE-5-CLI-PLAYBOOK.md`: the command tree
(`check`/`lint`/`format`/`symbols`), global flags (`--format human|json|github|sarif|rdjson`,
`--output-file`, `--config`/`--no-config`, `--error-on-warning`, `--quiet`, `--no-color`), the
`--engine-defaults` vs `--strict` warning-mode flags (plan §1.3), and the **exit-code table** (0/1/2/101 —
CLI playbook §6, the load-bearing contract for CI authors). A doctested `cli-in-ci` example backs it.

---

## 6. Step-by-step implementation plan

Ordered so each step is independently mergeable and CI-green; the W1/W6-dependent steps are isolated so W5
can proceed in parallel and "snap in" the real source when the dependency lands.

1. **M0 — structural completion (no dependencies).** Expand `SUMMARY.md` to the §5 target tree; create all
   NEW pages as stubs with real headings + "forthcoming where it depends on W1/W3/W6" banners; de-stage the
   three consume pages and reconcile them to the verified Phase-5 surface; fix the stale mdBook-0.5 comment
   in `book.toml` (§2.2 #4). *Exit:* `mdbook build docs` + `mdbook test` + linkcheck green on PR (the
   existing `docs.yml` job covers this).

2. **M1 — the docgen xtask (against a stub table).** Add `tasks::docgen` + the `xtask docgen` dispatch
   (§3.1); render per-warning pages + the index + the `SUMMARY.gen.md` fragment from a **7-entry stub table**
   (the Phase-2 curated subset) so the machinery is live before W1; wire `cargo xtask docgen --check` into a
   CI job and into `xtask ci`. *Exit:* `docgen` writes pages, `docgen --check` passes, the index renders in
   the book.

3. **M2 — playground live-docs.** Implement the §3.2 URL-state contract in `playground/index.html` (parse
   `?code=&warning=`, prefill Monaco, flash the marker); document the schema in
   `contributing/playground-links.md`; make the docgen emit the `▶ Open in the playground` link per page.
   *Exit:* a generated warning page's link opens the playground prefilled with that warning's fixture and
   shows the diagnostic live (verify headless — §3.3 browser-embed reuses this).

4. **M3 — examples + CI.** Create `examples/{rust-embed,node-embed,browser-embed,cli-in-ci}` (§3.3); add a
   CI examples-build job (Rust + CLI always; node/browser gated on the existing napi/wasm jobs); wire
   `{{#include}}` so the consume-page snippets are the compiled examples. *Exit:* CI builds/runs all four;
   editing the public API breaks the relevant example.

5. **M4 — docs.rs polish.** Add the doctested top-level example to `gdscript-ide` (§4.1); doc the
   `gdscript-base` POD types (§4.2); add the internal-crate "not stable" banners (§4.3); propose
   `#![deny(missing_docs)]` on `gdscript-ide`; add the rustdoc-`-D warnings` CI check. *Exit:*
   `cargo test --doc -p gdscript-ide` green; `RUSTDOCFLAGS="--cfg docsrs" cargo +nightly doc` clean.

6. **M5 — snap in W1.** When W1 lands `WarningCode`, replace the docgen stub table with the
   `WarningCode::ALL` loop reading `default_level()`/`setting_name()`/`message()`/`since()`; regenerate;
   the 48 pages now exist and `--check` guards them. Add the `UNSAFE_*` divergence block conditionally.
   *Exit:* 48 generated pages, differential corpus fixtures used as the examples, `--check` green.

7. **M6 — snap in W6.** Paste W6's verbatim stability policy + Godot matrix into `reference/contract.md`;
   cross-link from the warning divergence notes, consume pages, docs.rs. *Exit:* the contract page is
   normative and link-checked.

8. **M7 — final pass.** Run the full exit-criteria checklist (§ Exit-criteria mapping below); ensure every
   plan-§5 surface is present, generated where required, and CI-guarded.

---

## 7. Test plan (docs as a tested artifact)

Docs ship with the same discipline as code — "tested" means CI fails on drift, broken links, broken samples,
and broken examples.

1. **mdBook samples** — `mdbook test docs` (already in `docs.yml`) validates every Rust code block compiles.
   Mark non-compiling illustrative blocks `rust,ignore` (the existing `consume/rust.md` already does this).
2. **Link integrity** — `mdbook-linkcheck` (`warning-policy = "error"`) on every PR (already wired). A
   periodic job flips `follow-web-links = true` to catch dead external links (the #93510 link, crates.io,
   napi.rs) without making PR CI flaky.
3. **Docgen freshness (the anti-drift gate)** — `cargo xtask docgen --check` in CI + `xtask ci`: regen and
   diff; fail with "run `cargo xtask docgen`" if the committed warning pages are stale. **This is the test
   that makes "can't drift from the implementation" true.**
4. **The doctested public-API example** — `cargo test --doc -p gdscript-ide` (Testing #7). The top-level
   usage example in the crate `//!` must run, so a breaking API change breaks the doctest.
5. **Examples build/run** — the M3 CI job: `cargo build --example rust-embed` (+ run + assert),
   `node examples/node-embed` (assert a known diagnostic), `gdscript check` the CLI sample (assert exit
   code), headless-browser load of `browser-embed` (assert diagnostic count = the in-browser smoke test,
   Testing #6). An API change that breaks a consumer surfaces here.
6. **Playground deep-link round-trip** — a headless test: take a generated page's `▶ playground` URL, load
   it, assert Monaco's model == the fixture source and the expected marker is present (validates the §3.2
   encode/decode + prefill path).
7. **`#![deny(missing_docs)]` on `gdscript-ide`** (M4) — a compile-time test that every public item is
   documented; plus rustdoc `-D warnings` to catch broken intra-doc links.
8. **Contract-page consistency** — assert the supported-Godot matrix in `reference/contract.md` matches the
   bundled API versions the code actually ships (cross-check against the `gdscript-api` engine blobs / W6's
   matrix); a small test or `xtask` check so the matrix can't lie. (Coordinate the source of truth with W6.)

No fixture/golden/property corpus is W5-owned beyond the docgen golden (the committed generated tree *is* the
golden, guarded by `--check`); the warning **example** fixtures are W1/Testing-owned and *reused* here.

---

## 8. Risks + mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **Warning reference drifts from the implementation** (the whole reason it's generated) | **Critical** | `cargo xtask docgen` from `WarningCode` (single source) + `--check` CI gate (§3.1, Test #3); examples reuse the test fixtures so doc == test == impl. |
| **W1 not landed when W5 needs the enum** | **High** | docgen built + CI-wired against a **7-entry stub table** (M1); M5 snaps in `WarningCode::ALL`. W5 is unblocked on everything except the final 48-page fill. |
| **W6 contract prose not ready** | Med | the contract page exists as a stub with the framing prose (M0); W6's verbatim blocks paste in at M6. The §2.2-#2 correction (contract = `gdscript-ide` **+ re-exported `gdscript-base` POD**) is W5's to carry into the page. |
| **Consume pages document a surface that moved since Phase 0** | Med | reconcile each against the verified sources at write time (`gdscript-ide/src/lib.rs`, `gdscript-ffi/src/lib.rs`); back snippets with the CI-built `examples/` so they can't silently rot (M3). |
| **Playground deep links break on the static Pages deploy** | Med | stateless URL state (base64, no server/shortener); headless round-trip test (Test #6); the deploy already serves the playground at `/playground/` (verified `docs.yml`). |
| **mdBook 0.4.40 vs 0.5 / linkcheck-backend churn** | Low | don't upgrade in W5; fix the stale `book.toml` comment, note the pin as known. If 0.5 is wanted later it's its own task (linkcheck backend changed). |
| **docs.rs offline build fails** (network-isolated sandbox) | Low | metadata already correct; the M4 `RUSTDOCFLAGS=--cfg docsrs cargo +nightly doc --no-deps` preview + a CI rustdoc job catch it before publish. |
| **Examples explode CI cost / need GUIs** | Low | Rust + CLI examples are trivial; node/browser piggyback on the existing napi/wasm jobs; browser is the headless smoke test we owe anyway (Testing #6) — net-neutral. |

**Biggest correctness risk:** the warning-reference drift — owned by the docgen `--check` gate.
**Biggest dependency risk:** W1's `WarningCode` enum — de-risked by the stub-table-then-snap-in sequencing.

---

## 9. Dependencies on other workstreams

| Dependency | Direction | What W5 needs / gives |
|---|---|---|
| **W1 — full warning set** | **W5 depends on W1** | the `WarningCode` enum + `default_level()` / `setting_name()` / `message()` / `since()` (plan §1.1) are the docgen source; the `fixtures/warnings/<name>.gd` corpus is reused as the page examples. **Hard blocker for the final 48-page fill** (mitigated by the stub table). |
| **W6 — API stabilization & contract** | **W5 depends on W6** | the verbatim *stability policy statement* (§6.2) + *supported-Godot matrix* (§6.3) for the contract page; the `#[non_exhaustive]` decisions to note in the POD rustdoc. W5 supplies back the §2.2-#2 correction (contract = `gdscript-ide` + re-exported `gdscript-base` POD). |
| **W3 — formatter** | **W5 depends on W3 (soft)** | the formatter config (`line_width`/`indent`/`safe_mode`) for `guide/configuration.md` + `reference/config-schema.md`; if W3 slips, those rows are marked "forthcoming" (non-blocking). |
| **Phase 5 — CLI** | **W5 depends on (shipped)** | the CLI command tree, flags, and exit codes for `reference/cli.md` (from `PHASE-5-CLI-PLAYBOOK.md`); the `cli-in-ci` example. Already shipped. |
| **Phase 5 — LSP** | **W5 depends on (shipped)** | the advertised capabilities + `initializationOptions` schema + stdio transport for the client pages. Already shipped. |
| **Phase 5 — playground** | **W5 extends (shipped)** | the existing `playground/index.html` gains the §3.2 URL-state prefill. Already shipped + deployed. |
| **W2 — narrowing** | **W5 depends on (soft)** | the narrowing pages + the `UNSAFE_*` divergence prose / the #93510 live demo link; if narrowing examples shift, the fixtures (shared with W1) keep them honest. |
| **W7 — ecosystem** | **W5 feeds W7** | the "add a client" guide + the docs lower the integration cost feeding the ≥1-external-consumer criterion; the deferred-tail items (front-site, CLI-gen) belong on W7's public roadmap. |

---

## 10. Exit-criteria mapping (to PHASE-6 §Exit criteria)

| Plan exit criterion | This playbook |
|---|---|
| "Docs complete: mdBook guide (install / consume-from-Rust-Node-browser / configuration / per-warning reference / 'add a client' guide) finished and link-checked" | §5 page tree + M0–M2; linkcheck already wired (§7.2) |
| "docs.rs API docs polished" | §4 + M4 (doctested example, POD docs, internal banners, `deny(missing_docs)`) |
| "examples build in CI" | §3.3 + M3 |
| "A live web playground … wired into the docs (warning + narrowing pages link live demos)" | §3.2 + M2 (per-page `▶ playground` deep links) |
| "the contract page (semver policy + supported-Godot matrix) is published" | §5.4 + M6 (W6 prose, W5 page) |
| Warning reference "Generated from the `WarningCode` table … so it can't drift" | §3.1 + M1/M5 + the `--check` gate (§7.3) |

---

## Sources (verified against the tree, 2026)

- **Plan:** `PHASE-6-V1-RELEASE.md` §Workstream 5 (the three doc surfaces), §1.1–1.4 (the `WarningCode`
  table the reference is generated from), §6.2–6.3 (the contract-page prose), §Testing #6–7, §Exit criteria.
- **Research:** `research/07-ecosystem-and-release-tooling.md` §5 (docs.rs metadata pattern, mdBook +
  `SUMMARY.md` skeleton, `mdbook test` + linkcheck CI, the rust-analyzer "Other Editors" / "add a client"
  model), §5.5 (the recommended SUMMARY skeleton).
- **Verified code state:** `docs/book.toml`, `docs/src/SUMMARY.md`, `docs/src/{guide,consume,clients,
  contributing,adr}/*`; `.github/workflows/docs.yml` (Pages: mdBook `/` + playground `/playground/`, mdbook
  0.4.40 pin); `playground/{index.html,README.md}`; `crates/*/Cargo.toml` (`[package.metadata.docs.rs]` on
  all 8); `gdscript-ide/src/{lib.rs,features.rs}` (`Diagnostic.code` is a **`String`**, no `WarningCode`
  enum; POD types live in `gdscript-base`, re-exported); `gdscript-ffi/src/lib.rs` (the `AnalysisHandle`
  Node surface); `xtask/src/{main.rs,tasks.rs}` (no `docgen` task yet); root `README.md` (badges + live
  playground link). **Phase-5 playbook format bar:** `PHASE-5-CLI-PLAYBOOK.md`.
</content>
</invoke>
