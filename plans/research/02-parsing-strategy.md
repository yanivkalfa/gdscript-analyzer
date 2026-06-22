# 02 — Parsing Strategy for `gdscript-analyzer`

> Research note. Goal: choose the PARSING layer for a reusable Rust GDScript static-analysis library
> ("Roslyn for Godot"). Hard requirements: **error recovery** (parse broken code), **incremental
> reparsing**, a **lossless / full-fidelity syntax tree** (whitespace + comments preserved, for
> formatting + rename), and **stable cross-version maintenance** as GDScript evolves.
>
> Date of research: 2026-06-22. All claims cited with URLs; versions/dates noted inline.

---

## TL;DR Recommendation

**Adopt strategy (C-prime): hand-written, rowan-based, lossless recursive-descent parser as the
primary target — but bootstrap the MVP behind a parser-trait abstraction, optionally wrapping
`tree-sitter-gdscript` for the very first weeks.** Concretely:

- The **syntax tree we commit to and expose in our public API is a rowan red-green CST** built by a
  **resilient hand-written recursive-descent parser** (the rust-analyzer / Biome architecture).
- A small **`logos`-based lexer** plus a **hand-written indentation pre-pass** (Python-style
  INDENT/DEDENT/NEWLINE injection) feeds the parser. This is the part that "owns the hard problem"
  (significant whitespace) and is ~1–2 weeks of work, reusable regardless of parser backend.
- We **do not make `tree-sitter-gdscript` our long-term grammar of record**, because (a) the grammar
  is controlled by a single external maintainer and synced to Godot by hand, (b) tree-sitter's comment
  handling (`extras`) is awkward for lossless formatting / rename, and (c) we want to own error
  messages and recovery quality. It is, however, an **excellent reference grammar and an acceptable
  MVP stopgap** behind our trait.

Rationale, evidence, and the migration path are below.

---

## 1. tree-sitter-gdscript — maturity assessment

**Canonical repo:** `PrestonKnopp/tree-sitter-gdscript` — "Tree sitter grammar for Godot's GDScript."
MIT licensed. Companion repo `PrestonKnopp/tree-sitter-godot-resource` parses `.tscn` / `.tres` /
`project.godot`.
Sources: <https://github.com/PrestonKnopp/tree-sitter-gdscript>,
<https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/LICENSE>,
<https://github.com/PrestonKnopp/tree-sitter-godot-resource>.

**Maintenance / activity (good):** Latest release **v6.1.0 (2025-11-02)**; ~348 commits across 17
releases; issues opened by the maintainer as recently as Sep/Oct 2025; npm package updated within ~2
months of this research. It is **actively, if single-maintainer, maintained**.
Sources: <https://github.com/PrestonKnopp/tree-sitter-gdscript>,
<https://www.npmjs.com/package/tree-sitter-gdscript>.

**Godot-version sync (manual, lossy):** The README tracks a "Latest Godot Commit Syntactically Synced"
marker (e.g. Godot module commit `6ae54fd787`) and explicitly warns **"Some commits may have been
missed."** Sync is a manual human process against `modules/gdscript` — there is no automated guarantee
it matches a given Godot 4.x release.
Source: <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/README.md>.

**Grammar completeness (broadly complete for GDScript 2.0 / Godot 4.x):** It covers modern GDScript
(typed vars, annotations, lambdas, `match`, signals, `await`, etc.) well enough that real tools ship
on it (see GDQuest below). No authoritative per-version completeness matrix is published; gaps surface
as issues. GDQuest's own formatter README concedes "GDScript has grown into a complex language… there
can always be edge cases or less common syntax that may not be handled correctly yet."
Source: <https://github.com/GDQuest/GDScript-formatter>.

**Error recovery & incremental parsing (inherited from tree-sitter core, good):** tree-sitter is "an
incremental parsing system… can build a concrete syntax tree… and efficiently update the syntax tree
as the source file is edited," typically **< 1 ms** per edit, and performs **error recovery** by
bounding each error region and still returning a usable tree. The tree is a lossless CST that "should
include all the whitespace and comments." Initial (cold) parse is ~2–3× slower than a hand-written
parser, but incremental edits are sub-millisecond.
Sources: <https://github.com/tree-sitter/tree-sitter>,
<https://tomassetti.me/incremental-parsing-using-tree-sitter/>,
<https://github.com/tree-sitter/tree-sitter-rust>.

**Indentation:** handled by a C **external scanner** (`src/scanner.c`) — see §4.
Source: <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/src/scanner.c>.

**Rust + WASM bindings (exist, first-class):**
- Rust: published as the `tree-sitter-gdscript` crate; consumed via the `tree-sitter` crate
  (`tree_sitter::Parser` + `Language`). The `tree-sitter` crate is at **0.26.x (2025)** and offers a
  `wasm` feature (via `wasmtime-c-api`).
- WASM/browser: `web-tree-sitter` on npm; `tree-sitter build --wasm` (since v0.26.1 auto-downloads
  `wasi-sdk`).
Sources: <https://crates.io/crates/tree-sitter-gdscript>, <https://crates.io/crates/tree-sitter>,
<https://docs.rs/tree-sitter>, <https://docs.rs/crate/tree-sitter/latest>,
<https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md>.

**Who uses it:**
- **Zed** uses tree-sitter-gdscript for GDScript support.
  Source: <https://zed.dev/docs/languages/gdscript>.
- **GDQuest GDScript-formatter** (Rust, MIT, production; v0.20.1 2026-05-26) is built on
  `PrestonKnopp/tree-sitter-gdscript` via **Topiary** (tree-sitter-query-driven formatter). This is
  the strongest existing proof that tree-sitter-gdscript is good enough for real formatting tooling.
  Source: <https://github.com/GDQuest/GDScript-formatter>, <https://www.gdquest.com/library/gdscript_formatter/>.
- **Godot's own editor / official VSCode plugin DO NOT use tree-sitter.** Godot has its own
  **hand-written C++ tokenizer + recursive-descent parser** in `modules/gdscript`
  (`gdscript_tokenizer.cpp`, `gdscript_parser.cpp`), and the official
  `godotengine/godot-vscode-plugin` gets language features from **Godot's built-in LSP** (it launches
  a headless Godot editor as the language server) and uses a **TextMate grammar** only for
  highlight/format fallback — not tree-sitter.
  Sources: <https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_tokenizer.cpp>,
  <https://github.com/godotengine/godot-vscode-plugin>,
  <https://deepwiki.com/godotengine/godot-vscode-plugin/3.4-code-formatting-and-snippets>.

**Verdict:** tree-sitter-gdscript is **production-grade as a fast incremental highlighter / structural
parser and a fine MVP / reference grammar**, but it is **not the ideal grammar-of-record for an
IDE-fidelity static-analysis library we intend to own**: single external maintainer, manual Godot
sync ("commits may have been missed"), tree-sitter's `extras`-based comment model is awkward for
lossless formatting/rename (§5), and we'd be downstream of grammar/error-recovery decisions we don't
control. **Great starting point; not the destination.**

---

## 2. Hand-written lossless parser — the rust-analyzer / Biome approach

This is the architecture used by **rust-analyzer**, **Biome**, Swift libsyntax, C#/Roslyn, and Kotlin.

**Layering (lexer → parser → CST → AST):**
- `crates/parser` is a **hand-written recursive-descent parser** that emits a flat **event stream**
  ("start node X", "finish node Y", "advance"); it is decoupled from the tree via `TokenSource` /
  `TreeSink`-style traits, so it can target either source text or macro token-trees.
- `crates/syntax` turns events into a **lossless CST using `rowan`**, then offers a typed **AST as a
  thin, zero-cost view over the CST**.
Sources: <https://rust-analyzer.github.io/book/contributing/architecture.html>,
<https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/architecture.md>.

**Lossless trees — `rowan` (red-green):** rowan is "a library for lossless syntax trees, inspired in
part by Swift's libsyntax." It uses a **red-green tree**: the **green tree** holds position-independent
source text (including all whitespace/comments) as shared nodes keyed by a `SyntaxKind` + text width;
the **red tree** is built lazily on top to give absolute offsets/parent pointers. Perfect
round-tripping (every byte preserved) is what enables formatting, rename, and code actions.
Sources: <https://github.com/rust-analyzer/rowan>,
<https://github.com/rust-analyzer/rowan/blob/master/README.md>,
<https://dev.to/cad97/lossless-syntax-trees-280c>.

**`cstree` (the alternative to rowan):** a rowan-derived crate that (a) **interns** token text
(dedups identifiers), and (b) makes red nodes **`Send + Sync`** so realized trees can be shared across
threads — at the cost of **no in-place mutation** (you replace nodes to produce new trees) and red
nodes staying allocated once created. For a library that may be driven from an LSP server with a
thread pool, `cstree`'s thread-safety + interning is attractive.
Sources: <https://github.com/domenicquirl/cstree>, <https://docs.rs/cstree>,
<https://crates.io/crates/cstree>.

**Error recovery (resilient parsing):** The defining property is **"parsing never fails": the parser
returns `(Tree, Vec<Error>)` rather than `Result`**, so it always yields a tree even on broken code —
essential for IDE responsiveness. matklad's **"Resilient LL Parsing Tutorial" (2023-05-21)** is the
canonical recipe and states the two goals: **(1) localize errors** so a mistake in one function
doesn't disturb highlighting of unrelated functions, and **(2) recognize valid prefixes / partial
constructs** (e.g. an incomplete `func` is still recognized as a function). Mechanisms: an
**`Open`/`Close`/`Advance` event stream** with a `MarkOpened`/`MarkClosed` API (so nodes can be
wrapped retroactively), a **homogeneous tree** where any node can hold tokens or subtrees in any order
(so `ErrorTree` nodes drop in without special-casing), and **recovery sets** (FIRST/FOLLOW-derived)
that decide at each loop whether to consume, skip-as-error, or bubble up.
Sources: <https://rust-analyzer.github.io/book/contributing/architecture.html>,
<https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html>,
<https://thunderseethe.dev/posts/parser-base/>.

**Incremental reparsing:** rust-analyzer supports **low-latency incremental reparses** at the syntax
layer and uses the **Salsa** query framework above it for demand-driven caching/invalidation of
higher analyses. (Practical MVP shortcut: reparse the whole file per keystroke — files are small —
and add block-level incrementality later. tree-sitter gives sub-ms edits "for free," which is one
reason to keep it available behind the trait early on.)
Sources: <https://rust-analyzer.github.io/book/contributing/architecture.html>,
<https://rust-analyzer.github.io/blog/2021/12/30/2021-recap.html>.

**How others parse (lossless vs not):**
- **Biome** hand-writes its parser and produces a **lossless CST** via an internal rowan fork,
  **`biome_rowan`**; its CST "keeps track of all the information of a program, trivia included… every
  character from the original source, including whitespace and comments." Producing a CST (not an AST)
  is "a lot more work," which is the explicit tradeoff vs swc/oxc.
  Sources: <https://deepwiki.com/biomejs/biome/6.1-parser-architecture>,
  <https://docs.rs/biome_rowan>, <https://biomejs.dev/internals/architecture/>.
- **swc / oxc** are extremely fast hand-written parsers but produce **ASTs, not lossless CSTs**
  (optimized for compile/transform throughput, not round-trip formatting). oxc benchmarks itself ~3×
  faster than swc and ~5× faster than Biome — precisely because Biome pays for a lossless CST.
  Sources: <https://github.com/oxc-project/bench-javascript-parser-written-in-rust>,
  <https://oxc.rs/docs/guide/benchmarks>.

**What it takes to hand-write a GDScript parser:** a `logos` lexer (~a few hundred token rules) + an
indentation pre-pass (§4) + a resilient recursive-descent parser (~the grammar surface of GDScript:
class/func/var/const/enum/signal/annotations/match/lambdas/typed params/expressions w/ a Pratt
expression parser). Order-of-magnitude: **a focused few weeks for a usable parser, a few months to
rust-analyzer polish**. matklad's tutorial + Biome/rust-analyzer source are direct templates.

---

## 3. Parser-combinator / generator options

| Tool | Type | Lossless CST? | Error recovery | Indentation (significant whitespace) | Fit |
|---|---|---|---|---|---|
| **logos** | lexer generator (derive) | n/a (tokens) | n/a | Lexer only — INDENT/DEDENT done in a pre-pass you write | **Use it** for the lexer |
| **chumsky** (1.0-era) | parser combinator | No (builds your output type; not a rowan CST out of the box) | **First-class**, recovery strategies built in | Docs claim **Python-style semantic indentation** support; combine with logos via adapter | Strong for AST tools; not lossless by default |
| **lalrpop** | LR(1) generator | No (AST via actions) | Weak/limited (LR error recovery is notoriously hard) | Indentation needs a hand-written pre-lexer feeding the generated parser | Poor fit for IDE recovery + whitespace |
| **pest** | PEG generator (grammar file) | No | "error reporting and recovery" but PEG ordered-choice obscures errors | Possible but clumsy for INDENT/DEDENT | Poor fit for IDE-grade recovery |
| **tree-sitter** | GLR-ish incremental | **Yes (CST, all bytes)** | **Yes**, built-in | **External C scanner** | Strong for highlight/incremental; comment model awkward for formatting (§5) |
| **hand-written + rowan/cstree** | recursive descent | **Yes** | **Best (you control it)** | Pre-pass you own | **Best for our requirements** |

Notes/citations:
- **logos**: "ridiculously fast lexers," compiles token specs to a single DFA / jump tables; rewrite
  aims to allow rope (`ropey`) input later. Pure lexer — indentation logic lives in your code.
  Sources: <https://github.com/maciejhirsz/logos>, <https://docs.rs/logos>, <https://logos.maciej.codes/>.
- **chumsky**: parser-combinator "with powerful error recovery," "fast enough" (~630k lines JSON/s,
  slower than a tuned hand-written parser but explicitly fine until you're large), context-sensitive
  features incl. **Python-style semantic indentation**; integrates with logos via a `TokenStream`
  adapter. But its output is your chosen type — you'd have to build a rowan CST yourself to be
  lossless, which removes much of its convenience.
  Sources: <https://docs.rs/chumsky>, <https://github.com/zesterer/chumsky>,
  <https://blog.jsbarretto.com/post/parser-combinators-and-error-recovery>,
  <https://news.ycombinator.com/item?id=32031591>.
- **lalrpop** (LR(1)) and **pest** (PEG): great for batch compilers/DSLs, but neither gives the
  IDE-grade *recovery + lossless trivia* combination cheaply, and both fight Python-style indentation.
  Sources: <https://github.com/lalrpop/lalrpop>, <https://docs.rs/pest>,
  <https://rustprojectprimer.com/ecosystem/parsing.html>.

**Takeaway:** generators/combinators optimize for *AST construction*, not the *lossless-CST + bespoke
recovery + significant-whitespace* trifecta we need. The two tools that actually deliver that trifecta
are **tree-sitter** (external grammar) and a **hand-written rowan/cstree parser** (we own it). `logos`
is a useful component in the hand-written path.

---

## 4. Indentation sensitivity (the hard part)

GDScript uses **Python-like significant indentation** (independent of Python). Godot's tokenizer
**emits special `INDENT` / `DEDENT` / `NEWLINE` tokens** so the parser can see block structure, tracks
a `line_continuation` flag for trailing **backslash `\`**, and **suppresses indentation changes inside
multiline expressions** (inside brackets/parens). It is lenient — it reports **many errors at once**
rather than stopping, including indentation mismatches. Godot 4.0 **forbids mixing tabs and spaces**
(error: "Used tab character for indentation instead of space as used before in the file"); the dedicated
tracking issue `godotengine/godot#40488` catalogs the indentation/line-break edge cases.
Sources: <https://godotengine.org/article/gdscript-progress-report-writing-tokenizer/> (article dated
**May 2020**), <https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_tokenizer.cpp>,
<https://github.com/godotengine/godot/issues/40488>,
<https://github.com/godotengine/godot-proposals/issues/2800>.

**How tree-sitter-gdscript's external scanner (`src/scanner.c`) does it:**
- Maintains an **`indent_vec` indent stack** (guard 0 at base; push on increase).
- `skip_whitespace`: **space = +1, tab = +8**, newline resets the column counter — i.e. **a tab counts
  as 8 columns** for comparison purposes.
- Emits **INDENT** when indentation increases, **DEDENT** when it decreases, **NEWLINE** at line
  boundaries (outside error-recovery mode).
- **Line continuation:** `\` immediately before `\n` continues the logical line (no NEWLINE emitted).
- **Bracket context:** tracks `within_brackets`; **inside brackets/parens, indentation changes are
  suppressed** (multiline calls/arrays/dicts don't generate INDENT/DEDENT).
- **Empty lines** keep indentation state (no spurious DEDENT), preventing premature block close.
- Special **comment re-attachment**: a column-0 comment inside a function body is reassigned to the
  function's indent level so it doesn't falsely close scope; `#region`/`#endregion` are excluded from
  that adjustment.
- **Known wart in the source:** commented-out `COLON` handling with a note that it **"breaks if
  elses"**, i.e. fragility around `:`-introduced blocks / dict-vs-block ambiguity.
Source: <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/src/scanner.c>.

**Edge cases that bite any implementation (must be in our test corpus):**
1. **Backslash line continuation** `\` — join lines, don't emit NEWLINE/INDENT.
2. **Multiline brackets** `(`, `[`, `{` — newlines/indentation inside are *not* significant; need a
   bracket-depth counter that pauses INDENT/DEDENT.
3. **`func` bodies / nested blocks** — the core INDENT/DEDENT driver.
4. **Lambdas** (`func(): ...` inline + multiline lambda bodies) — a block that can start mid-expression.
5. **`match` blocks** — patterns introduce a nested indentation context under `:`.
6. **Tab vs space** — must compare consistently (Godot errors on mixing; we should *recover* and flag,
   not abort) and pick a tab width convention for column math (scanner uses 8).
7. **Empty / comment-only / whitespace-only lines** — must not emit DEDENT.
8. **Dedented comments** — attach to the right scope (the scanner's special case above).
9. **Trailing `:` introducing a block** — the source-noted "breaks if elses" ambiguity zone.

**Plan for our parser:** implement INDENT/DEDENT/NEWLINE injection as a **standalone, well-tested
indentation pre-pass over the `logos` token stream** (a small state machine: indent stack +
bracket-depth counter + continuation flag), modeled on Godot's own tokenizer semantics so we match the
engine's behavior. This isolates ~80% of the language's quirk-risk into one ~few-hundred-line module
with a golden-file test corpus, and it's reusable whether the backend is hand-written or (temporarily)
tree-sitter.

---

## 5. Lossless vs AST-only — for our IDE use cases

Our use cases — **formatting, rename, semantic tokens, code actions** — all require **trivia
(whitespace + comments) preserved with exact byte offsets**, i.e. a **lossless CST**, not just an AST.

- **tree-sitter:** *is* lossless in the sense that every byte is reachable, and it's incremental and
  recovers from errors — strong. **But** comments/whitespace are typically modeled as **`extras`** that
  "may appear anywhere," which gives **limited control over where comments attach in the tree**. This
  is a known pain point for **lossless formatting and comment-attachment**; Topiary explicitly warns
  that nodes not captured by a query "lose their token separators" — a recurring class of formatter bug
  on tree-sitter trees.
  Sources: <https://tree-sitter.github.io/tree-sitter/creating-parsers/2-the-grammar-dsl.html>,
  <https://github.com/tree-sitter/tree-sitter/discussions/2186>,
  <https://topiary.tweag.io/book/getting-started/on-tree-sitter.html>,
  <https://news.ycombinator.com/item?id=36008139>.
- **logos + hand-rolled rowan/cstree:** trivia are **first-class tokens we place deliberately** in the
  green tree (leading/trailing trivia attached to the node we choose), exactly the model rust-analyzer
  and Biome use for high-fidelity formatting and rename. We also **own error nodes and error messages**,
  which matters for code actions and diagnostics quality.
  Sources: <https://github.com/rust-analyzer/rowan>, <https://docs.rs/biome_rowan>,
  <https://deepwiki.com/biomejs/biome/6.1-parser-architecture>.

**Conclusion:** both are technically lossless, but for **maintainability + IDE-fidelity + recovery we
control**, the **hand-rolled rowan/cstree CST wins**; tree-sitter wins on **time-to-MVP + free
incrementality**. This asymmetry is exactly what the hybrid recommendation exploits.

---

## 6. RECOMMENDATION

**Primary strategy: (B) a hand-written, lossless, rowan/cstree-based, resilient recursive-descent
parser — reached via a short (C) hybrid bootstrap.** I.e. commit to owning the parser, but de-risk the
schedule by abstracting the backend so an optional tree-sitter wrapper can deliver functionality in
week 1 while we build the real thing.

**Why B is the destination (not A):**
1. **We must own the grammar.** The user's explicit goal is a *community-grade foundation that stays
   synced with Godot*. tree-sitter-gdscript is a **single-maintainer** grammar **manually** synced to
   Godot, with the README itself warning **"some commits may have been missed."** Owning a Rust grammar
   lets *us* (and the community) PR Godot-version updates directly into the analyzer on our schedule,
   tied to our test corpus.
   Source: <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/README.md>.
2. **IDE fidelity.** Rename/format need precise, controllable **comment/trivia attachment** and
   **error nodes** — where tree-sitter's `extras` model and query-driven formatting are weakest (§5).
   Sources: <https://github.com/tree-sitter/tree-sitter/discussions/2186>,
   <https://topiary.tweag.io/book/getting-started/on-tree-sitter.html>.
3. **Error-message quality.** A "Roslyn for Godot" lives and dies by diagnostics; a hand-written
   resilient parser lets us craft messages and recovery, following the proven rust-analyzer/Biome path.
   Sources: <https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html>,
   <https://rust-analyzer.github.io/book/contributing/architecture.html>.
4. **Precedent that it's tractable.** Biome ships a full lossless hand-written CST parser; rust-analyzer
   is the reference; even small GDScript tools (`atelico/gdstyle`, MIT, Rust) already hand-write an
   *"indentation-aware"* GDScript lexer + parser. The path is well-trodden.
   Sources: <https://deepwiki.com/biomejs/biome/6.1-parser-architecture>, <https://github.com/atelico/gdstyle>.

**Why a hybrid bootstrap (the (C) on-ramp), not pure A or pure B from day one:**
- **A pure-B start is months before anything works.** A thin **tree-sitter-gdscript wrapper behind a
  `Parser` trait** can give us a real lossless-ish tree + sub-ms incrementality + error recovery in
  **week 1**, unblocking downstream work (symbol tables, semantic tokens) immediately. It is MIT, has
  a maintained **Rust crate** and **WASM** path, and is already proven in **Zed** and the **GDQuest
  formatter**.
  Sources: <https://crates.io/crates/tree-sitter-gdscript>, <https://zed.dev/docs/languages/gdscript>,
  <https://github.com/GDQuest/GDScript-formatter>.

**Concrete migration path:**
1. **Week 0–1 — abstraction + lexer.** Define a `SyntaxKind` enum and a `Parser`/`SyntaxTree` trait.
   Build the **`logos` lexer + indentation pre-pass** (§4) first — it's needed by *both* backends and
   is the riskiest language-specific piece. Land a golden-file test corpus for indentation edge cases.
2. **Weeks 1–3 — tree-sitter MVP (optional but recommended).** Wrap `tree-sitter-gdscript` to satisfy
   the trait, mapping its CST into our `SyntaxKind`s. Ship semantic tokens + outline + basic
   diagnostics. This validates the API and gives users something now. Keep it **feature-gated** so it
   never becomes load-bearing.
3. **Weeks 3–N — hand-written rowan/cstree parser.** Implement the resilient recursive-descent parser
   per matklad's tutorial against **`cstree`** (prefer cstree over rowan for **`Send+Sync` trees +
   interning**, valuable under an LSP threadpool), producing our lossless CST + thin typed AST views.
   Use the tree-sitter output as a **differential oracle** in tests (parse the same corpus both ways,
   diff structure) to reach parity fast.
4. **Cutover.** Flip the default `Parser` backend to the hand-written one once it passes the corpus +
   differential tests; demote tree-sitter to a test oracle / optional fast-incremental fallback.
5. **Cross-version maintenance.** Track Godot's `modules/gdscript` tokenizer/parser as the *source of
   truth*; encode each Godot 4.x version's surface as test fixtures; PRs that bump Godot support touch
   our grammar + corpus, not an external repo.

**Crate choice within B:** `cstree` over `rowan` for thread-safe shared trees + token interning (LSP
concurrency), accepting its no-in-place-mutation constraint (we produce new trees on edit anyway).
Sources: <https://github.com/domenicquirl/cstree>, <https://docs.rs/cstree>.

---

## Key sources (chronology / versions)

- tree-sitter-gdscript repo, **v6.1.0 2025-11-02**, MIT, manual Godot sync —
  <https://github.com/PrestonKnopp/tree-sitter-gdscript>,
  <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/README.md>,
  <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/src/scanner.c>,
  <https://github.com/PrestonKnopp/tree-sitter-gdscript/blob/master/LICENSE>
- tree-sitter core (incremental + recovery + lossless), crate **0.26.x (2025)** —
  <https://github.com/tree-sitter/tree-sitter>, <https://crates.io/crates/tree-sitter>,
  <https://docs.rs/tree-sitter>, <https://tomassetti.me/incremental-parsing-using-tree-sitter/>,
  <https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md>
- tree-sitter comment/`extras` limitation — <https://github.com/tree-sitter/tree-sitter/discussions/2186>,
  <https://topiary.tweag.io/book/getting-started/on-tree-sitter.html>
- GDQuest GDScript-formatter (Rust/Topiary/tree-sitter, v0.20.1 2026-05-26) —
  <https://github.com/GDQuest/GDScript-formatter>, <https://www.gdquest.com/library/gdscript_formatter/>
- atelico/gdstyle (hand-written GDScript lexer+parser, Rust, MIT, v0.1.7 2026-06-09) —
  <https://github.com/atelico/gdstyle>
- Godot tokenizer/parser (engine source of truth) —
  <https://github.com/godotengine/godot/blob/master/modules/gdscript/gdscript_tokenizer.cpp>,
  <https://godotengine.org/article/gdscript-progress-report-writing-tokenizer/> (May 2020),
  <https://github.com/godotengine/godot/issues/40488>
- Godot VSCode plugin uses Godot's own LSP, not tree-sitter —
  <https://github.com/godotengine/godot-vscode-plugin>,
  <https://deepwiki.com/godotengine/godot-vscode-plugin/3.4-code-formatting-and-snippets>
- Zed uses tree-sitter-gdscript — <https://zed.dev/docs/languages/gdscript>
- rust-analyzer architecture (hand-written resilient parser, rowan, lossless, Salsa) —
  <https://rust-analyzer.github.io/book/contributing/architecture.html>,
  <https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/architecture.md>
- matklad, "Resilient LL Parsing Tutorial," 2023-05-21 —
  <https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html>
- rowan (lossless red-green trees) — <https://github.com/rust-analyzer/rowan>,
  <https://dev.to/cad97/lossless-syntax-trees-280c>
- cstree (Send+Sync + interning fork of rowan) — <https://github.com/domenicquirl/cstree>,
  <https://docs.rs/cstree>
- Biome parser (hand-written, lossless CST via biome_rowan) —
  <https://deepwiki.com/biomejs/biome/6.1-parser-architecture>, <https://docs.rs/biome_rowan>,
  <https://biomejs.dev/internals/architecture/>
- swc/oxc are fast AST parsers (not lossless CST) —
  <https://github.com/oxc-project/bench-javascript-parser-written-in-rust>, <https://oxc.rs/docs/guide/benchmarks>
- chumsky (combinator, recovery, Python-style indentation) — <https://docs.rs/chumsky>,
  <https://github.com/zesterer/chumsky>, <https://blog.jsbarretto.com/post/parser-combinators-and-error-recovery>
- logos (fast lexer) — <https://github.com/maciejhirsz/logos>, <https://docs.rs/logos>
- lalrpop / pest — <https://github.com/lalrpop/lalrpop>, <https://docs.rs/pest>,
  <https://rustprojectprimer.com/ecosystem/parsing.html>
