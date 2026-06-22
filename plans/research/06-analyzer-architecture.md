# 06 — Analyzer Architecture: a "Roslyn for Godot" as a reusable Rust library

**Status:** research note (architecture design)
**Date:** 2026-06-22
**Owner constraint (load-bearing):** `gdscript-analyzer` is a **LIBRARY**, not an LSP server.
The LSP server is *one* client. Others must consume the *same* analysis API: a web
playground (WASM), a CLI linter, CI, the **guitkx** TypeScript tooling, and the
community. The library must be **incremental** (fast re-analysis on edits) and expose
a clean **query/snapshot** API that returns **engine-neutral** data (offsets/ranges/
structured items) — never LSP types.

The template for all of this is **rust-analyzer**, which was explicitly built as a
library stack (`syntax` → `base-db` → `hir-*` → `ide`) with a thin LSP server crate on
top. We copy that separation directly.

---

## 1. rust-analyzer's layered architecture (the template)

rust-analyzer is a layered set of crates; each layer depends only on layers below it,
and the dependency direction flows strictly upward:

```
syntax  →  base-db  →  hir-expand → hir-def → hir-ty  →  hir  →  ide-db → ide  →  rust-analyzer (server)
[lossless]  [salsa     [-------- the "compiler brain" --------]   [feature   [LSP / JSON
 CST/AST]    inputs]                                              layer, POD] over stdio]
```

### Layer 1 — `syntax` (+ `parser`, **rowan**)
- Hand-written recursive-descent parser that emits a flat event stream ("start node X",
  "finish node Y"); it is "independent of the particular tree structure and particular
  representation of the tokens."
- `syntax` wraps the parser output in a **rowan** lossless CST (immutable, thread-safe,
  full-fidelity incl. whitespace/comments) plus a typed AST layer on top.
- Critical property: *"the syntax crate is completely independent from the rest of
  rust-analyzer. It knows nothing about salsa or LSP."* This is what lets formatters,
  syntax highlighters, and tests use the parser with zero semantic machinery.
- Source: rust-analyzer `docs/dev/architecture.md`
  <https://github.com/rust-lang/rust-analyzer/blob/d7c99931d05e3723d878bea5dc26766791fa4e69/docs/dev/architecture.md>,
  rendered: <https://rust-analyzer.github.io/book/contributing/architecture.html>

### Layer 2 — `base-db` (+ **salsa**)
- Provides the incremental-computation infrastructure and defines most **input queries**
  — "facts supplied by the client of the analyzer" (file text, the crate graph, etc.).
- It is filesystem-agnostic: *"base_db doesn't know about file system and file paths.
  Files are represented with opaque `FileId`, there's no operation to get an
  `std::path::Path` out of the `FileId`."* The VFS/path mapping lives above it.
- Source: architecture.md (as above).

### Layer 3 — the "compiler brain": `hir-expand`, `hir-def`, `hir-ty`, `hir`
- These internal crates *are* "the brain of rust-analyzer … the compiler part of the
  IDE": name resolution, macro expansion, and type inference.
- Key intermediate representations: **`ItemTree`** (condenses a syntax tree into a stable
  "summary"), **`DefMap`** (module tree + scopes), **`Body`** (per-function expression
  info). A core incrementality invariant: *"typing inside a function's body never
  invalidates global derived data."*
- The top-level **`hir`** crate is the **semantic-model façade**: it wraps the internal
  ECS-style API into an OO-flavored API (each call takes an extra `db` argument) and
  exposes the **`Semantics`** type that maps syntax nodes ↔ semantic definitions.
- *"If you think about 'using rust-analyzer as a library', `hir` crate is most likely
  the façade you'll be talking to."* — architecture.md.

### Layer 4 — `ide` (+ `ide-db`, `ide-completion`, `ide-assists`, `ide-diagnostics`)
- The **feature layer**: completion, hover, goto-definition, find-references,
  diagnostics, inlay hints, folding, etc. Built on top of `hir`.
- It is the **public API boundary**, and deliberately protocol-neutral: *"ide crate's
  API is build out of POD types with public fields. The API uses editor's terminology,
  it talks about offsets and string labels rather than in terms of definitions or
  types."* Syntax trees and HIR types are absent from the API surface (used only in
  the implementation).
- `ide-db` holds shared IDE infrastructure (symbol index, the `RootDatabase`, search).
- This layer hosts the **`AnalysisHost` / `Analysis`** pair (see §3).

### Layer 5 — `rust-analyzer` (the server crate)
- *"rust-analyzer is the only crate that knows about LSP and JSON serialization."*
- It owns `GlobalState`, the main loop, the VFS, config, and — crucially — the
  **translation** of `ide`'s POD results into LSP wire types. The library below it has
  no idea LSP exists.
- Source: architecture.md (as above); see also the rendered guide
  <https://rust-analyzer.github.io/book/contributing/guide.html>.

**Why this matters for us:** the entire stack *below* the server crate (`syntax` …
`ide`) is already a reusable library. We are not inventing a separation — we are
copying rust-analyzer's proven one. Our LSP server, CLI, WASM playground, and guitkx
adapter all sit where `rust-analyzer` (the server crate) sits, each mapping the same
`ide`-equivalent POD results to their own protocol.

---

## 2. salsa — the incremental query engine

**What it is:** *"a generic framework for on-demand, incrementalized computation … you
define a 'database' of queries with both inputs and values derived from those inputs;
as you set the inputs, you can re-execute the derived queries and it will try to re-use
results from previous invocations."* (<https://github.com/salsa-rs/salsa>)

**Current version:** **salsa 0.27.0** (released 2026-06-04, on crates.io). This is the
"new salsa" with the macro/entity API — *not* the old `query_group!` API that
rust-analyzer historically vendored as `rust-analyzer-salsa`. (Astral's Ruff/ty and
others have moved to new salsa.)

### Core model (new salsa)
- **`#[salsa::input]`** — the mutable entry points (e.g. a source file). Fields are
  changed via generated setters that take `&mut db`, because mutating an input may
  invalidate derived data:
  ```rust
  #[salsa::input]
  pub struct SourceFile {
      pub path: PathBuf,
      #[returns(ref)] pub text: String,
  }
  ```
- **`#[salsa::tracked]` functions** — pure, **memoized** derived queries:
  ```rust
  #[salsa::tracked]
  fn parse(db: &dyn Db, file: SourceFile) -> Ast<'_> { /* … */ }
  ```
- **`#[salsa::tracked]` structs** — immutable intermediate values created during
  computation (our `Ast`, `SymbolTable`, `Scope`). "Their fields can never change once
  created (until the next revision)." A `#[id]` field lets salsa match a struct to its
  previous-revision counterpart so unchanged downstream queries are not re-run.
- **`#[salsa::interned]`** — cheap-equality interned values (identifiers, paths).
- **`#[salsa::accumulator]`** — side-channel collection, the natural home for
  **diagnostics**: `Diagnostics::push(db, …)` during a tracked fn, then
  `parse::accumulated::<Diagnostics>(db)`.
- **The database**:
  ```rust
  #[salsa::db]
  #[derive(Default, Clone)]
  pub struct AnalyzerDatabase { storage: salsa::Storage<Self> }
  #[salsa::db]
  impl salsa::Database for AnalyzerDatabase {}
  ```

### Invalidation: the red-green algorithm + durability
- On a `set`, salsa bumps the **revision**. When a tracked fn is called again, the
  **red-green** algorithm checks whether the specific inputs it read actually changed;
  if not, it returns the memoized value without re-running.
- **Durability** divides inputs into volatile (the file being typed) vs durable (the
  Godot stdlib / rarely-edited project files). salsa keeps a per-durability **version
  vector**; when a volatile input changes, queries that read *only* durable inputs
  "skip over the entire query subgraph" and never re-validate. This is what keeps
  keystroke-latency analysis cheap on large projects.
  (<https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html>)
- Cancellation is built in: changing an input cancels in-flight queries on other
  snapshots via a panic that unwinds to the query boundary (see §7).

### Decision: salsa now or later?
**Recommendation: design the *boundaries* for salsa from day one, but do NOT wire salsa
into the MVP. Adopt salsa at v1.**
- Reasons to defer: salsa adds a real learning curve and constrains your data model
  (everything flows through `&dyn Db`, tracked structs carry a `'db` lifetime). For an
  MVP whose job is "parse + symbol table + answer stateless queries," a plain
  `HashMap<FileId, Arc<ParsedFile>>` with full re-parse on change is fast enough
  (GDScript files are small) and far simpler to get correct.
- Reasons it's inevitable: cross-file resolution (`class_name`, `preload`, autoloads,
  inheritance) plus interactive latency is exactly salsa's sweet spot, and rolling our
  own memoization/invalidation is a tar pit.
- **Path:** keep all derived computation behind a `db`-shaped trait and pure functions
  from the start (`fn parse(db, file)`, `fn symbols(db, file)`, `fn resolve(db, …)`),
  so the v1 migration is "replace the hand-written cache with `#[salsa::tracked]`,"
  not a rewrite. (See §8.)

Sources: <https://github.com/salsa-rs/salsa>,
<https://salsa-rs.github.io/salsa/overview.html>,
<https://salsa-rs.github.io/salsa/tutorial/db.html>,
durable-incrementality (above).

---

## 3. The public API: `AnalysisHost` + immutable `Analysis` snapshot

We copy rust-analyzer's `ide::Analysis` / `ide::AnalysisHost` design verbatim in shape.

From architecture.md and the `ide::Analysis` rustdoc
(<https://rust-lang.github.io/rust-analyzer/ide/struct.Analysis.html>):

- **`AnalysisHost`** holds the mutable world. *"AnalysisHost is a state to which you can
  transactionally `apply_change`."* There is exactly **one** host.
- **`Analysis`** is *"an immutable snapshot of the state"* and *"the main entry point for
  asking semantic information about the world."* You can cheaply fork an `Analysis` and
  send it to a background thread; there may be many equivalent `Analysis` instances
  while there is a single `AnalysisHost`.
- **Inputs/outputs are file-and-offset based**, never domain types: queries take
  `FilePosition { file_id, offset }` / `FileRange { file_id, range }` and return POD
  results (`NavigationTarget`, `CompletionItem`, `HoverResult`, `Diagnostic`,
  `InlayHint`, …), all wrapped in **`Cancellable<T>`**.

### rust-analyzer `ide::Analysis` surface we mirror (model)
(from the rustdoc page above)
- Navigation: `goto_definition`, `goto_declaration`, `goto_implementation`,
  `goto_type_definition`, `find_all_refs`.
- Completion/help: `completions`, `signature_help`, `hover`.
- Diagnostics: `syntax_diagnostics`, `semantic_diagnostics`, `full_diagnostics`.
- Structure: `file_structure`, `folding_ranges`, `inlay_hints`, `parse`.
- Highlighting: `highlight`, `highlight_range`, `highlight_related`.
- Refactor: `rename`, `structural_search_replace`.
- All return `Cancellable<T>` and speak offsets/ranges, not LSP.

### Our sketch (`crates/ide`)
```rust
pub struct AnalysisHost { db: AnalyzerDatabase }
impl AnalysisHost {
    pub fn new() -> Self;
    pub fn apply_change(&mut self, change: Change);     // set inputs (file text, roots)
    pub fn snapshot(&self) -> Analysis;                 // cheap, Send
}

#[derive(Clone)]                                        // Send + cheap to fork
pub struct Analysis { db: salsa::Snapshot<AnalyzerDatabase> }
impl Analysis {
    pub fn parse(&self, file: FileId) -> Cancellable<SyntaxTree>;
    pub fn completions(&self, pos: FilePosition) -> Cancellable<Vec<CompletionItem>>;
    pub fn hover(&self, pos: FilePosition) -> Cancellable<Option<Hover>>;
    pub fn goto_definition(&self, pos: FilePosition) -> Cancellable<Vec<NavTarget>>;
    pub fn find_all_refs(&self, pos: FilePosition) -> Cancellable<Vec<Reference>>;
    pub fn diagnostics(&self, file: FileId) -> Cancellable<Vec<Diagnostic>>;
    pub fn document_symbols(&self, file: FileId) -> Cancellable<Vec<SymbolNode>>;
    pub fn signature_help(&self, pos: FilePosition) -> Cancellable<Option<SignatureHelp>>;
}
```
`Change`, `FileId`, `FilePosition`/`FileRange`, and every result type are **plain serde
structs** in a small `gdscript-analyzer-base` crate so clients can build/consume them
without depending on salsa internals.

---

## 4. Staying LSP-agnostic

This is already proven by rust-analyzer and is the single most important rule for us:

- The library (`ide` and below) returns **engine-neutral** results: byte offsets,
  `TextRange`, structured items with string labels. *"The API uses editor's terminology,
  it talks about offsets and string labels rather than in terms of definitions or
  types."*
- The **server** crate is *"the only crate that knows about LSP and JSON serialization."*
  It owns the offset↔LSP-`Position` (UTF-8↔UTF-16) conversion and the mapping of POD
  items to `CompletionItem`/`Hover`/`Location` LSP types.

For us this means **N adapters over one library**:
- `gdscript-lsp` (server crate): POD → LSP types (and UTF-16 column mapping).
- `gdscript-cli`: POD → human/JSON text (lint output, `--format json`).
- guitkx adapter: POD → whatever guitkx's TS tooling wants (see §5).
- WASM playground: POD → JSON for the browser UI.

None of these conversions live in the library. Adding a new client never touches `ide`.

---

## 5. Embedded-language support for guitkx (Volar.js model)

**guitkx** is JSX-like markup with embedded GDScript inside `{expr}` blocks. Today the
TS LSP proxies those embedded snippets to Godot; we want `gdscript-analyzer` to answer
them. The well-trodden pattern for "language A embeds language B" is **Volar.js**, the
embedded-language tooling framework behind Vue/Astro/MDX
(<https://volarjs.dev/>, <https://volarjs.dev/core-concepts/embedded-languages/>).

### How Volar models it
- A **`LanguagePlugin`** provides `createVirtualCode` / `updateVirtualCode`. Each handled
  file becomes a **`VirtualCode`** with: `id`, `languageId`, `snapshot` (the text),
  `mappings`, and an `embeddedCodes: VirtualCode[]` for the nested languages.
  (<https://volarjs.dev/reference/languages/>)
- For guitkx: the host `.guitkx` `VirtualCode` would emit an **embedded GDScript
  `VirtualCode`** whose snapshot is a synthetic, valid GDScript document assembled from
  all the `{expr}` blocks (e.g. concatenated, possibly wrapped in a synthetic
  `func`/class so it parses).
- The **`mappings`** array is the source map between host and embedded text. Each
  `Mapping` (from `@volar/source-map`) is:
  ```ts
  interface Mapping<Data = unknown> {
    sourceOffsets: number[];      // offsets in the .guitkx document
    generatedOffsets: number[];   // offsets in the synthetic GDScript document
    lengths: number[];            // segment lengths in source
    generatedLengths?: number[];  // defaults to lengths
    data: Data;                   // CodeInformation: which features this span enables
  }
  ```
  (<https://github.com/volarjs/volar.js/blob/master/packages/source-map/lib/sourceMap.ts>)
- **`CodeInformation`** flags decide *which* features a mapped span participates in —
  `verification` (diagnostics), `completion`, `semantic`, `navigation`, `structure`,
  `format`. So a `{expr}` span can opt into completion+hover+diagnostics but a synthetic
  wrapper prologue we inject can be marked non-navigable.
  (<https://github.com/volarjs/volar.js/blob/master/packages/language-core/lib/types.ts>)
- `SourceMap.toGeneratedLocation(sourceOffset)` and
  `toSourceLocation(generatedOffset)` translate offsets in each direction by binary-/
  linear-searching the offset arrays. That is the entire request/response round-trip:
  map host position → embedded position, ask the analyzer, map results back.

### How a consumer feeds *our* analyzer
The Rust library does **not** need to know about guitkx. The guitkx client:
1. Builds the synthetic GDScript document + a `mappings` source map (Volar-style) from
   the `.guitkx` file.
2. Registers the synthetic doc as an ordinary `FileId` via `apply_change` (a virtual
   path like `guitkx://Foo.guitkx/embedded.gd`).
3. Calls our normal offset-based queries with the **mapped (generated) offset**.
4. Translates returned offsets/ranges **back** to `.guitkx` coordinates with the same
   source map before handing them to the editor.

So the library stays a pure GDScript analyzer; **embedding is a client concern** layered
on top via a source map — exactly Volar's "virtual code" separation, just with our Rust
analyzer playing the role of the embedded-language service. Crucially, our offset-based,
LSP-agnostic API (§3/§4) is what makes this clean: a synthetic document plus an offset
map is all the client needs.

---

## 6. The FFI boundary (Rust core, TS/Node server, browser playground)

Across napi or WASM you **cannot** hand JS rich Rust objects (the salsa db, rowan trees,
HIR). Two complementary shapes, and we use both:

1. **Stateful handle (the host stays in Rust).** Keep `AnalysisHost` alive on the Rust
   side behind an opaque handle. JS calls methods (`applyChange`, `completions(fileId,
   offset)`) and gets back **serde-serialized JSON value objects**. This preserves
   incrementality across edits — the salsa cache lives in the handle, not re-created per
   call. This is the primary shape.
2. **Stateless request/response value objects (serde).** For the CLI and CI, a
   one-shot `analyze(files) -> Report` where everything in and out is a plain serde
   struct. Easy to test, trivially serializable.

**How the JS-tooling ecosystem does it (model):**
- **Biome** and **oxc** expose their Rust engines to Node via **napi-rs**; oxc also
  shipped a `wasm-bindgen` build for the browser playground. The cross-boundary data is
  serializable value objects (ASTs/reports), not live Rust handles.
  (<https://napi.rs/blog/announce-v2>, <https://docs.rs/biome_analyze/>)
- **napi-rs v3** is the key enabler for us: *"you can compile your project into
  WebAssembly with almost no code changes,"* targeting **both** Node native addons and
  browser WASM from **one** codebase — exactly the duplication oxc used to suffer
  maintaining separate napi + wasm-bindgen bindings.
  (<https://napi.rs/blog/announce-v3>)

**Recommendation:**
- One thin `gdscript-analyzer-ffi` crate using **napi-rs v3**, exposing an
  `AnalysisHandle` class (methods take `fileId: number`, `offset: number`; return JSON)
  plus a couple of stateless helpers.
- All FFI result types are `#[derive(Serialize)]` POD mirrors of the `ide` POD types
  (or the same types if they already derive serde). Keep the FFI surface *small* and
  *flat* (numbers, strings, arrays of plain objects) — no enums-with-data gymnastics
  that serialize poorly.
- One Rust codebase → `@gdscript/analyzer` (Node, for the TS LSP + guitkx) and a WASM
  package (browser playground) from the same crate.

---

## 7. Incremental + cancellation + threading (and WASM reality)

**Cancellation (free with salsa).** rust-analyzer/salsa cancel in-flight work when
inputs change: *"If a task queries an invalidated input, it is cancelled via a special
panic that is captured at the task join site."* `unwind_if_cancelled` lets expensive
queries poll for cancellation. The `ide` layer is *"the boundary where the panic is
caught and transformed into a `Result<T, Cancelled>`"* — hence `Cancellable<T>` on every
query. (<https://docs.rs/rust-analyzer-salsa/latest/salsa/>, architecture.md)

**Parallelism.** Old salsa required a `ParallelDatabase::snapshot()` and only
`RootDatabase` implemented it; "parallel query dependencies are not legible to Salsa,
which complicates the invalidation story." **New salsa (0.27 / the "3.0" line)** gives
"trivial parallel computation for all databases" and hands out references instead of
cloned values — rust-analyzer's own perf plan targets "sub-100ms autocomplete … on
multi-core machines" off the back of it.
(<https://github.com/rust-lang/rust-analyzer/issues/17491>)

**WASM reality:** standard `wasm32` has **no threads**. napi-rs notes `std::thread`/
`tokio`/Rayon only work under `wasm32-wasip1-threads` (not browser). So the browser
playground is single-threaded regardless.

**Recommended MVP threading model:**
- **Single-writer, multi-reader.** The owner of `AnalysisHost` (LSP main loop) applies
  changes on one thread; it forks `Analysis` snapshots for read queries.
- **Native (LSP/CLI):** run read queries on a small worker pool or per-request threads,
  relying on snapshot + cancellation. Don't build a custom scheduler for MVP.
- **WASM (playground):** single-threaded, synchronous; queries run on the JS event loop.
  Same library, no thread APIs compiled in (feature-gate any thread use).
- Keep all query types `Send` so the native path can parallelize later without a
  redesign.

---

## 8. MVP vs v1 architecture (pragmatic progression)

**Guiding principle:** ship a useful single-file analyzer fast, but lock in the
*boundaries* (offset-based POD API; pure `(db, file) -> derived` query functions;
host/snapshot split) so the incremental engine drops in without a rewrite.

### Crate layout (final shape; MVP fills a subset)
```
crates/
  gdscript-syntax      # lexer + parser + rowan CST/typed AST.  No salsa, no LSP.
  gdscript-base        # FileId, FilePosition/Range, Change, all POD result structs (serde)
  gdscript-db          # salsa inputs + tracked queries (v1).  MVP: hand-rolled cache.
  gdscript-hir         # symbol table, scopes, name resolution, (later) type inference
  gdscript-ide         # AnalysisHost / Analysis; features: completion/hover/goto/diags
  gdscript-ffi         # napi-rs v3: Node addon + WASM, JSON in/out  (thin)
clients/ (separate, depend only on gdscript-ide / gdscript-base)
  gdscript-lsp         # LSP server: POD -> LSP types, UTF-16 mapping
  gdscript-cli         # lint/format CLI: POD -> text/JSON
  (guitkx adapter, in the TS repo, via gdscript-ffi)
  (playground, browser, via gdscript-ffi WASM)
```

### MVP (weeks, not months)
- `gdscript-syntax`: full lexer + parser + rowan tree + typed AST. **This is the
  foundation — do it properly**, it's reused unchanged forever.
- `gdscript-base`: the POD types + `FileId`/positions (serde).
- `gdscript-hir` (minimal): per-file **symbol table** + scopes; resolve locals,
  functions, `class_name`, `extends`, members within a file.
- `gdscript-db` (MVP form): a plain `HashMap<FileId, Arc<Parsed>>`; on `apply_change`,
  **re-parse the changed file** (and dependents, naively). Expose computation as **pure
  functions** `fn parse(store, file)`, `fn symbols(store, file)` so they're salsa-shaped.
- `gdscript-ide`: `AnalysisHost`/`Analysis` with `parse`, `diagnostics` (syntax +
  undefined-symbol), `document_symbols`, `hover`, `completions`, `goto_definition`
  (intra-file first). Return `Cancellable<T>` even if MVP never actually cancels.
- `gdscript-ffi`: napi-rs v3 handle + JSON, so the TS LSP and playground can integrate
  immediately.
- **Defer:** cross-file project graph, type inference, salsa, parallelism, rename/SSR.

### v1 (the incremental analyzer)
- Replace `gdscript-db` internals with **salsa 0.27**: `#[salsa::input]` for files,
  `#[salsa::tracked]` for parse/symbols/resolve, `#[salsa::accumulator]` for
  diagnostics, **durability** for the Godot stdlib/builtins. Public `ide` API is
  unchanged — only the engine behind it changes.
- Flesh out `gdscript-hir`: cross-file name resolution (autoloads, `preload`/`load`,
  inheritance chains), then type inference.
- Turn on real cancellation (salsa panic → `Cancelled`) and native multi-reader
  parallelism (snapshots), keeping WASM single-threaded via feature gates.
- guitkx embedded support hardened: synthetic-document + Volar-style source map fed
  through the *unchanged* offset-based API.

### Why this ordering is safe
Every later step changes an **implementation**, never the **boundary**: the
offset-in/POD-out `Analysis` API (§3/§4), the pure `(db, file) -> value` query shape
(§2), and the host/snapshot split (§3) are all chosen on day one specifically so that
"add salsa," "add cross-file HIR," and "add threads" are localized swaps, not rewrites —
the same property that let rust-analyzer evolve its engine under a stable `ide` API.

---

## Sources
- rust-analyzer architecture (pinned): <https://github.com/rust-lang/rust-analyzer/blob/d7c99931d05e3723d878bea5dc26766791fa4e69/docs/dev/architecture.md>
- rust-analyzer architecture (rendered): <https://rust-analyzer.github.io/book/contributing/architecture.html>
- rust-analyzer contributing guide: <https://rust-analyzer.github.io/book/contributing/guide.html>
- `ide::Analysis` rustdoc: <https://rust-lang.github.io/rust-analyzer/ide/struct.Analysis.html>
- salsa repo (v0.27.0): <https://github.com/salsa-rs/salsa>
- salsa overview: <https://salsa-rs.github.io/salsa/overview.html>
- salsa tutorial — database: <https://salsa-rs.github.io/salsa/tutorial/db.html>
- salsa durability (durable incrementality): <https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html>
- salsa (legacy fork) rustdoc, cancellation/parallel: <https://docs.rs/rust-analyzer-salsa/latest/salsa/>
- rust-analyzer perf plan (salsa 3.0, parallelism): <https://github.com/rust-lang/rust-analyzer/issues/17491>
- Volar.js home: <https://volarjs.dev/>
- Volar embedded languages: <https://volarjs.dev/core-concepts/embedded-languages/>
- Volar languages reference: <https://volarjs.dev/reference/languages/>
- Volar `VirtualCode`/`CodeInformation` types: <https://github.com/volarjs/volar.js/blob/master/packages/language-core/lib/types.ts>
- Volar `Mapping`/`SourceMap`: <https://github.com/volarjs/volar.js/blob/master/packages/source-map/lib/sourceMap.ts>
- napi-rs v2 (Node addons in Rust): <https://napi.rs/blog/announce-v2>
- napi-rs v3 (one codebase → Node + WASM): <https://napi.rs/blog/announce-v3>
- biome_analyze (Rust analysis crate exposed to JS): <https://docs.rs/biome_analyze/>
