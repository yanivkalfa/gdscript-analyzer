# Phase 5 · Workstream 1 — `gdscript-lsp` Playbook (the standalone LSP server)

> Research-backed build plan for the standalone, spec-compliant LSP server that wraps the existing
> `gdscript-ide` `AnalysisHost`/`Analysis` API. Mirrors the Phase-4 M0 playbook format. Sources are
> 2026-current and adversarially verified (24/24 confirmed, 1 refuted — noted inline).
>
> **Parent docs:** [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md)
> §Workstream 1, [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2 (the API shape this consumes).

---

## 0. The one-line thesis

The hard part is **already done**: `gdscript-ide` gives us a salsa-incremental, cancellable,
single-writer/many-reader analysis engine returning POD. `gdscript-lsp` is a **thin, synchronous
protocol shell** around it — its only real engineering is (a) the **byte-offset(UTF-8) ↔ LSP
(line, UTF-16 char)** conversion and (b) a **rust-analyzer-style main loop** that dispatches reads to
a thread pool and turns salsa cancellation into `ContentModified`. **Do not pull in an async
framework.**

---

## 1. Framework decision — `lsp-server` + `lsp-types` + a hand-rolled loop ✅

**Decision:** use the low-level **`lsp-server`** (transport + `Connection` = a pair of crossbeam
channels) + **`lsp-types`** (the protocol structs) and write our own event loop — exactly what
rust-analyzer does. **Reject `tower-lsp` / `tower-lsp-server` / `async-lsp`.**

Rationale (verified):
- rust-analyzer deliberately uses its *own* `lsp-server`, not tower-lsp; "crates/rust-analyzer is the
  only crate that knows about LSP and JSON serialization." [RA architecture]
- `async-lsp`'s own README critique of tower-lsp: it "doesn't accept `&mut self` for notifications,"
  forcing interior locks — a poor fit for a single mutable `AnalysisHost`. [async-lsp]
- Our engine is **synchronous + cancellation-via-unwind** (salsa global revision counter). An async
  runtime adds an impedance mismatch (futures vs. a blocking `salsa::Cancelled::catch`) for zero gain
  — we don't need async I/O; stdio framing is trivial and `lsp-server` already does it.
- `lsp-server`'s `Connection { sender, receiver }` is just channels → trivial **in-process test
  harness** (two connected `Connection`s, no subprocess). [lsp_server::Connection]

Tradeoff accepted: we hand-write the dispatch loop (≈150 lines) instead of getting handler
registration from a framework. That control is the point — it's where cancellation + latency tagging
live.

Crates: `lsp-server`, `lsp-types` (3.17), `crossbeam-channel`, `serde_json`, plus our `gdscript-ide`.
Optionally reuse the published **`line_index` (0.1.2)** crate for offset↔position, or vendor a tiny
equivalent (we already have line knowledge in the parser — decide in M0).

---

## 2. Architecture (the rust-analyzer shape, shrunk)

```
            ┌──────────────────────────── main thread ────────────────────────────┐
 stdio ──►  │  Connection (lsp-server)  ─►  main loop (crossbeam select!)          │
            │     • reads LSP messages                                             │
            │     • applies edits to  GlobalState.host: AnalysisHost  (WRITE)      │
            │     • dispatches READ requests to the thread pool with a snapshot    │
            │     • writes responses/notifications back                            │
            └───────────────┬──────────────────────────────────────────┬──────────┘
                            │ snapshot() = host.analysis() (cheap)      │ results
                       ┌────▼─────────────── thread pool ───────────────▼────┐
                       │  read handlers run Analysis queries (Cancellable)   │
                       │  Worker | LatencySensitive intent                   │
                       └─────────────────────────────────────────────────────┘
```

- **`GlobalState`** (main-thread only) owns `host: AnalysisHost`, the **VFS/URI↔FileId interner**,
  the negotiated `PositionEncoding`, client capabilities, and the per-doc `LineIndex` cache. RA's
  `GlobalState.snapshot()` builds a `GlobalStateSnapshot { analysis: host.analysis(), Arc<config>,
  Arc<vfs> }`; we mirror that. [RA global_state]
- **Writes** (didOpen/didChange/didClose/config) mutate `host` on the main thread only — single
  writer. Each write **bumps the salsa revision**, cancelling in-flight reads.
- **Reads** are dispatched to a thread pool, each holding a cloned `Analysis` snapshot + the
  `LineIndex` for the target doc. Tag intent **`LatencySensitive`** (typing-driven: diagnostics,
  semantic tokens, completion) vs **`Worker`** (goto-def, references, workspace symbols). [RA thread]

---

## 3. Position encoding — THE load-bearing correctness work ⚠️

Every range-bearing POD we return is **byte offsets (u32) over UTF-8**; LSP `Position` is
`(line, character)` where `character` is **UTF-16 code units by default**. Get this wrong and every
range is subtly off on any line with a non-ASCII char.

Plan:
1. **Negotiate** in `initialize`: read `client.general.positionEncodings`; prefer `"utf-8"` if
   offered, else fall back to **`"utf-16"`** (mandatory baseline); echo the chosen kind in our
   `ServerCapabilities.positionEncoding`. Model it as `enum PositionEncoding { Utf8, Wide(Utf16|Utf32) }`
   (RA's exact shape). [LSP 3.17 spec; RA PR #17003]
2. **Per-document `LineIndex`**: built once per text version, maps `u32` byte offset ↔ `(line, col)`.
   For `Utf8`, `col` is the raw byte column; for `Wide`, convert via a `to_wide`/`to_utf16` step that
   walks the line counting UTF-16 code units (astral chars = surrogate pair = **2** units). Reuse the
   `line_index` crate or vendor it. [RA line_index.rs; line_index 0.1.2]
3. **Both directions**: incoming LSP positions → byte offset (for `FilePosition`), outgoing byte
   ranges → LSP positions (for every result). One `to_proto`/`from_proto` module, the *only* place
   that knows about encoding — like RA's `to_proto.rs`.
4. **Refuted optimization (do NOT do):** tracking "which lines have non-ASCII via a HashSet" is *not*
   how RA does it (claim killed 0-3). Just build a flat line-start table; convert on demand.
5. **Mandatory tests**: multi-byte (Korean `된장` = 3 UTF-8 bytes each), astral/emoji (surrogate
   pairs), CRLF lines, trailing-newline edge. The cited real bug returned `10..14` instead of `8..14`
   on 3-byte chars under `utf-8` — our golden tests must include it. [helix #5894; RA #202]

**This is the single biggest correctness risk in the whole workstream — build and test the
`LineIndex` first, before any feature.**

---

## 4. Transport & lifecycle

- **stdio** with `Content-Length` framing — `lsp-server` handles it.
- Handshake: `initialize` (first request; respond before anything else, error `-32002` if a request
  precedes it) → `initialized` → work → `shutdown` → `exit`. [LSP 3.17]
- **Capabilities are data-driven**: advertise only what's wired. Ship M0 caps first
  (textDocumentSync, diagnostics), light up the rest per milestone. A capability we don't back yet is
  simply absent from `ServerCapabilities`. Prefer **static** registration (in the `initialize`
  result); reserve dynamic `client/registerCapability` for later file-watcher globs.

---

## 5. Text sync & the VFS

- **`textDocumentSync = Incremental`.** Maintain an in-memory overlay keyed by URI; apply each
  `didChange` content change (range + text) to rebuild the doc text, then rebuild its `LineIndex` and
  call `host.apply_change(Change::change_file(file_id, new_text))`.
- **URI ↔ FileId** via a `PathInterner` (RA's Vfs pattern): `file_id(uri)` interns on first
  `didOpen`/scan; `file_path(id)` for reverse mapping in results. `res://` paths feed
  `Change::set_file_path` (drives Phase-3 `preload`/`extends` + Phase-4 scene resolution). [RA vfs]
- `didOpen`→ intern + `change_file`; `didChange`→ patch + `change_file`; `didClose`→ drop the overlay
  (keep the FileId; the on-disk text remains the source).
- **`didChangeWatchedFiles`** for `.gd`/`.tscn`/`project.godot` changed outside the editor → the same
  `apply_change`. Register the glob via dynamic registration once basic sync is solid.

---

## 6. Concurrency & cancellation (the minimal correct loop)

- One **`crossbeam select!`** over: the LSP receiver, the thread-pool result channel, (later) the
  debounce timer. [RA main_loop]
- On a **request**: snapshot `host.analysis()`, spawn the handler on the pool with the snapshot +
  `LineIndex`. The handler runs the `Analysis` query inside `salsa::Cancelled::catch` (already done
  for us — methods return `Cancellable<T>`).
- On an **edit**: apply to `host` on the main thread → bumps revision → in-flight reads unwind to
  `Err(Cancelled)`. The handler maps that to LSP error **`ContentModified (-32801)`**; a well-behaved
  client re-requests. [salsa cancellation; RA dispatch]
- Honor **`$/cancelRequest`**: track in-flight request ids; on cancel, drop the result (the salsa
  unwind handles the compute side). No need to forcibly kill the thread.
- **No head-of-line blocking**: reads never run on the main thread; the main thread only does cheap
  writes + dispatch + I/O.

---

## 7. Feature → LSP mapping (every `Analysis` method)

All ranges below go through the §3 `to_proto` converter. (★ = beats the engine LSP.)

| LSP request | `Analysis` method | Notes / gotchas |
|---|---|---|
| `publishDiagnostics` (push) | `diagnostics(file)` | **Push + debounce** (≈150–300 ms coalesce on a keystroke flurry), like gopls. Map `Severity`→`DiagnosticSeverity`, `code`→`code`, `fixes`→related code actions. |
| `textDocument/completion` (+ `completionItem/resolve`) | `completions(pos)` | `CompletionKind`→`CompletionItemKind`; `insert_text` (default = label); trigger chars `.`, `$`, `%`, `/`, `"`. Lazy `detail`/docs via resolve later. |
| `textDocument/hover` | `hover(pos)` | `HoverResult.ty_label` + `doc` (Markdown) → `MarkupContent`. `None`→ no hover (never a placeholder). |
| `textDocument/signatureHelp` | `signature_help(pos)` | `active_signature`/`active_parameter` map directly; trigger `(` and `,`. |
| `textDocument/documentSymbol` | `document_symbols(file)` | Hierarchical `DocumentSymbol` (we already nest). `SymbolKind` map. |
| `textDocument/foldingRange` | `folding_ranges(file)` | Line-based folds. |
| `textDocument/inlayHint` (3.17) ★ | `inlay_hints(file)` | `InlayHint.offset`→position; `kind`→`Type`/`Parameter`. The engine LSP has none. |
| `textDocument/semanticTokens/full` ★ | **NEW query (see §8)** | The 5-int relative encoding + a legend. Headline feature the engine LSP lacks. |
| `textDocument/definition` | `goto_definition(pos)` | `NavTarget`→`LocationLink` (use `focus_range` as `targetSelectionRange`, `full_range` as `targetRange`). Includes **scene goto** (`$Path`→`.tscn`). ★ |
| `textDocument/references` | `find_references(pos)` | `Reference`→`Location[]`. |
| `textDocument/rename` (+ `prepareRename`) ★ | `rename(pos, name)` | `SourceChange`→`WorkspaceEdit` (`documentChanges` if client supports). `RenameError`→ LSP error. `prepareRename` = dry-run for the focus range. Cross-file rename the engine LSP can't do. |
| `workspace/symbol` | `workspace_symbols(query)` | `NavTarget`→`SymbolInformation`/`WorkspaceSymbol`. |
| `textDocument/codeAction` (+ resolve) | `code_actions(pos)` | Map to `CodeAction` with `WorkspaceEdit`; lazy-resolve the edit later. |

---

## 8. The `semantic_tokens` gap (a real prerequisite)

`gdscript-ide` has **no `semantic_tokens` method today**. The LSP can't invent it — it needs a new
**`Analysis::semantic_tokens(file) -> Vec<SemanticToken>`** query in `gdscript-ide` returning
`{ range: TextRange, token_type: u32, modifiers: u32 }` (POD, byte ranges). The server then:
1. Declares a **legend** (token types: `keyword, function, method, variable, parameter, property,
   class, enum, enumMember, string, number, comment, namespace, type, …`; modifiers: `declaration,
   readonly, static, deprecated, …`).
2. Encodes to the **5-integer relative form** `(Δline, Δstart, length, typeIdx, modifierBits)`,
   sorted by position. [LSP 3.17 semantic tokens]
3. Ship **`full`** first; add `full/delta` + `range` only if profiling demands.

The query reuses what we already have (the parse + `infer` results + the scene layer): identifiers
classify by their resolved `Ty`/symbol kind, `$Path`/`%Unique` get a distinct modifier, etc. **Scope
this query as the first task of M2**, not the LSP wiring.

---

## 9. Testing & dogfooding

- **In-process harness**: two connected `lsp-server::Connection`s (memory channels), drive
  initialize→didOpen→request, assert on responses. No subprocess. [lsp_server]
- **Golden tests** for `to_proto`/`from_proto` with the multi-byte/astral/CRLF corpus (§3.5) — the
  highest-value tests in the crate.
- **Capability test**: assert advertised caps match the wired handlers (no over-advertising).
- **Dogfood**: a ~30-line VS Code thin-client extension (just spawns the binary over stdio) + a
  Neovim `vim.lsp.start`/`nvim-lspconfig` snippet. Both live under `ide-extensions~/` or the LSP
  crate's `editors/`.
- **CI**: the harness tests run in `xtask ci`; the binary must build on the standard targets +
  (sanity) not regress wasm for the rest of the workspace.

---

## 10. Distribution shape (brief — full GA is Workstream 5, later)

- A single **stdio binary** `gdscript-lsp`. cargo-dist builds per-platform archives. [cargo-dist]
- The binary **must not assume** a running Godot editor or any network — pure stdin/stdout, reads the
  workspace from disk via the VFS.
- Per-editor clients are thin: VS Code extension spawns it; Neovim via `nvim-lspconfig`; Helix/Zed via
  their generic LSP config. **GA publishing waits** until `gdscript-ide`'s API is semver-stable
  (per the Phase-5 sequencing note) — build + dogfood pre-GA.

---

## 11. Milestones (each ends green through `xtask ci` + the harness)

- **M0 — the spine.** `lsp-server` loop + lifecycle (initialize/shutdown/exit) + **position-encoding
  negotiation + `LineIndex` + `to_proto`/`from_proto` (with the multi-byte golden tests)** +
  incremental text sync + URI↔FileId VFS + **push diagnostics with debounce** + cancellation→
  `ContentModified`. *Exit:* open a file, type, see live diagnostics at correct ranges in VS Code with
  no Godot running.
- **M1 — read features.** hover, completion (+trigger chars), signatureHelp, documentSymbol,
  foldingRange. *Exit:* hover shows inferred types; completion after `.`/`$`.
- **M2 — the headline features.** the new `Analysis::semantic_tokens` query (§8) + `semanticTokens/
  full`, then `inlayHint`. *Exit:* semantic highlighting + `: Type` inlays the engine LSP can't give.
- **M3 — navigation & refactor.** definition (incl. `$Path`→`.tscn`), references, rename
  (+prepareRename), workspace/symbol, codeAction. *Exit:* cross-file rename + scene goto over LSP.
- Per-milestone: an **adversarial bug-hunt** (find→verify→fix) like every prior milestone.

---

## 12. Risks (rated)

| Risk | Sev | Mitigation |
|---|---|---|
| **UTF-16 position bugs** | **Critical** | The §3 single converter + the multi-byte/astral golden corpus, built in M0 before any feature. *The* correctness risk. |
| Diagnostics storms on keystrokes | High | Debounce + coalesce; `LatencySensitive` intent; cancel superseded computes. |
| Cancellation correctness | Med | Lean on the existing `Cancellable`; map to `ContentModified`; in-process tests that edit mid-request. |
| Capability over-advertising | Med | Data-driven caps + the capability test; advertise per milestone. |
| Incremental-sync edge cases (CRLF, multi-edit ordering) | Med | Apply changes in spec order; rebuild `LineIndex` per version; fuzz didChange. |
| Client quirks (VS Code vs Neovim vs Helix encoding) | Med | Honor negotiated encoding strictly; test against ≥2 clients early. |

**Biggest correctness risk:** position encoding (§3). **Biggest leverage point:** the existing
`Analysis` API + `Cancellable` — the analyzer, incrementality, and cancellation are already built;
this workstream is a protocol shell, so M0's spine unlocks every later feature cheaply.

---

## Sources (verified, 2026-current)
- LSP 3.17 spec (Position, `positionEncoding`, semantic tokens 5-int, pull vs push diagnostics, inlay
  hints, lifecycle) — microsoft.github.io/language-server-protocol
- rust-analyzer architecture / `main_loop.rs` / `global_state.rs` / `to_proto.rs` / `line_index.rs` /
  PR #17003 / issue #202 — the canonical hand-rolled-loop + position-encoding reference
- `async-lsp` README (the tower-lsp critique); `tower-lsp-server` (the maintained fork) — framework
  comparison
- `lsp_server::Connection`, `vfs::Vfs` (rust-lang.github.io/rust-analyzer) — transport + URI↔FileId
- `line_index` 0.1.2 — reusable offset↔position
- helix #5894 — encoding negotiation in practice; gopls diagnostics — push+debounce precedent
- cargo-dist; VS Code language-server extension guide — distribution + thin client
- **Refuted:** the "HashSet of non-ASCII lines" line-index optimization is *not* rust-analyzer's
  design (killed 0-3) — use a flat line-start table.
