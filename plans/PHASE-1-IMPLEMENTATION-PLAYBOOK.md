# gdscript-analyzer Phase 1 — Execution Playbook (Parser & Syntax MVP)

> Synthesized from current-API research + adversarial verdicts. Supersedes parts of `PHASE-1-PARSER-AND-SYNTAX-MVP.md` (corrections in §8). Every open decision is resolved here. Implement directly from this.
>
> Workspace facts: Rust edition 2024, MSRV **1.88.0**, single `0.x` version, `MIT OR Apache-2.0`. Local toolchain `stable-x86_64-pc-windows-gnu` (mingw, no `link.exe`). Core crates (`base`, `syntax`, `db`, `hir`, `ide`) must `cargo check --target wasm32-unknown-unknown`.

---

## 1. Confirmed dependency table

All versions verified mid-2026. License column flags anything cargo-deny-relevant; all are in the allowed family (MIT/Apache/BSD/ISC/Zlib/Unicode/MPL). **The only single-licensed crate is `insta` (Apache-2.0 only)** — still allowed, note it in `deny.toml` review.

### Runtime dependencies (enter the shipped graph)

| Crate | Pin | Crate(s) used in | wasm32-unknown-unknown | MSRV / edition | gnu | SPDX | Notes |
|---|---|---|---|---|---|---|---|
| `cstree` | `0.14` (0.14.0) | `gdscript-syntax` | ✅ (no upstream proof — **gate in week 1**) | 1.85 / 2024 | ✅ | MIT OR Apache-2.0 | `derive` feature ON; `parking_lot` is an **unconditional** transitive dep (the wasm watch item). Keep `multi_threaded_interning`/`lasso_compat` OFF. |
| `cstree_derive` | `0.14` | (via `cstree` `derive`) | ✅ (proc-macro, host) | 1.85 / 2024 | ✅ | MIT OR Apache-2.0 | Pulled by `cstree`'s `derive` feature. |
| `logos` | `0.16` (0.16.1) | `gdscript-syntax::lexer` | ✅ | 1.80 / 2021 | ✅ | MIT OR Apache-2.0 | Pure Rust, no build.rs. `0.16` rejects unbounded greedy `.*`/`.+` without `allow_greedy`. |
| `text-size` | `1.1` (1.1.1) | `gdscript-base`, `syntax` | ✅ | — | ✅ | MIT OR Apache-2.0 | `TextSize`/`TextRange` POD. Also a cstree transitive dep — reuse it directly. |
| `rustc-hash` | `2.1` | core crates (FxHashMap) | ✅ | — | ✅ | MIT OR Apache-2.0 | Deterministic hasher → keeps `getrandom` OUT of the graph. |
| `serde` | `1` | `gdscript-base` (POD derives) | ✅ | — | ✅ | MIT OR Apache-2.0 | `features=["derive"]`. |
| `napi` | `3` (3.9.3) | `gdscript-ffi` only | ❌ (native + wasip1-threads) | **1.88.0** / 2024 | ✅ (best-effort, PR #2026) | MIT | Default features keep `napi4` + `dyn-symbols` (gnu `.node` loading). |
| `napi-derive` | `3` | `gdscript-ffi` only | ❌ | 1.88.0 | ✅ | MIT | |
| `wasm-bindgen` | `0.2.125` | `bindings/wasm` only | ✅ (this IS the wasm path) | 0.2 lib MSRV 1.77 / CLI 1.86 | ✅ | MIT OR Apache-2.0 | Browser path. |
| `serde-wasm-bindgen` | `0.6.5` | `bindings/wasm` only | ✅ | 1.66 / 2021 | ✅ | MIT | `to_value`/`from_value`. Feature-complete, not abandoned. |
| `console_error_panic_hook` | `0.1.7` | `bindings/wasm` only | ✅ | — | ✅ | MIT OR Apache-2.0 | Panic backtraces in browser console. |

### Build / dev dependencies (never in the shipped or wasm graph)

| Crate | Pin | Used in | MSRV / edition | gnu | SPDX | Notes |
|---|---|---|---|---|---|---|
| `napi-build` | `2` (2.3.2) | `gdscript-ffi` build.rs | — | ✅ | MIT | **README says `"1"` — stale. Pin `"2"`.** `napi_build::setup()`. |
| `expect-test` | `1.5` (1.5.1) | `syntax` dev-deps | 1.60 / 2018 | ✅ | MIT OR Apache-2.0 | PRIMARY golden mechanism (rust-analyzer's). |
| `proptest` | `1.11` (1.11.0) | `syntax` dev-deps | 1.85 / 2021 | ✅ | MIT OR Apache-2.0 | PRIMARY robustness harness. **Leave `fork`/`timeout` features OFF** (defaults are in-process). |
| `tree-sitter` | `0.26` (0.26.9) | `syntax` dev-deps, `optional` | **1.90** / 2024 | ✅ (needs mingw gcc on PATH) | MIT | **MSRV 1.90 > our 1.88** → MUST stay behind a feature in dev-deps; never on MSRV/wasm lane. |
| `tree-sitter-gdscript` | `6.1` (6.1.0) | `syntax` dev-deps, `optional` | 2021 | ✅ | MIT | Compiles `parser.c` + `scanner.c` via `cc` → native-only. |
| `insta` | `1.48` | `syntax` dev-deps, OPTIONAL | 1.66 / 2021 | ✅ | **Apache-2.0 ONLY** | Only if you want JSON snapshot redactions; else skip. |
| `cargo-fuzz` + `libfuzzer-sys` `0.4` + `arbitrary` `1.4` | — | separate `fuzz/` crate | nightly | ❌ on Windows | MIT OR Apache-2.0 | **CI-only (ubuntu+nightly).** Cannot run on stable windows-gnu. |

### `[workspace.dependencies]` lines (paste into root `Cargo.toml`)

```toml
[workspace.dependencies]
# --- runtime, core (wasm-clean) ---
cstree      = { version = "0.14", features = ["derive"] }
logos       = "0.16"
text-size   = "1.1"
rustc-hash  = "2.1"
serde       = { version = "1", features = ["derive"] }

# --- FFI: Node addon (napi) — native + wasip1-threads only ---
napi        = { version = "3", default-features = true }   # keeps napi4 + dyn-symbols
napi-derive = "3"
napi-build  = "2"                                          # NOT "1" (README is stale)

# --- FFI: browser (wasm-bindgen) ---
wasm-bindgen             = "0.2.125"
serde-wasm-bindgen       = "0.6.5"
console_error_panic_hook = "0.1.7"

# --- dev / test ---
expect-test = "1.5"
proptest    = "1.11"

# --- oracle: native-only, feature-gated, MSRV-1.90 (kept off MSRV/wasm lanes) ---
tree-sitter          = { version = "0.26", optional = true }
tree-sitter-gdscript = { version = "6.1",  optional = true }
```

In `gdscript-syntax/Cargo.toml`, the wasm-clean dependency on cstree must drop `std` for the wasm build path:

```toml
[dependencies]
cstree = { workspace = true }                 # native: ["derive"] from workspace
# the wasm core build is exercised by `cargo check --target wasm32-unknown-unknown`;
# cstree keeps `std` by default which is fine on wasm32-unknown-unknown (std subset).
# Only flip to default-features=false + a global allocator if a strict no_std target is added.

[features]
tree-sitter-oracle = ["dep:tree-sitter", "dep:tree-sitter-gdscript"]

[dev-dependencies]
expect-test = { workspace = true }
proptest    = { workspace = true }
tree-sitter          = { workspace = true }    # optional → only built with the feature
tree-sitter-gdscript = { workspace = true }
```

---

## 2. Resolved architecture decisions

- **CST backend = cstree 0.14.** *CONFIRMED.* `Syntax` trait (`from_raw`/`into_raw`/`static_text`), `GreenNodeBuilder`, interned token text, Send+Sync red `SyntaxNode`, byte-for-byte lossless round-trip all verified. **DO:** one `#[repr(u32)]` `SyntaxKind` enum, `#[derive(Syntax)]`, build via `GreenNodeBuilder`, finish with `new_root_with_resolver`, store the interner in `Parse`.
  - **Correction (load-bearing):** `RawSyntaxKind` is `pub struct RawSyntaxKind(pub u32)` — **u32, not u16.** Annotate `#[repr(u32)]`. The existing plan's `#[repr(u16)]` is wrong (see §8).

- **The napi(Node) / wasm-bindgen(browser) split — STATED UNAMBIGUOUSLY.** *CONFIRMED (3/0).* napi-rs v3's only wasm target is `wasm32-wasip1-threads`, which in a browser needs `@napi-rs/wasm-runtime` (~308 KB), `SharedArrayBuffer`, COOP/COEP cross-origin-isolation headers, and effectively a bundler. **It is NOT a drop-in for a static `hello.html`.** Therefore:
  - **`gdscript-ffi` (crate, napi-rs v3, `crate-type=["cdylib"]`) produces the Node `.node` addon → `bindings/node/hello.mjs`.** This is the LSP/CLI path. Native (and, much later/optionally, a wasip1-threads StackBlitz fallback). **Excluded from the wasm32-unknown-unknown CI gate.**
  - **`bindings/wasm` (crate, wasm-bindgen + `wasm-pack build --target web`, `crate-type=["cdylib","rlib"]`) produces the browser artifact → `playground/hello.html`.** Single-threaded, `wasm32-unknown-unknown`, no SAB, no COOP/COEP, no runtime polyfill. Loads via `<script type="module">` + `init()`.
  - Both crates depend on the same wasm-clean `gdscript-ide`. Neither bindings crate is "core."

- **tree-sitter = optional, native-only, feature-gated oracle (never load-bearing).** *CONFIRMED (2/0, 1 "uncertain" only nitpicking the word "CANNOT").* It ships `parser.c` + a real C external `scanner.c` compiled via `cc-rs`; with the stock build it does not target `wasm32-unknown-unknown`. **DO:** gate behind `tree-sitter-oracle` feature in `[dev-dependencies]`; never reachable from `gdscript-ide` or the MSRV/wasm lanes. (A `tree-sitter-wasm-build-tool` path technically exists for C scanners, but it is irrelevant — we deliberately keep it native dev-only.)

- **logos as a LOSSLESS lexer.** *CONFIRMED (3/0).* Skipping is opt-in; **never** use `#[logos(skip ...)]`/`Skip`. Define `Whitespace`/`Newline`/comment variants as ordinary `#[regex]`/`#[token]`. `span()`/`slice()` give byte ranges over `&str`. **Caveat:** logos does not guarantee total coverage — any unmatched byte yields `Err`; add an explicit `ERROR`/catch path so the stream stays gap-free.

- **Robustness harness on stable Windows-gnu = proptest, not cargo-fuzz.** *CONFIRMED (2/1, the refute only corrected "Linux/macOS-centric" — cargo-fuzz supports Windows-**MSVC**, never gnu).* `cargo-fuzz`/libFuzzer needs nightly + MSVC AddressSanitizer; **gnu is unsupported.** **DO:** `proptest` in ordinary `#[test]`s locally (in-process default config); confine `cargo-fuzz` to an ubuntu+nightly CI job.

- **Golden mechanism = expect-test** (rust-analyzer's). CST printer = cstree's built-in `SyntaxNode::debug(&resolver, true)`. Differential = `xtask differential` over non-trivia skeletons with a `KNOWN_DIVERGENCES.md` allowlist.

- **getrandom = absent.** Core hashers stay on `rustc-hash`/hashbrown-`foldhash` (cstree's default). Add a CI `cargo tree -i getrandom --target wasm32-unknown-unknown` assertion that it is ABSENT. Only if it ever appears, add `getrandom = { version="0.4", features=["wasm_js"] }` gated to `bindings/wasm` (the `wasm_js` Cargo feature alone suffices on 0.3.2+/0.4 — the old `--cfg getrandom_backend` RUSTFLAG is dead).

- **Allocator:** leave default `dlmalloc`; set NO `#[global_allocator]`; reject `wee_alloc`.

---

## 3. Per-workstream implementation notes (WS1–WS7)

### WS1 — Lexer (`gdscript-syntax::lexer`)

**Module:** `src/lexer.rs` (+ `src/lexer/strings.rs` for the string callbacks). API: `pub fn tokenize(src: &str) -> Vec<RawToken>` where `RawToken { kind: SyntaxKind, range: TextRange }`.

Define a `logos`-deriving `LexKind` (1:1 mappable into `SyntaxKind`), trivia as first-class variants:

```rust
use logos::{Logos, Lexer, Filter};

#[derive(Default, Debug, Clone, PartialEq)]
pub enum LexError { UnterminatedString, InvalidNumber, #[default] UnexpectedChar }

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(error = LexError)]          // diagnostics carried; NEVER aborts
pub enum LexKind {
    #[regex(r"[ \t]+")]            Whitespace,
    #[regex(r"\r\n|\n|\r")]        NewlinePhys,      // WS2 consumes/transforms
    #[token("\\\n")]              LineContinuation,
    #[regex(r"#region[^\n]*",    priority = 3)] RegionComment,
    #[regex(r"#endregion[^\n]*", priority = 3)] EndRegionComment,
    #[regex(r"##[^\n]*",         priority = 2)] DocComment,
    #[regex(r"#[^\n]*",          priority = 1)] LineComment,

    #[regex(r"[0-9][0-9_]*")]                              IntDec,
    #[regex(r"0[xX][0-9a-fA-F_]+")]                        IntHex,
    #[regex(r"0[bB][01_]+")]                               IntBin,
    #[regex(r"[0-9][0-9_]*\.[0-9_]*([eE][+-]?[0-9_]+)?")] FloatTrailing,
    #[regex(r"\.[0-9][0-9_]*([eE][+-]?[0-9_]+)?")]        FloatLeading,
    #[regex(r"[0-9][0-9_]*[eE][+-]?[0-9_]+")]             FloatExp,

    // strings via CALLBACKS (greedy .* won't compile in 0.16):
    #[token("\"\"\"", multiline_string)]  StringMlD,
    #[token("'''",    multiline_string)]  StringMlS,
    #[regex("\"[^\"\\n]*\"")]             StringD,
    #[regex("'[^'\\n]*'")]                StringS,
    #[regex(r#"r"[^"\n]*""#)]             RawStringD,
    #[regex(r#"&"[^"\n]*""#)]             StringName,
    #[regex(r#"\^"[^"\n]*""#)]            NodePath,

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]   Ident,   // keywords reclassified post-lex (see below)
    // punctuation/operators as #[token(...)] (literals out-prioritize Ident automatically)
}
```

**Gotchas:**
- **0.16 greedy-dot:** do NOT use `"""(.*?)"""`. Multiline/raw/`&`/`^` strings use a callback scanning `lex.remainder()` + `lex.bump(n)` to the closing delimiter; on unterminated, `bump` to EOF and `Filter::Emit` (still a token → lossless) or `FilterResult::Error(LexError::UnterminatedString)`.
- **Keyword vs identifier:** simplest robust path = lex everything matching the ident regex as `Ident`, then a tiny `reclassify(text) -> SyntaxKind` post-step maps the ~35-word keyword table (WS2/§Godot table). `true`/`false`/`null` → `LITERAL` (Godot does NOT keyword them); `PI/TAU/INF/NAN` → their own const kinds; `namespace`/`trait` → reserved keyword kinds. This avoids same-priority ties.
- **Spans → u32:** `lex.span()` is `Range<usize>`; convert to `TextSize`/`u32` at the boundary (assert source < 4 GiB).
- **Losslessness guardrail (test):** `concat(slice for every token) == src`.
- `extras = ()` — indentation state lives in WS2, not the lexer.

### WS2 — Indentation pre-pass (`gdscript-syntax::prepass`)

**Module:** `src/prepass.rs`. API: `pub fn run(tokens: Vec<RawToken>, src: &str) -> (Vec<RawToken>, Vec<IndentDiagnostic>)`. Model on **Godot's `gdscript_tokenizer.cpp`**, NOT tree-sitter's `scanner.c`.

**CORRECTION (load-bearing): tab width = `tab_size` default 4, NOT 8.** Godot: `int tab_size = 4`. tree-sitter scanner.c hardcodes 8 — do not copy it. Parameterize `tab_size: u32 = 4`. (Within a consistently-indented file any positive width yields the same structure; the divergence only shows on mixed lines, which Godot errors anyway — so 4 is safe and matches the engine; differential disagreements vs tree-sitter here are EXPECTED and allowlisted.)

State machine fields (mirroring the engine):

```rust
struct Layout {
    tab_size: u32,                 // = 4
    indent_stack: Vec<u32>,        // column counts, base [0]
    saved_stacks: Vec<Vec<u32>>,   // indent_stack_stack — for lambda bodies (mid-expr blocks)
    paren_depth: u32,              // () [] {} => multiline_mode (significance suppressed)
    line_continuation: bool,       // trailing `\`
    indent_char: Option<IndentChar>, // FIRST-IN-FILE-WINS, sticky whole file
    pending_newline: bool,
    pending_indents: i32,          // >0 INDENTs, <0 DEDENTs queued
}
```

Rules (each a golden corpus case):
- **`check_indent`** runs only on physical lines that are NOT blank, comment-only, inside brackets, a `\`-continuation, or multiline-string interiors. Column: `+tab_size` per tab, `+1` per space.
- **Two distinct errors:** same-line tab+space → `"Mixed use of tabs and spaces for indentation."`; cross-line deviation from `indent_char` → `"Used %s character for indentation instead of %s as used before in the file."`. Both are diagnostics + **recover** (never abort).
- **Blank/comment-only lines:** advance bookkeeping, emit NO `NEWLINE`/`INDENT`/`DEDENT` (the classic spurious-DEDENT bug). Column-0 comments inside a body must not close scope.
- **Bracket suppression** (`paren_depth>0`) and **`\` continuation** are DISTINCT — track separately.
- **Lambdas:** an inline/multiline lambda body starting mid-expression saves/restores the indent stack (`saved_stacks`). A flat single stack is insufficient.
- **Drain order before each real token:** DEDENTs/INDENTs, then NEWLINE, then the token.
- **EOF:** emit pending NEWLINE, pop indent_stack to 0 (one DEDENT each), then EOF.
- **Synthetic tokens are ZERO-WIDTH:** `NEWLINE`/`INDENT`/`DEDENT` carry NO source bytes (the real bytes live in the `Whitespace`/`NewlinePhys` trivia tokens that remain in the stream). Do not give them text or round-trip breaks.

12-case golden corpus stays as in the existing plan (`fixtures/lexer-prepass/*.tokens`), with tab cases re-blessed for tab=4.

### WS3 — Parser → cstree (`gdscript-syntax::parser`)

**Module layout:** `src/syntax_kind.rs` (the enum + `Syntax` derive), `src/parser/event.rs` (Event + Marker), `src/parser/grammar.rs` (the `parse_*` fns + Pratt), `src/parser/sink.rs` (event→cstree builder), `src/parse.rs` (`Parse`, public `parse()`).

**Event model = matklad's flat shape** (simplest correct, 1:1 onto cstree). Adopt rust-analyzer's `Marker` + `DropBomb` ergonomics. **Use cstree's `checkpoint()`/`start_node_at()` for retroactive wrapping — NOT `open_before` vector-insert, NOT rust-analyzer's `forward_parent`/tombstone.**

```rust
enum Event { Open { kind: SyntaxKind }, Close, Advance }
struct MarkOpened { index: usize }
struct MarkClosed { index: usize }
// fuel: Cell<u32> in Parser; nth() decrements & panics at 0, advance() resets to 256.
```

Parser walks **non-trivia only**: keep `nontrivia: Vec<usize>` (indices into the full token list); `nth()`/`at()` index through it. Trivia is re-attached at build time (the sink).

**Pratt expression parser — port Godot's ladder with TWO quirks encoded:**
- `**` (power) is **LEFT-associative** in GDScript (NOT Python's right). Encode `(lbp, rbp)` with `lbp < rbp`.
- `as` (cast) sits at the **low `PREC_CAST` slot** (between assignment and ternary). Ternary `x if c else y` is **right-assoc**; assignment lowest + right-assoc. `is`/`is not` high; `in`/`not in` between comparison and logic-not; `await`/unary `-`/`not`/`~` prefix; call `()`/index `[]`/field `.` postfix via checkpoint promotion.

**Recovery (resilient-LL, never returns `Result`):** `parse() -> (GreenNode, interner, Vec<SyntaxError>)`. Every list/loop body chooses parse-element / `advance_with_error` (skip one token → `ERROR_NODE`) / `break` (bubble up), gated on FIRST/recovery sets. Statement resync set: `NEWLINE | DEDENT | {func var const class enum signal static @ if for while match return ...}`. Block recovery rides the prepass: a block is `INDENT … DEDENT`, so a malformed statement still terminates at the matching DEDENT.

Grammar-production checklist (each → a `parse_*` fn) is unchanged from the existing plan §WS3 table — port it verbatim.

### WS4 — Typed AST (`gdscript-syntax::ast`)

**Module:** `src/ast.rs` (+ `src/ast/nodes.rs`). Zero-cost typed view; `AstNode { can_cast, cast, syntax }`. Unchanged in design from the existing plan, with ONE plumbing consequence of cstree interning:

**Text accessors need the resolver.** `Name::text()` etc. must take/hold the resolver. Store it in `Parse`; expose `Parse::syntax_node()` (cheap) and have AST text methods resolve through the stored interner. e.g. `FuncDecl::name(&self) -> Option<Name>` returns a node; `Name::text(&self, resolver) -> &str` (or wrap a resolver-carrying view). Plan the AST layer around this — it is the single biggest ergonomic difference from rowan.

### WS5 — tree-sitter oracle (`#[cfg(feature = "tree-sitter-oracle")]`)

**Module:** `src/oracle.rs` (gated). `pub struct TreeSitterParser { parser: tree_sitter::Parser }`; `Parser::new()` then `parser.set_language(&tree_sitter_gdscript::LANGUAGE.into())` (**`.into()` is required** — `LANGUAGE` is a `LanguageFn`, not `fn language()`). `parser.parse(src, None)` → `Tree`.

`oracle_shape(&Tree) -> Vec<(depth, &'static str, Range<usize>)>` flattens via a `TreeCursor` DFS, **skipping `extras`/`comment`** (tree-sitter models comments as floating extras — excluding them avoids false mismatches). Map by `node.kind()` **string** (stable), never `kind_id()` (unstable across grammar versions). node-type → SyntaxKind table (from the existing research):

```
source→SOURCE_FILE  class_definition→CLASS_DECL  function_definition→FUNC_DECL
variable_statement→VAR_DECL  const_statement→CONST_DECL  signal_statement→SIGNAL_DECL
enum_definition→ENUM_DECL  if_statement→IF_STMT  elif_clause→ELIF_CLAUSE  else_clause→ELSE_CLAUSE
for_statement→FOR_STMT  while_statement→WHILE_STMT  match_statement→MATCH_STMT
call→CALL_EXPR  binary_operator→BIN_EXPR  unary_operator→UNARY_EXPR  subscript→INDEX_EXPR
attribute→FIELD_EXPR  lambda→LAMBDA_EXPR  identifier→NAME_REF  integer|float|string→LITERAL
comment→(trivia; excluded)  ERROR→ERROR_NODE
```

**Local note:** running oracle tests on windows-gnu requires mingw `gcc` on PATH (or `CC_x86_64-pc-windows-gnu`). CI Linux/macOS have it; keep these tests OFF the wasm and MSRV-1.88 lanes.

### WS6 — ide skeleton (`gdscript-ide`)

Unchanged in shape from the existing plan: plain `RootDatabase { files, parsed }` (no salsa), `AnalysisHost`/`Analysis`, four real features (`diagnostics` = parse errors only, `document_symbols`, `folding_ranges`, by-name `completions`), rest stubbed `Ok(empty)`. Derived computations as pure `(db, file) -> value` free functions. POD result types in `gdscript-base` (serde, byte offsets, `LineIndex` carrying byte↔UTF-16). **Must `cargo check --target wasm32-unknown-unknown`.** Keep `proptest`/`expect-test` strictly in `[dev-dependencies]` so they never touch this gate.

### WS7 — FFI (BOTH bindings)

**`gdscript-ffi` (Node `.node`, napi-rs v3):**

```rust
// crate-type = ["cdylib"]; build.rs: napi_build::setup();
use napi_derive::napi;
use gdscript_ide::AnalysisHost;

#[napi(object)]                       // POD, all fields pub, crosses by clone → TS interface
pub struct DocumentSymbol { pub name: String, pub kind: u32,
    pub detail: Option<String>, pub start: u32, pub end: u32 }

#[napi] pub struct AnalysisHandle { host: AnalysisHost }   // class, NOT object

#[napi]
impl AnalysisHandle {
    #[napi(constructor)] pub fn new() -> Self { Self { host: AnalysisHost::new() } }
    #[napi] pub fn apply_change(&mut self, file_id: u32, text: Option<String>) { /* ... */ }
    #[napi] pub fn document_symbols(&self, file_id: u32) -> napi::Result<Vec<DocumentSymbol>> { /* ... */ }
    // diagnostics / folding_ranges / completions similarly
}
```
`Vec<#[napi(object)]>` → JS `Array<{...}>` directly (no serde/JSON-string). `Result<T>` unwraps; `Err` throws. Build: `napi build --platform --release` → `*.node` + `index.js` (gnu-aware loader, PR #2026) + `index.d.ts`. `bindings/node/hello.mjs` imports it, `applyChange` + prints `documentSymbols`. (Local gnu `.node` is best-effort; publish via CI MSVC runners.)

**`bindings/wasm` (browser, wasm-bindgen):**

```rust
// crate-type = ["cdylib","rlib"]; deps: gdscript-ide, wasm-bindgen, serde-wasm-bindgen, console_error_panic_hook
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)] fn start() { console_error_panic_hook::set_once(); }

#[wasm_bindgen] pub struct WasmAnalysis { host: gdscript_ide::AnalysisHost }

#[wasm_bindgen]
impl WasmAnalysis {
    #[wasm_bindgen(constructor)] pub fn new() -> WasmAnalysis { /* ... */ }
    #[wasm_bindgen(js_name = applyChange)]
    pub fn apply_change(&mut self, file_id: u32, text: String) { /* ... */ }
    #[wasm_bindgen(js_name = documentSymbols)]
    pub fn document_symbols(&self, file_id: u32) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&symbols).map_err(|e| e.into())   // plain JS array/objects
    }
}
```
Build: `wasm-pack build --target web --out-dir playground/pkg --out-name gdscript bindings/wasm`. `playground/hello.html`: `import init, { WasmAnalysis } from './pkg/gdscript.js'; await init();`. **"No server" caveat:** `file://` is blocked by browser CORS on the ES-module import + the `_bg.wasm` `fetch`; serve `playground/` over any trivial static HTTP server (`python -m http.server`). This is a browser limitation, not an application/WASI server. Do byte→UTF-16 conversion on the JS side (or via `LineIndex`).

---

## 4. cstree tree-building deep-dive (the riskiest piece)

### 4.1 SyntaxKind + Syntax impl

`#[repr(u32)]` (NOT u16). Use the derive feature so `from_raw` never desyncs (hand-written `from_raw` panics on unknown discriminants).

```rust
use cstree::prelude::*;                         // Syntax, RawSyntaxKind, GreenNode, GreenNodeBuilder
use cstree::syntax::{ResolvedNode, SyntaxNode};
use cstree::interning::Interner;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Syntax)]
#[repr(u32)]
pub enum SyntaxKind {
    // trivia (variable text → interned, NO static_text)
    Whitespace, Comment, DocComment, RegionComment, EndRegionComment, LineContinuation,
    // synthetic structural (zero-width)
    Newline, Indent, Dedent,
    // literals / names
    Int, Float, String, StringName, NodePath, Ident,
    // keywords / punctuation (fixed lexeme → static_text)
    #[static_text("func")] FuncKw,
    #[static_text(":")]    Colon,
    #[static_text("->")]   Arrow,
    #[static_text("**")]   StarStar,
    #[static_text("(")]    LParen,
    #[static_text(")")]    RParen,
    // ... rest of the ~35 keywords + all operators/punctuation with #[static_text]
    Error,
    // nodes (NO static_text)
    SourceFile, FuncDecl, ParamList, Block, BinExpr, /* ... full node set ... */
    ErrorNode,
    Tombstone,            // keep last
}
pub type GdNode = ResolvedNode<SyntaxKind>;   // Display + .text() round-trip
```

- `#[static_text("…")]` ONLY on fixed-lexeme kinds (keywords, operators, punctuation). Return implicitly `None` for `Ident`/literals/trivia/comments/synthetic-structural/nodes — those go through `token(kind, text)` and get interned.
- `RawSyntaxKind(self as u32)`; `.0` to read.

### 4.2 Event → green-tree sink

cstree has **NO** built-in `TreeSink`/`build_recursive` — the sink loop is ours. It owns the raw token list (incl. trivia) and re-attaches trivia at `Open` boundaries using a `n_attached_trivias`-style heuristic (walk preceding trivia backwards; a `"\n\n"` blank line breaks attachment; `##` doc-comments pull to the FOLLOWING declaration).

```rust
pub struct Parse {
    pub green: GreenNode,
    pub interner: cstree::interning::TokenInterner,   // MUST keep — token text is interned
    pub errors: Vec<SyntaxError>,
}
impl Parse {
    pub fn syntax_node(&self) -> SyntaxNode<SyntaxKind> {
        SyntaxNode::new_root(self.green.clone())      // cheap, Send+Sync
    }
}

fn build(events: &[Event], tokens: &[RawToken], src: &str) -> Parse {
    let mut b = GreenNodeBuilder::<SyntaxKind>::new();
    let mut ti = 0usize;
    for ev in events {
        match *ev {
            Event::Open { kind } => {
                // (real impl: attach leading trivia here per n_attached_trivias)
                b.start_node(kind);
            }
            Event::Close => b.finish_node(),
            Event::Advance => {
                // flush leading trivia, then the meaningful token
                while ti < tokens.len() && is_trivia(tokens[ti].kind) { emit(&mut b, &tokens[ti], src); ti += 1; }
                if ti < tokens.len() { emit(&mut b, &tokens[ti], src); ti += 1; }
            }
        }
    }
    while ti < tokens.len() { emit(&mut b, &tokens[ti], src); ti += 1; }  // trailing trivia
    let (green, cache) = b.finish();
    let interner = cache.unwrap().into_interner().unwrap();   // Some because ::new() owns the cache
    Parse { green, interner, errors: /* ... */ vec![] }
}

fn emit(b: &mut GreenNodeBuilder<SyntaxKind>, t: &RawToken, src: &str) {
    if let Some(_) = t.kind.static_text() { b.static_token(t.kind); }   // fixed lexeme, no bytes stored
    else if is_zero_width(t.kind) { /* Newline/Indent/Dedent: skip OR emit empty — see note */ }
    else { b.token(t.kind, &src[t.range]); }                            // interned
}
```

**Retroactive wrapping (Pratt promotion / postfix call/index/field):** capture `let c = b.checkpoint()` before children, later `b.start_node_at(c, SyntaxKind::BinExpr)` then `finish_node()`. This replaces `open_before`/`forward_parent` entirely. (In the parser layer the Marker model expresses this; the sink replays it as a checkpoint — or, simplest, the grammar uses `open_before` on the event vector and the sink stays a flat Open/Close/Advance replay. Pick ONE: recommended = grammar emits matklad events, sink replays flat, and any retroactive wrap is done by `open_before` on the event Vec at parse time. cstree `checkpoint` is the alternative if you build directly without an event Vec.)

### 4.3 Resolver / interner handling

- `finish()` → `(GreenNode, Option<NodeCache>)`. With `GreenNodeBuilder::new()` the cache is `Some`; `cache.unwrap().into_interner().unwrap()` extracts the `TokenInterner`. (`None` only if you used `with_cache`/`with_interner` — be deliberate.)
- **Always** read text back via the resolver: either `SyntaxNode::new_root_with_resolver(green, interner) -> ResolvedNode` (gives `Display` + `.text()` directly) for round-trip tests, or keep the interner in `Parse` and call `token.resolve_text(&interner)` in the AST layer. A bare `new_root` cannot resolve identifier/literal text.
- **Shared interner across files:** use one project-wide `NodeCache` (`GreenNodeBuilder::with_cache(&mut cache)`) so identifiers/keywords intern once; store the resolver alongside each file's green tree.

### 4.4 Byte-for-byte round-trip test

```rust
#[test]
fn round_trips() {
    let parse = crate::parse(SRC);
    let root = SyntaxNode::<SyntaxKind>::new_root_with_resolver(
        parse.green.clone(), parse.interner.clone());
    assert_eq!(root.to_string(), SRC);   // Display == original source, byte-for-byte
}
```

**Zero-width synthetic token note (decide explicitly):** `Newline`/`Indent`/`Dedent` carry no bytes. If you emit them via `b.token(kind, "")` the round-trip still holds (empty contributes nothing) and they remain visible to grammar/folding. If you instead drop them at sink time, the grammar must consume them from the event stream only. **Recommended:** emit as empty-text tokens — keeps them in the tree for folding/structure while preserving `to_string() == src`, since the real newline/whitespace bytes are carried by the retained `Whitespace`/`NewlinePhys` trivia tokens.

---

## 5. Test & fixtures infrastructure

- **Golden tool = `expect-test` 1.5** with `expect_file!`, blessed by `UPDATE_EXPECT=1 cargo test`. Mirror rust-analyzer's `dir_tests`: walk `fixtures/<area>/*.gd` → produce a string → `expect_file![format!("{path}.cst")].assert_eq(&actual)`.
- **CST S-expr printer = cstree's built-in.** `pub fn dump_cst(node, resolver) -> String { node.debug(resolver, true) }` → rust-analyzer `.rast` shape (`KIND@start..end`, tokens `KIND@start..end "text"`, 2-space indent). For flat `.tokens` dumps write a 5-line `dump_tokens()` emitting `KIND@start..end "text"` per line. A trivia-filter flag on the same printer doubles as the differential skeleton serializer.
- **`xtask` subcommands:**
  - `cargo xtask fixtures --bless` → just sets `UPDATE_EXPECT=1` and shells out to `cargo test` (do NOT reimplement file IO/diff).
  - `cargo xtask differential` → parses each corpus file with both backends (requires `--features tree-sitter-oracle`), normalizes to non-trivia skeletons, `assert_eq`; accepted divergences recorded in `fixtures/differential/KNOWN_DIVERGENCES.md` (including the expected tab=4-vs-8 disagreements).
- **LOCAL panic-free robustness harness (stable Windows-gnu):** `proptest` in ordinary `#[test]`s, default `ProptestConfig` (`fork:false`, `timeout:0` → in-process, mingw-clean). Three strategies: `s in "\\PC*"` (text), `any::<Vec<u8>>()` → `from_utf8_lossy` (invalid-UTF-8/truncation), `prop::sample::select(corpus)` + mutation. Body asserts `parse()` does not panic. **Commit `proptest-regressions/`** as the permanent regression corpus. The fuel `Cell<u32>` guard turns any grammar loop into an immediate panic the harness catches.
- **`cargo-fuzz` lives ONLY in a separate `fuzz/` crate, run on ubuntu+nightly CI** (`fuzz_target!(|data: &[u8]| { let _ = gdscript_syntax::parse(&String::from_utf8_lossy(data)); })`). Never wired into the local flow. Crashers feed back as `proptest-regressions/` seeds or `fixtures/` recovery cases.
- **Two separate parser invariants:** round-trip (`to_string()==src`, plain `assert_eq`) vs CST shape (`expect_file!` golden). Don't conflate.
- **CRLF/`.gitattributes`:** normalize `\r\n`→`\n` in the harness AND mark `fixtures/** -text` (or `eol=lf`) so Windows-local and ubuntu-CI goldens stay byte-identical.

---

## 6. Concrete build order (each step ends in a verifiable gate)

1. **SyntaxKind + Syntax derive.** Define the full `#[repr(u32)]` enum; `#[derive(Syntax)]`; `#[static_text]` on fixed lexemes.
   → `cargo test -p gdscript-syntax syntax_kind::round_trips` (the 3-node `func foo` tree round-trips byte-for-byte).
2. **wasm gate, immediately.** Add `gdscript-syntax` + cstree, a trivial `pub fn parse(&str)` stub.
   → `cargo check -p gdscript-ide --target wasm32-unknown-unknown` green (converts the parking_lot/cstree-wasm assumption into fact). Add `cargo tree -i getrandom --target wasm32-unknown-unknown` → asserts ABSENT.
3. **WS1 lexer.** logos enum + string callbacks + keyword reclassify.
   → `cargo test -p gdscript-syntax lexer` incl. lossless `concat(slices)==src`.
4. **WS2 prepass.** Engine-faithful state machine, tab=4.
   → `cargo test -p gdscript-syntax prepass` — all 12 golden `.tokens` cases green (re-blessed for tab=4).
5. **WS3 sink + tiny grammar.** Event model + Marker/DropBomb + fuel guard; sink driving `GreenNodeBuilder`; parse `SourceFile`→`FuncDecl` only.
   → `cargo test -p gdscript-syntax parser::round_trip` green on a hand-written `.gd`.
6. **WS3 Pratt + full grammar.** Port Godot's precedence ladder (`**` left, `as` low); all `parse_*` fns.
   → `cargo test -p gdscript-syntax goldens` — every grammar-row `.cst` golden green; lossless round-trip over the whole corpus.
7. **WS3 recovery + robustness.**
   → broken-code fixtures assert tree+errors+sibling-survival; `cargo test -p gdscript-syntax robustness` (proptest, panic-free) green; `proptest-regressions/` committed.
8. **WS4 AST.**
   → `cargo test -p gdscript-syntax ast` — accessors return correct children/text (via resolver) on golden trees.
9. **WS5 oracle (gated).**
   → `cargo test -p gdscript-syntax --features tree-sitter-oracle differential` green modulo `KNOWN_DIVERGENCES.md`. (Confirm mingw gcc on PATH locally.)
10. **WS6 ide.** `AnalysisHost`/`Analysis`; four real features; POD types in `gdscript-base`.
    → feature goldens green; `cargo check -p gdscript-ide --target wasm32-unknown-unknown` still green.
11. **WS7a Node FFI.** `gdscript-ffi` (napi), `napi-build = "2"`.
    → `napi build --platform --release` produces `*.node`; `node bindings/node/hello.mjs` prints document symbols.
12. **WS7b browser FFI.** `bindings/wasm` (wasm-bindgen).
    → `wasm-pack build --target web --out-dir playground/pkg --out-name gdscript bindings/wasm` succeeds; `playground/hello.html` over `python -m http.server` renders the symbol list.
13. **CI assembly.** fmt + clippy(-D, all+pedantic) + test matrix (ubuntu/macos/windows) + MSRV-1.88 (NO tree-sitter feature) + wasm32 check of `gdscript-ide` + cargo-deny + ubuntu-nightly `cargo-fuzz` job + the `getrandom`-absent assertion + the differential job.
    → `cargo xtask ci` green locally and in Actions.

---

## 7. Updated risk list (newly surfaced by the research)

| Risk (not fully captured by the existing plan) | Sev | Mitigation |
|---|---|---|
| **cstree wasm32 unproven upstream** (docs.rs built no wasm target for 0.14.0; `parking_lot` is an unconditional dep). | High | **Build-order step 2** runs the wasm gate before any parser work; if it fails, switch cstree to `default-features=false` + global allocator (still keep `derive`, keep `multi_threaded_interning` OFF). |
| **napi-rs wasm ≠ browser** — if anyone tries to ship the napi wasm target as `hello.html` it fails (needs SAB/COOP-COEP/308KB runtime/bundler). | High | Hard split: `gdscript-ffi`=Node only, `bindings/wasm`=browser only; `gdscript-ffi` excluded from the wasm32 gate; documented in ADR. |
| **tree-sitter MSRV 1.90 > workspace 1.88** — a normal dep would break the MSRV CI lane. | High | `optional` + `dev-dependencies` + `tree-sitter-oracle` feature; CI asserts the feature is OFF on MSRV and wasm lanes. |
| **logos 0.16 greedy-dot compile error** — naive `"""(.*?)"""` won't compile. | Med | Hand-written `remainder()`-scanning callbacks for all multiline/prefixed strings (also gives unterminated recovery). |
| **Tab width 4 vs 8** — copying tree-sitter's `+8` diverges from the engine on mixed/edge files. | Med | `tab_size=4` (engine default), parameterized; tab=8 differential disagreements pre-allowlisted in `KNOWN_DIVERGENCES.md`. |
| **cstree interning forces resolver plumbing** through the AST text layer (bigger than the plan implies). | Med | Store interner in `Parse`; AST text accessors take/hold the resolver; use `new_root_with_resolver` in tests. |
| **cargo-fuzz unusable on stable windows-gnu** (nightly + MSVC ASan only). | Med | proptest is the local harness; cargo-fuzz is CI-only (ubuntu/nightly). |
| **CRLF golden drift** between Windows-local and ubuntu-CI. | Med | LF-normalize in harness + `.gitattributes` `fixtures/** -text`. |
| **Lambda mid-expression blocks** need a stack-of-stacks, not a flat indent stack. | Med | `saved_stacks: Vec<Vec<u32>>` in WS2 (mirrors Godot's `indent_stack_stack`). |
| **insta single-license (Apache-2.0 only)** asymmetry. | Low | Prefer expect-test everywhere; if insta is used for JSON goldens, note it explicitly in `deny.toml` review. |
| **Local mingw `.node` is least-CI-tested** napi path. | Low | Treat local gnu `.node` as dev-convenience; publish binaries via CI MSVC runners. |

---

## 8. Corrections to `PHASE-1-PARSER-AND-SYNTAX-MVP.md`

1. **`SyntaxKind` repr — line ~341 (`#[repr(u16)]`).** cstree's `RawSyntaxKind` is `pub struct RawSyntaxKind(pub u32)` (u32, not rowan's u16). **Change to `#[repr(u32)]`.** Comment "count drives the u16<->kind cast" → u32.
2. **Prepass tab width — lines ~159, ~187–191 (`tab = 8`, `col += 8 - (col % 8)`).** Godot uses `tab_size` default **4**, not tree-sitter's 8. **Change to a parameterized `tab_size: u32 = 4`; column = `+tab_size` per tab, `+1` per space (no tab-stop modulo — Godot adds a flat `tab_size`).** Note the tab=8 reference to scanner.c is the wrong oracle for this rule.
3. **Prepass needs a stack-of-stacks for lambdas.** The pseudocode's single `indent_stack` is insufficient for inline/multiline lambda bodies starting mid-expression. Add `saved_stacks: Vec<Vec<u32>>` (Godot's `indent_stack_stack`).
4. **Prepass error model.** The plan mentions one `MIXED_INDENT` diagnostic; Godot has **two distinct** errors (same-line tabs+spaces vs cross-line first-wins `indent_char`). Model both.
5. **`true`/`false`/`null` are NOT keywords (lines ~112, ~356).** Godot tokenizes them as `LITERAL` in the identifier path. Reclassify them to a literal kind, not `TRUE_KW`/`FALSE_KW`/`NULL_KW`. Add reserved-but-unused `namespace`/`trait` keyword kinds. `PI/TAU/INF/NAN` get their own const kinds (engine: `CONST_PI` etc.), not `IDENT`.
6. **FFI/WASM strategy — §WS7 and architecture §4.** "napi-rs v3 compiles the same source to a Node `.node` addon **and** a wasm build" is misleading for Phase 1: the napi wasm target is `wasm32-wasip1-threads` and is **not** usable for the plain-browser `hello.html`. **Replace:** `gdscript-ffi` (napi) builds the Node `.node` / `hello.mjs`; a **separate `bindings/wasm` (wasm-bindgen + `wasm-pack --target web`) builds the browser `hello.html`.** The architecture's existing `crates/wasm` fallback is now the PRIMARY browser path for Phase 1, not a "decide later" fallback.
7. **`napi-build` version.** Pin **`"2"`** (2.3.2), not the README's stale `"1"`.
8. **logos pin.** `"0.16"` (0.16.1), not "0.15.x (confirm)". Note the `allow_greedy`/string-callback requirement.
9. **cstree pin.** `"0.14"` (0.14.0), not "0.12.x (confirm)". `GreenNodeBuilder` has no `build_recursive`/`TreeSink`; the event sink is ours. `finish() -> (GreenNode, Option<NodeCache>)`; extract interner via `cache.unwrap().into_interner().unwrap()`.
10. **Round-trip test must use the resolver.** `parse(src).syntax_node().to_string()==src` only holds when the node carries the interner — use `new_root_with_resolver` (or keep the interner in `Parse`). The bare `syntax_node()` (`new_root`) cannot resolve interned identifier/literal text.
11. **Robustness row (§Testing, line ~782).** "`cargo fuzz` / corpus-mutation" implies cargo-fuzz runs locally — it cannot on stable windows-gnu. **Primary local harness = proptest in `#[test]`s; cargo-fuzz is an ubuntu+nightly CI-only job.**
12. **tree-sitter dependency placement.** It must be `optional` + `[dev-dependencies]` + feature-gated (MSRV 1.90 > 1.88; native-only). The plan's `feature = "tree-sitter-backend"` should live in dev-deps and be asserted OFF on the MSRV and wasm CI lanes; rename to `tree-sitter-oracle` to reflect it is oracle-only by default.