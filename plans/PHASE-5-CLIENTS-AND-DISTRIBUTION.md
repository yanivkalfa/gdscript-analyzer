# PHASE 5 — Clients & Distribution

> **Status:** plan. **Parallelizable:** can START as soon as Phase 2's API lands; runs alongside Phases 3/4.
> **Canonical parents this doc obeys:** [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§1 client crates, §2 the `AnalysisHost`/`Analysis` POD API, §4 FFI/WASM strategy, §5 data shipping, §7 portability), [`ROADMAP.md`](ROADMAP.md) (Phase 5 deliverable + exit criteria; the dependency graph).
> **Primary evidence:** [`research/01-rust-distribution-tooling.md`](research/01-rust-distribution-tooling.md) (napi per-platform npm packaging, the `binding.js` loader, crates.io publishing, release-plz/cargo-dist), [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md) (the web playground, Monaco/CodeMirror, bundle size, UTF-16, data shipping). **Secondary:** [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) (the LSP server as a thin client; the Volar embedded adapter), [`research/05-prior-art-and-landscape.md`](research/05-prior-art-and-landscape.md) (the engine-LSP gaps we beat).

This phase turns "we have a semantic analysis library" into "people can use it." It builds the **four clients** that sit where rust-analyzer's server crate sits — each a thin adapter mapping our protocol-neutral POD ([`01`](01-ARCHITECTURE.md) §2) into its own shape — and ships the whole thing as **GA packages** on crates.io + npm with a **live web playground**. None of this touches the core library: adding a client never edits `gdscript-ide`.

---

## Goal & scope

### What ships

1. **`gdscript-lsp`** — a real, **standalone, spec-compliant** LSP server binary. It runs with **no Godot editor**, over stdio, in VS Code / Neovim / Helix / Zed / Emacs. It advertises and answers the features the engine LSP **lacks or hides**: **semantic tokens, inlay hints, workspace symbols, rename, find-references** ([`research/05`](research/05-prior-art-and-landscape.md) §3). It is the **only** crate that knows `lsp-types` / JSON-RPC and the only place that maps our byte-offset POD ↔ LSP UTF-16.
2. **The guitkx integration** — **delete `godotProxy.ts`** (and the redundant `classdb.ts`). guitkx's Node LSP consumes the **`@gdscript-analyzer/core`** napi package through a **Volar-style source-map adapter**: extract `{expr}` blocks → synthetic `.gd` + source map → query our `AnalysisHost` → map results back. No running Godot editor, parity-or-better with the old proxy. **This validates the entire API design.**
3. **`gdscript-cli`** — `check` / `lint` / `format` / `symbols` for local dev + CI. Native binary; **this** is where filesystem reads live (never the core). Human + JSON + CI-friendly output, well-defined exit codes.
4. **The WASM web playground** — paste GDScript, get diagnostics/hover/completion **in-browser**, à la [play.ruff.rs](https://play.ruff.rs) / the Biome playground. Vite + an editor (Monaco **or** CodeMirror 6) calling the WASM analyzer's exports **directly** (no LSP-over-WASM). Deployed to GitHub Pages.
5. **Distribution GA** — published packages: **crates.io** (`gdscript-ide` = the public Rust API, via release-plz), **npm** (`@gdscript-analyzer/core` napi main package + per-platform `optionalDependencies`; `@gdscript-analyzer/wasm`), and **cargo-dist** binaries for the CLI + LSP. A **single shared version** across crates + npm, with **npm provenance**.

### Sequencing note (load-bearing)

Per [`ROADMAP`](ROADMAP.md) §dependency-graph: the LSP + guitkx swap + CLI **can begin the moment Phase 2's API is usable**; the **playground** needs the WASM binding (built in Phase 0, fed features as they land). **But GA on crates.io + npm should WAIT until the Phase-2 API (`gdscript-ide`) is semver-stable enough not to churn consumers** — every published artifact pins to one shared version, and breaking `gdscript-ide` breaks every client. Build and dogfood the clients pre-GA (the `ra_ap_*`-style early-access trick is available, [`research/01`](research/01-rust-distribution-tooling.md) §1.5); flip the GA switch once the API stops moving.

### Non-goals (deferred)

| Deferred | Where | Why not here |
|---|---|---|
| The formatter *implementation* (gdformat-compatible) | **Phase 6** | Phase 5 wires `cli format` + LSP `formatting` to `Analysis::format`; the real formatter is Phase 6. Here it may be a stub/passthrough. |
| Full 48-warning set + project-settings gating | **Phase 6** | Clients surface whatever diagnostics the core emits; the full set/gating lands in 6. |
| Python (PyO3) / C-ABI (cbindgen) clients | post-v1 | Cheap optionality ([`01`](01-ARCHITECTURE.md) §4); no core changes; out of the four-client scope. |
| Rename *correctness* | **Phase 3** | Phase 5 *exposes* rename over LSP; the cross-file `SourceChange` it renders is produced by Phase 3's engine. Phase 5 owns only the LSP `WorkspaceEdit` mapping. |

---

## Prerequisites

- **Phase 2 API stable** ([`PHASE-2`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md)): the `gdscript-ide` `AnalysisHost`/`Analysis` surface with `completions`/`hover`/`signature_help`/`diagnostics`/`document_symbols`/`inlay_hints`/`code_actions`, returning POD ([`01`](01-ARCHITECTURE.md) §2). **GA gates on this being semver-stable.**
- **Phase 0 distribution tooling/CI** ([`PHASE-0`](PHASE-0-ECOSYSTEM-AND-TOOLING.md)): the cargo workspace, the cross-target CI matrix, release-plz + cargo-dist wired (dry-run green), the `gdscript-api` codegen artifact + its pruned→rkyv→brotli web variant ([`01`](01-ARCHITECTURE.md) §5), and the CI-enforced `cargo check -p gdscript-ide --target wasm32-unknown-unknown` ([`01`](01-ARCHITECTURE.md) §7).
- **The napi + wasm bindings from Phase 1** (`gdscript-ffi`): the napi-rs v3 binding producing both a Node `.node` and a `wasm32` build, holding a stateful `AnalysisHandle` ([`01`](01-ARCHITECTURE.md) §4), JSON POD in/out. Phase 5 *packages* and *consumes* these; it does not invent them.
- **Phase 3** for the cross-file features the LSP advertises (rename / find-references / workspace symbols are *correct* only once Phase 3 fills `resolve_external`). The LSP server can ship Phase-2-only (single-file) capabilities first and light up the rest as Phase 3 lands — capability advertisement is data-driven (§Workstream 1).

---

## Workstream 1 — `gdscript-lsp` (the standalone LSP server)

`gdscript-lsp` is the crate that plays rust-analyzer's `rust-analyzer` (server) role: *"the only crate that knows about LSP and JSON serialization"* ([`research/06`](research/06-analyzer-architecture.md) §1, §4). It is a **thin client** over `gdscript-ide` — it holds one `AnalysisHost`, forks `Analysis` snapshots per request, and maps POD ↔ LSP. **No analysis logic lives here.**

### 1.1 The bar: beating the engine LSP

The Godot in-engine LSP ([`research/05`](research/05-prior-art-and-landscape.md) §3) requires a **running editor** (TCP :6005), **does not expose rename / find-references** (proposals #3687, #899, discussion #7952), has **fragile completion** (#101306), is **spec-non-compliant** (multi-connection), and the engine team *itself* proposes externalizing it (#11056). We beat it on every axis:

| Capability | Engine LSP | `gdscript-lsp` |
|---|---|---|
| Runs without a Godot editor | ✗ (needs :6005) | ✓ **standalone, stdio** |
| Rename | ✗ not exposed | ✓ (Phase 3 engine) |
| Find references | ✗ not exposed | ✓ (Phase 3 engine) |
| Workspace symbols | ✗ | ✓ |
| Semantic tokens | ✗ | ✓ |
| Inlay hints (inferred types) | ✗ | ✓ (Phase 2 engine) |
| Spec compliance / stdio | partial (TCP, multi-conn) | ✓ stdio JSON-RPC, single session |
| Headless / CI | engine process required | ✓ pure binary |

### 1.2 Transport & lifecycle

- **Transport:** stdio JSON-RPC (the universal default; works for VS Code, Neovim, Helix, Zed, Emacs out of the box — no TCP-bridge hacks like the engine needs). Crate: `tower-lsp` or `lsp-server` + `lsp-types`; an async runtime (`tokio`) on the **native** target only.
- **Lifecycle:** `initialize` (negotiate `positionEncoding`, read client capabilities) → advertise our `ServerCapabilities` → `initialized` → serve. `shutdown`/`exit` clean teardown.
- **State:** one `AnalysisHost`. `didOpen`/`didChange`/`didClose`/`didSave` → `apply_change` (push text via the VFS — the server reads files from disk for `workspace/didChangeWatchedFiles`, the **core never does**). Each request forks `Analysis::snapshot()`; a newer `apply_change` cancels in-flight reads (the `Cancellable<T>` boundary, [`01`](01-ARCHITECTURE.md) §2) → the server replies with the LSP `ContentModified` error and the client re-issues.
- **File watching:** register `workspace/didChangeWatchedFiles` for `**/*.gd`, `project.godot`, `**/*.tscn` (the Phase 3/4 project inputs); on change, `apply_change`.
- **Config:** `workspace/configuration` for warning gating (Phase 6), Godot version override, format-on-save. Defaults are sensible without config.

### 1.3 The capability registration (sketch)

```rust
// crates/gdscript-lsp/src/capabilities.rs  (sketch — illustrative)
fn server_capabilities(client: &ClientCapabilities, neg: PositionEncoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(neg),                 // negotiated; we prefer UTF-16, fall back per client
        text_document_sync: Some(TextDocumentSyncKind::INCREMENTAL.into()),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), ":".into(), "@".into(), "$".into(), "\"".into()]),
            resolve_provider: Some(true),             // lazy hover/docs on a completion item
            ..Default::default()
        }),
        hover_provider: Some(true.into()),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]), ..Default::default() }),
        definition_provider: Some(true.into()),
        references_provider: Some(true.into()),        // Phase 3 engine
        rename_provider: Some(RenameOptions {          // Phase 3 engine; prepareRename for validation
            prepare_provider: Some(true), work_done_progress_options: Default::default() }.into()),
        document_symbol_provider: Some(true.into()),
        workspace_symbol_provider: Some(true.into()),  // BEATS the engine LSP
        semantic_tokens_provider: Some(semantic_tokens_options().into()), // BEATS the engine LSP
        inlay_hint_provider: Some(true.into()),        // BEATS the engine LSP
        folding_range_provider: Some(true.into()),
        code_action_provider: Some(true.into()),
        document_formatting_provider: Some(true.into()), // Phase 6 formatter behind it
        ..Default::default()
    }
}
```

### 1.4 The LSP ↔ POD mapping (the one rule)

Our POD speaks **byte offsets** in a `FileId`; LSP speaks **UTF-16 line/character** ([`01`](01-ARCHITECTURE.md) §4; [`research/06`](research/06-analyzer-architecture.md) §4). The server is the **only** place this conversion happens — backed by `gdscript-base::LineIndex` (the byte↔UTF-16 converter baked into the base crate, [`01`](01-ARCHITECTURE.md) §4). If a client negotiates UTF-8 in `initialize`, we honor it; default is UTF-16. A `FileId`↔`Url` map (and a `Url`↔VFS-path map) round out the translation layer.

**One full method mapping — completion, POD → `CompletionItem` with UTF-16:**

```rust
// crates/gdscript-lsp/src/handlers/completion.rs  (sketch — the load-bearing UTF-16 hop)
fn completion(state: &ServerState, p: CompletionParams) -> Result<CompletionResponse> {
    let (file, line_index) = state.resolve(&p.text_document_position.text_document.uri)?;
    // LSP UTF-16 Position -> byte offset (THE conversion, via LineIndex)
    let offset = line_index.offset_utf16(p.text_document_position.position)?;   // -> TextSize (bytes)
    let snap = state.host.analysis();                                          // cheap Send snapshot
    let items = match snap.completions(FilePosition { file, offset }) {        // POD, byte-based
        Ok(items) => items,
        Err(Cancelled) => return Err(lsp_content_modified()),                  // client re-issues
    };
    let lsp_items = items.into_iter().map(|c| CompletionItem {
        label: c.label,
        kind: Some(map_kind(c.kind)),                 // CompletionItemKind: Method/Property/Class/Keyword...
        detail: Some(c.detail),                       // the signature string
        documentation: c.doc.map(md_markup),          // MarkupContent::Markdown (resolved lazily if heavy)
        insert_text: Some(c.insert),
        text_edit: c.replace.map(|r| CompletionTextEdit::Edit(TextEdit {
            range: line_index.range_utf16(r.range),   // bytes -> UTF-16 range, the same hop backwards
            new_text: r.new_text,
        })),
        ..Default::default()
    }).collect();
    Ok(CompletionResponse::List(CompletionList { is_incomplete: false, items: lsp_items }))
}
```

Every other method is the same shape: UTF-16 position **in** → `line_index.offset_utf16` → POD query → POD result → `line_index.range_utf16` on every emitted `TextRange` → LSP type **out**.

### 1.5 Method → query → notes

| LSP method | `Analysis` query | Notes |
|---|---|---|
| `textDocument/completion` (+ `completionItem/resolve`) | `completions(pos)` | Trigger chars `.`/`:`/`@`/`$`/`"`. `resolve` lazily attaches docs/detail. POD kind → `CompletionItemKind`. |
| `textDocument/hover` | `hover(pos)` | `HoverResult { ty_label, doc }` → `MarkupContent::Markdown`; range → UTF-16. |
| `textDocument/signatureHelp` | `signature_help(pos)` | Trigger `(`/`,`. `active_param` → LSP `activeParameter`. |
| `textDocument/definition` | `goto_definition(pos)` | `NavTarget` → `Location`/`LocationLink`; ranges → UTF-16. (In-file Phase 2; cross-file Phase 3.) |
| `textDocument/references` | `find_references(pos)` | Phase 3 engine. `includeDeclaration` honored. |
| `textDocument/rename` (+ `prepareRename`) | `rename(pos, new)` | `SourceChange` → `WorkspaceEdit` (per-file `TextEdit[]`, ranges → UTF-16). `prepareRename` validates the symbol + returns its range. **Correctness inherited from Phase 3.** |
| `textDocument/documentSymbol` | `document_symbols(file)` | POD tree → `DocumentSymbol[]` (hierarchical). |
| `workspace/symbol` | `workspace_symbols(query)` | Fuzzy query over the project symbol index (Phase 3). **Engine LSP lacks this.** |
| `textDocument/semanticTokens/{full,range}` | `semantic_tokens(file)` | POD token stream → LSP **delta-encoded** `(deltaLine, deltaChar, len, type, mods)` in UTF-16; legend declared in `initialize`. **Engine LSP lacks this.** |
| `textDocument/inlayHint` | `inlay_hints(file)` | Inferred `: T` on `:=`/untyped decls + params → `InlayHint { position, label, kind: Type }`. **Engine LSP lacks this.** |
| `textDocument/foldingRange` | `folding_ranges(file)` | POD ranges → `FoldingRange` (line-based). |
| `textDocument/codeAction` | `code_actions(range)` + diagnostic-attached fixes | `CodeAction { title, edit: WorkspaceEdit, kind: QuickFix }`. |
| `textDocument/formatting` | `format(file)` | `SourceChange` → `TextEdit[]`. Phase 6 formatter behind it. |
| `textDocument/publishDiagnostics` (push) | `diagnostics(file)` | On `apply_change`, push `Diagnostic[]`: byte `TextRange` → UTF-16, POD severity → `DiagnosticSeverity`, `code` (e.g. `UNSAFE_METHOD_ACCESS`) → LSP `code`, fixes → `relatedInformation`/code-action data. |

---

## Workstream 2 — The guitkx integration (delete the Godot proxy)

This is the **API-design validation**: the first *full* external consumer of the published napi package, replacing a live-editor proxy with our in-process analyzer via a **Volar-style source-map adapter** ([`research/06`](research/06-analyzer-architecture.md) §5). Phase 2 *proved* the single-file API can answer embedded-GDScript questions ([`PHASE-2`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) §Workstream 7); **Phase 5 builds the full adapter and deletes the proxy.**

### 2.1 What guitkx does today (the path we delete)

The guitkx Node LSP (`C:\Yanivs\GameDev\ReactiveUI\ReactiveUI-Gadot\ide-extensions\lsp-server`, TypeScript) for embedded GDScript in `.guitkx` `{expr}` blocks:
1. builds an in-memory virtual `.gd` (`src/virtualDoc.ts`) from the embedded code, with a length-preserving source map (`src/sourceMap.ts`);
2. forwards **completion/hover/definition** to **Godot's built-in LSP over raw TCP :6005** via `src/godotProxy.ts` (`GodotProxy`: `ensureConnected`→`initialize`→`didOpen`→`completion`/`hover`), **degrading to `null` when no editor runs**;
3. ships a static `classdb/godot-control.json` (`src/classdb.ts`) for `Control` completion *because the proxy needs a live editor*.

**Pain removed:** requires a running editor on :6005; quality capped by the engine's gradual inference; `classdb.ts` is a parallel hand-maintained data path.

### 2.2 The Volar-style embedded adapter

The library stays **guitkx-agnostic** — it is a pure GDScript analyzer. Embedding is a *client* concern: the guitkx LSP builds a synthetic GDScript document + a source map and feeds it through our **unchanged** offset-based API ([`research/06`](research/06-analyzer-architecture.md) §5). The adapter lives in the **guitkx repo**, consuming `@gdscript-analyzer/core`.

```ts
// ide-extensions/lsp-server/src/analyzerAdapter.ts  (Volar-style — replaces godotProxy.ts)
import { AnalysisHandle } from "@gdscript-analyzer/core";   // napi-rs v3 .node addon

// A Volar Mapping: parallel arrays mapping .guitkx spans <-> synthetic .gd spans.
interface Mapping {
  sourceOffsets: number[];     // byte offsets in the .guitkx document
  generatedOffsets: number[];  // byte offsets in the synthetic .gd document
  lengths: number[];           // segment lengths (generatedLengths defaults to these)
  data: { completion: boolean; hover: boolean; navigation: boolean; verification: boolean };
}

const host = new AnalysisHandle();                 // holds the Rust AnalysisHost (cache survives edits)

function update(uri: string, doc: GuitkxDoc) {
  const { syntheticGd, mappings } = buildEmbedded(doc);   // existing virtualDoc.ts + sourceMap.ts, Volar-shaped
  host.applyChange({ open: { uri: `${uri}/embedded.gd`, text: syntheticGd } });  // register as a FileId; no fs
  return mappings;
}

function completion(uri: string, guitkxOffset: number, mappings: Mapping[]) {
  const genOffset = toGenerated(mappings, guitkxOffset);   // .guitkx -> synthetic .gd offset
  if (genOffset == null) return null;                      // outside any {expr} span
  const items = host.completions(`${uri}/embedded.gd`, genOffset);  // OUR analyzer, byte offset
  return items.map(it => ({ ...it, textEdit: mapEditBack(mappings, it.textEdit) })); // .gd -> .guitkx
}
// hover/definition: identical shape — map offset in, query, map result ranges back.
```

`toGenerated`/`toSource` binary-search the offset arrays (exactly `@volar/source-map`'s `SourceMap.toGeneratedLocation`/`toSourceLocation`). `data` flags (Volar `CodeInformation`) let an injected synthetic prologue be marked non-navigable while `{expr}` spans opt into completion+hover+diagnostics.

### 2.3 Migration steps

1. **Add** `@gdscript-analyzer/core` as a dependency of the guitkx lsp-server; instantiate one `AnalysisHandle`.
2. **Write** `analyzerAdapter.ts` (above) — reuse `virtualDoc.ts`/`sourceMap.ts` unchanged (only reshape mappings to the Volar `{sourceOffsets, generatedOffsets, lengths}` form if not already).
3. **Reroute** the lsp-server's completion/hover/definition handlers from `GodotProxy` → `analyzerAdapter`.
4. **Delete** `godotProxy.ts` and the :6005 connection logic.
5. **Delete** `classdb.ts` + `classdb/godot-control.json` — `Control`/`Button`/etc. members now come from the analyzer's engine model (the same `extension_api.json` data, in-process and inheritance-flattened).
6. **Add** diagnostics: push `host.diagnostics(embedded.gd)` mapped back to `.guitkx` ranges (the proxy never offered this).
7. **Add** the guitkx integration smoke test (§Testing) to the guitkx repo CI.

### 2.4 What stays / what goes

| Stays | Goes |
|---|---|
| `virtualDoc.ts` (synthetic `.gd` builder) | `godotProxy.ts` (the whole TCP :6005 proxy) |
| `sourceMap.ts` (offset map; reshaped to Volar `Mapping`) | `classdb.ts` + `classdb/godot-control.json` (parallel data path) |
| The lsp-server's editor-facing handlers | The "requires a running Godot editor" dependency |
| `analyzerAdapter.ts` (**new**) | — |

### 2.5 Acceptance criteria

- **No Godot editor needed:** completion/hover/definition/diagnostics for embedded GDScript work with **nothing on :6005** (CI + headless dev).
- **Parity-or-better:** for the same `.guitkx` fixtures, the analyzer returns **≥** the member set the proxy returned (e.g. the full `V.*` set + `button.` → `Button`/`Control`/`Node` chain), **plus** inferred-type hover and **diagnostics the proxy never had**.
- **Within the perf budget** (§Testing) on real `.guitkx` files.
- **`godotProxy.ts` + `classdb.ts` removed** from the tree.

---

## Workstream 3 — `gdscript-cli`

A native `clap` binary for local dev + CI. **The CLI is where real `std::fs` lives** ([`01`](01-ARCHITECTURE.md) §7) — it reads files from disk and pushes their text into the host via `apply_change`; the **core never touches the filesystem**. Distributed via cargo-dist ([`research/01`](research/01-rust-distribution-tooling.md) §7.3).

### 3.1 Commands

| Command | Does | `Analysis` query |
|---|---|---|
| `gdscript check [paths…]` | Type + parse diagnostics across the project (the CI workhorse). | `diagnostics(file)` ∀ file |
| `gdscript lint [paths…]` | Diagnostics filtered to the warning/lint subset (Phase 6 gating respected). | `diagnostics` + gating filter |
| `gdscript format [paths…] [--check] [--write]` | Format in place / check formatting. Phase 6 formatter behind it. | `format(file)` |
| `gdscript symbols [paths…]` | Dump document/workspace symbols (JSON for tooling/AI agents). | `document_symbols` / `workspace_symbols` |

### 3.2 Output formats

- **`--format human`** (default): colored, `path:line:col: severity[CODE] message`, source snippet with a caret — gdtoolkit/Ruff-style.
- **`--format json`**: a stable JSON array of POD diagnostics (the serde `Diagnostic`, byte ranges *and* line/col) for programmatic consumers.
- **`--format github`** (CI-friendly): GitHub Actions workflow-command annotations (`::error file=…,line=…,col=…::message`), so `check`/`lint` annotate PRs inline. (Optional companions: SARIF for code-scanning, `--format rdjson` for reviewdog.)

### 3.3 Exit codes

| Code | Meaning |
|---|---|
| `0` | Clean (no diagnostics at/above the fail threshold; `format --check` found nothing to change). |
| `1` | Diagnostics found (lint/check) **or** files would be reformatted (`format --check`). |
| `2` | Usage error (bad args/config). |
| `>2` | Internal error (panic caught, I/O failure). |

`--error-on-warning` raises warnings to the failure threshold for strict CI.

### 3.4 Config discovery

Walk up from each target path to the project root: locate `project.godot` (the Godot project marker → version detection, autoloads; the Phase 3 project model) and an optional analyzer config (`gdscript-analyzer.toml` / a `[tool]` table) for warning gating + format options. `--config <path>` overrides; `--no-config` ignores discovery.

### 3.5 gdformat-compatibility note

`format` shares the **same formatter** as the LSP `formatting` path and Phase 6 ([`PHASE-6`](PHASE-6-V1-RELEASE.md)) — one `Analysis::format` implementation, three surfaces (CLI, LSP, playground). The Phase-6 goal is **gdformat-compatible-or-better** output ([`research/05`](research/05-prior-art-and-landscape.md) §2: gdtoolkit's `gdformat` is the incumbent). In Phase 5 the command + plumbing + `--check`/`--write` UX ship; the formatting engine may be a passthrough until Phase 6 fills it.

---

## Workstream 4 — The WASM web playground

Paste GDScript → live diagnostics/hover/completion in the browser, **no server, no Godot, no install** — the single most legible proof of "reach" ([`research/08`](research/08-wasm-web-and-bindings.md) §2; the gap [`research/05`](research/05-prior-art-and-landscape.md) §8 names: every existing GDScript playground needs in-browser parse/diagnostics that **only WASM** provides).

### 4.1 Stack & editor recommendation

**Vite SPA + the WASM analyzer, calling exports directly (NO LSP-over-WASM)** — the validated Biome/Ruff/oxc topology ([`research/08`](research/08-wasm-web-and-bindings.md) §3.5: for a single-language playground, JSON-RPC is pure overhead).

**Editor: recommend Monaco, with a CodeMirror 6 fallback if bundle weight bites.** The tradeoff ([`research/08`](research/08-wasm-web-and-bindings.md) §3.4): Monaco is **~2–5 MB** (batteries-included, the VS Code feel; used by Ruff/swc/new-oxc); CM6 is **~50–300 KB** (a ~6 MB → 3.4 MB win in Sourcegraph's swap; used by Biome). **Recommendation: Monaco**, because (a) **Ruff is our cleanest template** (React + Vite + Monaco + a wasm-bindgen crate — [`research/08`](research/08-wasm-web-and-bindings.md) §2) and copying it de-risks the build, and (b) our wasm blob is single-digit MB regardless, so Monaco's weight is a minority of total page bytes. **If** the measured total bundle is unacceptable, switch to CM6 — both consume the **exact same** plain WASM API ([`research/08`](research/08-wasm-web-and-bindings.md) §3.4), so the editor is swappable without touching the analyzer glue.

### 4.2 Providers call WASM directly

Diagnostics, hover, and completion are just editor callbacks that call the WASM exports synchronously (optionally in a Web Worker to keep the UI responsive — a perf choice, not an LSP requirement, [`research/08`](research/08-wasm-web-and-bindings.md) §3.5).

```ts
// playground/src/analyzer.ts + monaco wiring  (sketch)
import init, { Analyzer } from "@gdscript-analyzer/wasm";
await init();                                          // loads + compiles the .wasm (import.meta)
await Analyzer.loadEngineApi("/data/extension_api.4.7.rkyv.br"); // §4.4 — fetched, brotli, lazy
const az = new Analyzer(PositionEncoding.UTF16);       // Ruff-style: analyzer emits UTF-16 directly

monaco.languages.registerCompletionItemProvider("gdscript", {
  triggerCharacters: [".", ":", "@", "$"],
  provideCompletionItems(model, position) {
    const offset = model.getOffsetAt(position);        // Monaco offset (UTF-16 code units)
    const items = az.completions(model.getValue(), offset);   // WASM export — returns UTF-16 ranges
    return { suggestions: items.map(toMonaco) };
  },
});
monaco.languages.registerHoverProvider("gdscript", {
  provideHover(model, position) {
    const h = az.hover(model.getValue(), model.getOffsetAt(position));
    return h && { range: toRange(h.range), contents: [{ value: h.markdown }] };
  },
});
// Diagnostics are PUSHED (not a provider): on change, setModelMarkers.
editor.onDidChangeModelContent(() => {
  const diags = az.diagnostics(model.getValue());      // UTF-16 line/col already
  monaco.editor.setModelMarkers(model, "gdscript-analyzer", diags.map(toMarker));
});
```

### 4.3 UTF-16 position handling

**The single most common WASM pitfall** ([`research/08`](research/08-wasm-web-and-bindings.md) §3.3): a Rust analyzer reports UTF-8 byte offsets; both editors want UTF-16. **Follow Ruff** — construct the analyzer with `PositionEncoding.UTF16` so it converts byte ranges → UTF-16 **inside the WASM** (reusing `gdscript-base::LineIndex`) and the JS never does offset math. (Same converter the LSP uses, [`01`](01-ARCHITECTURE.md) §4.)

### 4.4 The `extension_api.json` data-loading strategy

The engine model is several MB; **never `include_bytes!` it into the `.wasm`** ([`01`](01-ARCHITECTURE.md) §5; [`research/08`](research/08-wasm-web-and-bindings.md) §5). At build time: prune the no-docs dump → **rkyv** (zero-copy) → **brotli** → emit `extension_api.<version>.rkyv.br` as a **separate, content-hashed, immutable** asset. The playground **`fetch`es** it lazily (in parallel with wasm instantiation), brotli-decodes, and hands bytes to the analyzer for zero-copy access (`Analyzer.loadEngineApi`). **~hundreds of KB over the wire**, near-zero parse cost, and the wasm code-cache stays stable across data refreshes (the V8 wasm-caching win). The doc store loads lazily by `DocId` (hover is cold-path).

### 4.5 Bundle-size budget & techniques

Target: a **single-digit-MB** `.wasm` (in line with Ruff's 10.8 MB; far below Biome's 37 MB — we ship a parser+analyzer, not a full formatter+toolchain) ([`research/08`](research/08-wasm-web-and-bindings.md) §4.5). Techniques ([`research/08`](research/08-wasm-web-and-bindings.md) §4):
- **Cargo release profile:** `opt-level = "z"` (measure `"s"` too — sometimes smaller), `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
- **`wasm-opt -Oz`** (Binaryen) — another ~15–20% (wasm-pack runs it automatically on release).
- **`twiggy top`/`dominators`/`monos`** to find bloat (an analyzer is monomorphization-heavy — watch `monos`); `twiggy diff` as a CI size-regression guard.
- **Allocator:** keep default `dlmalloc`; **avoid `wee_alloc`** (unmaintained, RUSTSEC-2022-0054); evaluate `talc` only if measured.
- `console_error_panic_hook` in the wasm crate so panics reach `console.error`.

### 4.6 Deploy

GitHub Pages (static host — Vite `--target web` build, no server). The data asset served from the same Pages origin (or a CDN) with `Content-Encoding: br`, immutable cache headers. CI builds + deploys on release. (No COOP/COEP needed if we ship single-threaded wasm-bindgen — the playground runs single-threaded anyway, [`research/08`](research/08-wasm-web-and-bindings.md) §6.1.)

---

## Workstream 5 — Distribution GA (crates.io + npm)

The validated **oxc/swc topology** ([`research/01`](research/01-rust-distribution-tooling.md) §1.3, §1.2): one core, thin per-target wrappers, per-platform npm packages with a tiny loader.

### 5.1 The napi per-platform packaging

A **main npm package** = JS-only wrapper (the `binding.js` loader + `.d.ts`) declaring each platform package as an `optionalDependency`; **per-platform packages** = one `.node` each, named `@gdscript-analyzer/core-<triple>`, each carrying its own `os`/`cpu` so npm installs only the match ([`research/01`](research/01-rust-distribution-tooling.md) §2.3).

```jsonc
// bindings/node/package.json  (the main package — sketch)
{
  "name": "@gdscript-analyzer/core",
  "version": "0.X.0",
  "main": "binding.js",
  "types": "index.d.ts",
  "napi": { "binaryName": "gdscript", "targets": [
    "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc",
    "x86_64-apple-darwin", "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
    "armv7-unknown-linux-gnueabihf", "wasm32-wasip1-threads"
  ] },
  "optionalDependencies": {
    "@gdscript-analyzer/core-win32-x64-msvc":   "0.X.0",
    "@gdscript-analyzer/core-win32-arm64-msvc": "0.X.0",
    "@gdscript-analyzer/core-darwin-x64":       "0.X.0",
    "@gdscript-analyzer/core-darwin-arm64":     "0.X.0",
    "@gdscript-analyzer/core-linux-x64-gnu":    "0.X.0",
    "@gdscript-analyzer/core-linux-arm64-gnu":  "0.X.0",
    "@gdscript-analyzer/core-linux-x64-musl":   "0.X.0",
    "@gdscript-analyzer/core-linux-arm64-musl": "0.X.0",
    "@gdscript-analyzer/core-linux-arm-gnueabihf": "0.X.0",
    "@gdscript-analyzer/core-wasm32-wasi":      "0.X.0"
  }
}
```

**The loader** (`binding.js`, swc's reference idiom — [`research/01`](research/01-rust-distribution-tooling.md) §1.2): an `isMusl()` helper (three strategies: `process.report.header.glibcVersionRuntime` absence, `ldd --version`, `/usr/bin/ldd` read) + a `process.platform × process.arch` switch that tries (1) a local `./gdscript.<triple>.node` (dev), (2) `require('@gdscript-analyzer/core-<triple>')`, (3) the `@gdscript-analyzer/core-wasm32-wasi` WASI fallback, throwing `Failed to load native binding` on total failure. **The `@napi-rs/cli` generates this** — we don't hand-write it.

**Build/publish flow** ([`research/01`](research/01-rust-distribution-tooling.md) §2.4–2.5): the cross-compile CI matrix (per §5.4) builds each `.node`; a publish job runs `napi artifacts` (organize the downloaded `.node`s into `npm/<triple>/`) → `napi prepublish -t npm` (patch the main package's `optionalDependencies`, publish the sub-packages) → `npm publish --provenance`.

### 5.2 The wasm package

`@gdscript-analyzer/wasm` — the browser build the playground consumes. **Default route:** a dedicated wasm-bindgen crate (`wasm-pack build --target web`) for the **smallest artifact and no SharedArrayBuffer/COOP-COEP requirement** ([`01`](01-ARCHITECTURE.md) §4; [`research/01`](research/01-rust-distribution-tooling.md) §4.3 route B — Biome/Ruff's path), since playground bundle size matters and the playground is single-threaded anyway. (The napi-rs v3 `wasm32-wasip1-threads` artifact also exists as `@gdscript-analyzer/core-wasm32-wasi` from the single binding; we pick the smaller per measured size, [`01`](01-ARCHITECTURE.md) §4.) Ships the byte→UTF-16 converter in its glue.

### 5.3 crates.io publishing

- **`gdscript-ide` is the public Rust API** ([`01`](01-ARCHITECTURE.md) §1) — the crate external Rust consumers depend on (`cargo add gdscript-ide`). Its dependencies (`gdscript-base`/`-syntax`/`-api`/`-db`/`-hir`) publish alongside it; `gdscript-ffi`/`-lsp`/`-cli` are end-tools (mark binding/tool crates `publish = false` or publish the binaries via cargo-dist, mirroring oxc's split — [`research/01`](research/01-rust-distribution-tooling.md) §1.3).
- **release-plz** ([`research/01`](research/01-rust-distribution-tooling.md) §7.3): opens/maintains a Release PR (SemVer via conventional commits + cargo-semver-checks, changelog via git-cliff); on merge, tags `v<version>`, runs `cargo publish` for the workspace, cuts a GitHub release. That tag then **fires** the napi/wasm publish workflows + cargo-dist.
- **Name availability:** verify `gdscript-*` on crates.io; fall back to `gdscript-analyzer-<layer>` (the rust-analyzer `ra_ap_*` precedent) if taken ([`01`](01-ARCHITECTURE.md) §1 naming note). Pre-GA early access can use the `ra_ap_*`-style auto-publish trick.

### 5.4 Single shared version + lockstep + provenance

- **One shared version** across all crates + all npm packages ([`01`](01-ARCHITECTURE.md) §9: "single shared version (crates+npm)"). Every per-platform sub-package, the main napi package, the wasm package, and the crates pin to the **same** `0.X.0`. release-plz cuts the tag → that one tag drives every publish workflow → no version skew between a main package and its `optionalDependencies`.
- **cargo-dist** ([`research/01`](research/01-rust-distribution-tooling.md) §7.3) builds the CLI + LSP cross-platform binaries + installers (shell/PowerShell/MSI/Homebrew) on the same tag.
- **npm provenance** (`--provenance`) on every npm publish for supply-chain attestation.

### 5.5 The cross-compile matrix (CI)

`windows-latest` → `x86_64`/`aarch64-pc-windows-msvc`; `macos-latest` → `x86_64`/`aarch64-apple-darwin`; `ubuntu-latest` → `x86_64`/`aarch64-unknown-linux-gnu` (`--use-napi-cross`), `x86_64`/`aarch64-unknown-linux-musl` (`-x`, cargo-zigbuild), `armv7-unknown-linux-gnueabihf`, plus `wasm32-wasip1-threads`. Each job uploads `bindings-<triple>`; the publish job downloads all → `napi artifacts` → `npm publish --provenance` ([`research/01`](research/01-rust-distribution-tooling.md) §2.5).

### 5.6 Published artifacts

| Name | Registry | Contents | Consumers |
|---|---|---|---|
| `gdscript-ide` (+ `-base`/`-syntax`/`-api`/`-db`/`-hir`) | crates.io | The public Rust analysis API + its layers | Rust tools, alternative editors, other Rust analyzers |
| `@gdscript-analyzer/core` | npm | JS loader + `.d.ts`; `optionalDependencies` → per-platform `.node` | guitkx lsp-server, `gdscript-lsp` (if Node-hosted), Node tooling |
| `@gdscript-analyzer/core-<triple>` (×~9) | npm | one prebuilt `.node` per platform (incl. `-wasm32-wasi`) | resolved transitively by the loader |
| `@gdscript-analyzer/wasm` | npm | `wasm-pack --target web` `.wasm` + JS glue + UTF-16 converter | the web playground, browser IDEs |
| `gdscript-lsp` (binary) | cargo-dist (GitHub Releases) | standalone LSP server per OS/arch + installers | VS Code/Neovim/Helix/Zed/Emacs users |
| `gdscript-cli` (binary `gdscript`) | cargo-dist (GitHub Releases) | CLI per OS/arch + installers (shell/PS/MSI/Homebrew) | CI, local dev, AI/agent tooling |
| `extension_api.<ver>.rkyv.br` | GitHub Pages / CDN | pruned→rkyv→brotli engine model, content-hashed | the playground (lazy fetch) |

---

## Testing strategy

1. **LSP conformance tests** (`crates/gdscript-lsp`): a JSON-RPC harness drives `initialize`→requests→`shutdown`; assert each method's **UTF-16 mapping is exact** (golden: source with multi-byte chars — emoji/accents — and asserted UTF-16 ranges), capabilities advertise correctly, cancellation returns `ContentModified`. Smoke-test against a real client (VS Code extension host + a Neovim headless `nvim --headless` session) confirming hover/completion/rename round-trip **with no Godot running**.
2. **guitkx integration smoke test** (in the **guitkx repo** CI): mirror the old `scripts/live-full.js` but pointed at the adapter — take a real `.guitkx` fixture → `virtualDoc` → synthetic `.gd` + Volar mappings → `AnalysisHandle.completions`/`hover`/`diagnostics`; assert **≥** the proxy's member set + inferred-type hover + diagnostics, **without** :6005. Gated on the guitkx repo being present (skip if absent).
3. **CLI golden output** (`crates/gdscript-cli`): fixture projects → assert stdout for `--format human`/`json`/`github` (`insta` snapshots) + **exit codes** (clean=0, diagnostics=1, usage=2). `format --check` round-trip on the formatter fixtures.
4. **Playground e2e** (Playwright/`@playwright/test` against the built Vite app): paste a snippet → assert markers appear; trigger `.` → assert completion list; hover → assert tooltip; confirm the data asset is **fetched** (not embedded) and the wasm loads. Headless-Chromium in CI.
5. **Per-platform install tests:** in the npm matrix, on each runner `npm i @gdscript-analyzer/core` in a clean dir and `require()` it → assert the correct `.node` loaded (incl. the **musl** path on Alpine and the **WASI fallback** when no native match); `cargo add gdscript-ide` + a trivial `Analysis` call compiles + runs.
6. **Cross-target CI:** the §5.5 matrix builds every triple; `cargo check -p gdscript-ide --target wasm32-unknown-unknown` stays green ([`01`](01-ARCHITECTURE.md) §7); `twiggy diff` guards the wasm bundle against size regressions; a release **dry-run** (release-plz + napi prepublish `--dry-run` + cargo-dist plan) passes before any GA tag.

---

## Exit criteria (mirrors ROADMAP Phase 5)

A testable checklist:

- [ ] **Standalone LSP works with no Godot:** `gdscript-lsp` over stdio gives completion, hover, **semantic tokens, inlay hints, workspace symbols, rename, find-references** in VS Code **and** Neovim, with **nothing on :6005**.
- [ ] **guitkx ships proxy-free:** `godotProxy.ts` + `classdb.ts` deleted; the lsp-server answers embedded GDScript via `@gdscript-analyzer/core` with **parity-or-better** vs the old proxy + diagnostics, **no editor running**; the guitkx smoke test is green.
- [ ] **`npm i @gdscript-analyzer/core` works** on Windows/macOS/Linux (gnu **and** musl), resolving the right per-platform `.node` (WASI fallback when no native match).
- [ ] **`cargo add gdscript-ide` works** — an external Rust consumer builds against the public API and runs a query.
- [ ] **The playground analyzes pasted GDScript in-browser** — diagnostics/hover/completion via the WASM analyzer, the engine model **fetched** as a separate asset, deployed live on GitHub Pages.
- [ ] **CLI:** `gdscript check`/`lint`/`format`/`symbols` run on a real project with correct exit codes + JSON/GitHub output.
- [ ] **GA gate honored:** publish happens only after `gdscript-ide` is semver-stable; all artifacts share one version; npm provenance attached.

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| **API churn breaks consumers before GA.** Publishing `gdscript-ide` then breaking it churns every client (LSP/guitkx/CLI/playground all pin one version). | **Gate GA on Phase-2 API semver-stability** (the explicit ROADMAP rule). Dogfood all four clients **pre-GA** against the unpublished workspace (or `ra_ap_*`-style early access, [`research/01`](research/01-rust-distribution-tooling.md) §1.5). `cargo-semver-checks` in release-plz blocks accidental breaks. |
| **napi cross-compile matrix flakiness** (gnu/musl split doubles Linux; cross toolchains fragile — [`research/01`](research/01-rust-distribution-tooling.md) §2.6). | Copy oxc-resolver's proven `release-napi.yml` verbatim (cargo-zigbuild `-x` for musl, `--use-napi-cross` for glibc). Per-platform **install tests** (§Testing 5) catch a bad/missing `.node` before users do; the **WASI fallback** keeps install working even if one triple fails to build. |
| **WASM bundle too big** (rust-analyzer's cautionary multi-MB tale — [`research/08`](research/08-wasm-web-and-bindings.md) §2). | We ship a **parser+analyzer, not a full toolchain** (target single-digit MB like Ruff's 10.8, far below Biome's 37). `opt-level=z`+`lto`+`wasm-opt -Oz`; `twiggy` to hunt bloat + a **CI size-regression guard**; ship the engine model as a **separate fetched asset**, never `include_bytes!`. |
| **guitkx parity gaps** (analyzer returns less than the proxy for some embedded position). | The Phase-2 validation already proved single-file parity; treat any gap as a **fixture** in the guitkx smoke test, not a silent regression. On a `Variant`/`Unknown` receiver, the analyzer falls back to by-name completion ([`PHASE-2`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) §6.2) so it never drops **below** the proxy. |
| **Monaco bundle weight** dominates the playground page. | Copy Ruff's Monaco-on-Vite stack (de-risked); **but** the editor is swappable — both Monaco and CM6 consume the **identical** WASM API ([`research/08`](research/08-wasm-web-and-bindings.md) §3.4), so a CM6 swap (Sourcegraph's −43%) is a localized change if the measured bundle is unacceptable. |
| **LSP rename correctness is inherited from Phase 3.** A wrong cross-file `WorkspaceEdit` is a data-loss-class bug. | The server maps `SourceChange`→`WorkspaceEdit` faithfully but does **not** invent edits; correctness is Phase 3's (`find_references`/`rename` engine). Advertise `rename` only once Phase 3's rename passes its cross-file tests; `prepareRename` validates the symbol before any edit; LSP conformance tests assert the edit set matches Phase 3 golden `SourceChange`s. |
| **`positionEncoding` mismatch** (a client that only does UTF-8/UTF-32). | Negotiate in `initialize`; default UTF-16, honor a UTF-8 negotiation; the conversion is centralized in `LineIndex` so one code path serves all encodings. Conformance tests cover both. |

---

## References (relative links)

- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — client crates (§1), the `AnalysisHost`/`Analysis` POD API (§2), FFI/WASM strategy (§4), data shipping (§5), portability rules (§7), cross-cutting decisions (§9).
- [`ROADMAP.md`](ROADMAP.md) — Phase 5 deliverable + exit criteria; the dependency graph (Phase 5 parallelizable from Phase 2; GA waits for API stability).
- [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) — the distribution tooling/CI + the napi/wasm bindings + the engine-model codegen this phase packages.
- [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) — the API surface Phase 5 maps; the guitkx first-client validation seam this phase builds into the full adapter.
- [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) — fills `resolve_external`; provides the correct rename / find-references / workspace-symbols the LSP advertises.
- [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) — sharpens node-path types the LSP/playground then surface.
- [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) — the formatter behind `cli format`/LSP `formatting`; the full warning set + gating the clients surface.
- [`research/01-rust-distribution-tooling.md`](research/01-rust-distribution-tooling.md) — **PRIMARY**: napi per-platform npm packaging, the `binding.js` loader + `isMusl`, the cross-compile matrix, crates.io/release-plz/cargo-dist.
- [`research/08-wasm-web-and-bindings.md`](research/08-wasm-web-and-bindings.md) — **PRIMARY**: the playground stack, Monaco vs CodeMirror, direct-WASM (no LSP-over-WASM), UTF-16, data shipping, bundle-size budget.
- [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) — the LSP server as a thin client; the Volar embedded-language adapter (`Mapping`/`CodeInformation`/source map).
- [`research/05-prior-art-and-landscape.md`](research/05-prior-art-and-landscape.md) — the engine-LSP gaps we beat (rename/refs/workspace-symbols/semantic-tokens/standalone); the demand evidence; the Ruff/rust-analyzer templates.
