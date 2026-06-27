# Phase 6 · Workstream 3 — The Formatter Playbook (`gdscript-fmt`)

> Research-backed, **code-grounded** build plan for a `gdformat`-compatible, CST-based, idempotent
> GDScript formatter built on the lossless `cstree` tree we already produce. Matches the Phase-5
> playbook depth/format bar.
>
> **Parents:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 3 (+ Testing #4),
> [`PHASE-5-CLI-PLAYBOOK.md`](PHASE-5-CLI-PLAYBOOK.md) §1/§9 (the `format` passthrough this replaces),
> [`PHASE-5-LSP-PLAYBOOK.md`](PHASE-5-LSP-PLAYBOOK.md) §3/§7 (the position-encoding + `to_proto` path
> formatting edits flow through), [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2 (the
> `gdscript-ide` POD contract a `format` query must return), [`research/02-parsing-strategy.md`](research/02-parsing-strategy.md)
> §5 (why a lossless CST — not tree-sitter `extras` — is the right base for formatting),
> [`research/05-prior-art-and-landscape.md`](research/05-prior-art-and-landscape.md) §2 (gdtoolkit
> `gdformat` = the parity target; GDQuest/gdstyle = the Rust competition).

---

## 0. Thesis

The hard, irreversible part — a **lossless CST with every byte of trivia retained** — is **already
built and battle-tested** (`crates/gdscript-syntax`, with byte-exact round-trip asserted across a
realistic corpus + 30+ recovery/lambda/soft-keyword regression tests). A formatter is the **one
feature the analyzer is uniquely positioned to ship for free**: it is a pure, deterministic
`CST → Doc IR → width-budgeted render` transform. There is **no analysis, no salsa dependency on
anything but `parse`, no cross-file work**. The two real engineering risks are (a) **`gdformat`
parity is an externally-owned, fuzzy target** (mitigated by a parity golden corpus + a documented
deviation list, not by chasing pixel-perfection) and (b) **idempotence + semantics-preservation**
(the two hard invariants, guarded by property tests). Everything else is mechanical.

The formatter lands as a **new internal crate `gdscript-fmt`** (CST in → `String`/`SourceChange`
out), wrapped by **one new `Analysis::format` / `Analysis::format_range` query** in `gdscript-ide`,
consumed by **three clients** that all already exist as stubs/shells: the CLI `format` command
(currently a passthrough — `crates/gdscript-cli/src/lib.rs:149` `run_format`), the LSP
`formatting`/`rangeFormatting` capability (not yet advertised in
`crates/gdscript-lsp/src/lib.rs:91` `server_capabilities`), and the guitkx embedded-GDScript
range-format (the documented second external consumer).

---

## 1. Goal — the 1.0 cut vs. the deferred tail

**The 1.0 cut (what ships):**

- A `gdscript-fmt` crate: `format(&Parse, &FmtConfig) -> String` (whole file) and
  `format_range(&Parse, byte_range, &FmtConfig) -> Option<SourceChange>` (a sub-span — the guitkx +
  LSP `rangeFormatting` path), built on a **Wadler/Prettier `Doc` IR** (`group`/`line`/`indent`/
  `concat`) rendered against a width budget.
- **Idempotence** (`format(format(x)) == format(x)`) and **semantics-preservation** (re-parse →
  AST-equivalent modulo trivia) as the two non-negotiable invariants, both property-tested.
- **`gdformat` parity** on a golden corpus, with every intentional deviation documented as an
  explicit golden exception (the "documented superset").
- **Tabs by default**, `line_width = 100`, `safe_mode = true` — four config options, no more
  (§4; the anti-bikeshed stance from the plan's Risks table).
- **Error-tolerant:** a file that fails to parse cleanly is formatted in its well-formed regions and
  left byte-identical across any span covered by an `ErrorNode` (`safe_mode` refuses outright when an
  error would change the AST — §6).
- Wired into **CLI `format --check`/`--write`** and **LSP `formatting`/`rangeFormatting`**.

**The deferred tail (explicitly NOT 1.0 — stated in the public roadmap):**

| Deferred | Why |
|---|---|
| **Magic trailing comma** semantics (Black/gdformat's "a trailing comma forces multiline") | A nice-to-have parity refinement; ship the simpler "fit-or-explode" rule first, add the comma trigger as a documented option post-1.0 if parity demands. |
| **Comment *reflow*** (rewrapping long `#` prose to the width budget) | gdformat does not reflow comment prose either; we preserve comment text verbatim. Reflow is a separate, opinionated feature. |
| **Configurable style profiles** beyond the four options | Anti-bikeshed; the value is *a* consistent style, not a knob farm. |
| **Import/`preload` organization, sorting members** | A refactor, not a formatter; post-1.0 (matches the plan's deferred "organize-imports-style preload cleanup"). |
| **Formatting *inside* string literals / GDScript-in-strings** | Out of scope; strings are opaque tokens. |

---

## 2. Current state — what EXISTS vs. the gap

### 2.1 EXISTS (the foundation is real and strong)

| Capability | Where | Note for the formatter |
|---|---|---|
| **Lossless CST, byte-exact round-trip** | `crates/gdscript-syntax/src/parser.rs` (`Parse`, `build_tree`, `emit`), asserted by `round_trips`/`corpus_round_trips_byte_for_byte` | The single most important precondition. The formatter's job is to *rebuild* this text with normalized trivia — it starts from a tree that already preserves every byte. |
| **Trivia is fully classified + retained** | `crates/gdscript-syntax/src/syntax_kind.rs:32-83` — `Whitespace`, `LineComment`, `DocComment` (`##`), `RegionComment` (`#region`), `EndRegionComment`, `LineContinuation`, `NewlinePhys`, `Bom`; `is_trivia()` at `:372` | The formatter reads these to preserve comments, doc-comments, regions, and **blank-line intent** (a run of `NewlinePhys` trivia between decls). |
| **Synthetic, zero-width layout markers** | `Newline`/`Indent`/`Dedent` (`syntax_kind.rs:57-62`), injected by the prepass | The formatter **does not re-run the prepass** — it reads structure from the *node hierarchy* (`Block`, `IfStmt`, etc.) and re-emits indentation itself. The synthetic markers are zero-width, so they don't interfere. |
| **A complete node grammar to print** | `syntax_kind.rs:281-361` — `SourceFile`, `FuncDecl`, `Block`, `IfStmt`/`ElifClause`/`ElseClause`, `MatchStmt`/`MatchArm`, `BinExpr`, `CallExpr`/`ArgList`, `ArrayLit`/`DictLit`/`DictEntry`, `Annotation`/`AnnotationArgList`, `PropertyBody`/`Getter`/`Setter`, `TypedArray`/`TypedDict`, … | Every construct the formatter must lay out has a node kind. The Doc-builder is a `match` over these. |
| **A CST-walking idiom to copy** | `crates/gdscript-ide/src/features.rs:118` `folding_ranges` walks `ast::descendants(&parsed.syntax_node())` and matches on `node.kind()`; `region_folds` re-tokenizes for `#region` pairing | The formatter's Doc-builder follows the same shape: recurse the red tree, match `kind()`, read child nodes/tokens via `children_with_tokens()`. |
| **`children_with_tokens()` / `NodeOrToken` access** | `crates/gdscript-syntax/src/ast.rs:86-114` (`has_token`, `token_text`, `name_token_text`) | The exact API the Doc-builder uses to read a node's tokens **and interleaved trivia** in source order. |
| **The POD edit shape to return** | `crates/gdscript-base/src/lib.rs:329-363` — `TextEdit { range: TextRange, new_text }`, `FileEdit`, `SourceChange::single(file, edits)` | `format_range` returns exactly this. **Note:** `gdscript_base::TextRange` is a `{start:u32, end:u32}` POD (`:19-34`), **distinct from `text_size::TextRange`** used inside the CST — the boundary conversion is the formatter's analogue of the LSP's offset work (§3.6). |
| **`parse` salsa query** | `crates/gdscript-db/src/lib.rs:172` `#[salsa::tracked] fn parse(db, file) -> Parse` | The `format` query depends **only** on `parse` — so it's incrementally cached for free and invalidates only when the file text changes. |
| **The CLI `format` shell + flags** | `crates/gdscript-cli/src/cli.rs:80-91` (`FormatArgs { paths, check, write }`), `lib.rs:149` `run_format` (passthrough today), `lib.rs:81` dispatch | `--check`/`--write` plumbing, exit codes (`EXIT_DIAGNOSTICS=1` when reformat needed), and the load→fan-out engine (`engine.rs`) all exist. We swap the passthrough body. |
| **The load→fan-out batch engine** | `crates/gdscript-cli/src/engine.rs:166` `Project::diagnostics` (rayon `map_with(self.host.analysis())`) | `format` reuses this exact pattern: one host, fan out `analysis.format(file)` over rayon. |
| **LSP edit→`WorkspaceEdit` conversion + range→proto** | `crates/gdscript-lsp/src/handlers.rs:200` `NavCtx::workspace_edit`, `convert::range_to_lsp` | The LSP `formatting` handler reuses the **identical** `SourceChange`→`TextEdit[]` machinery rename already uses. |
| **napi/wasm session delegation** | `crates/gdscript-session/src/lib.rs` (per-method `String`-returning delegators), `crates/gdscript-ffi/src/lib.rs` (`#[napi]` wrappers) | Adding `format`/`format_range` is one delegator + one `#[napi]`/`wasm` wrapper, matching the existing 18 methods. |

### 2.2 The gap (what genuinely does not exist)

1. **No `gdscript-fmt` crate.** Confirmed: `crates/` has no `*-fmt`/`*-format` member; a workspace
   grep for `pub struct Doc`/`fn format`/`formatter` finds only the CLI passthrough and unrelated
   `format!` macro calls. This is greenfield.
2. **No `Doc` IR.** No Wadler/Prettier pretty-printer anywhere in the tree.
3. **No `Analysis::format*` query** — the `gdscript-ide` API (`crates/gdscript-ide/src/lib.rs:169-350`)
   has `syntax_tree`, `diagnostics`, … `rename`, `workspace_symbols`, but **no formatting method**.
4. **No `format`/`rangeFormatting` LSP capability** — `server_capabilities`
   (`lsp.rs:91`) advertises hover…codeAction but **not** `document_formatting_provider` /
   `document_range_formatting_provider`; `handle_request` (`lsp.rs:326`) has no
   `textDocument/formatting` arm.
5. **No formatter config.** `gdscript-analyzer.toml` (`crates/gdscript-cli/src/config.rs`) carries
   only `error_on_warning`; no `line_width`/`indent`/`safe_mode`. (The config plumbing — discovery,
   nearest-wins, inline override, forward-compat unknown-key tolerance — already exists to extend.)
6. **No golden / parity corpus.** `fixtures/` has empty `ide`/`parser` dirs;
   `crates/gdscript-syntax/test_data/golden/` has one `.cst`. No `fixtures/format/`.

---

## 3. Design — `gdscript-fmt` crate

### 3.1 Crate shape + dependencies

A new **internal, `wasm32`-safe** crate `crates/gdscript-fmt` (added to `Cargo.toml` `members`
`crates/*` already globs it; add to `default-members` for native CI). It must stay pure
text→tree→text — **no `std::fs`, no clocks, no threads** (the same portability rule the plan's §7
imposes and `gdscript-syntax`/`gdscript-ide` honor).

```toml
# crates/gdscript-fmt/Cargo.toml  (sketch)
[dependencies]
gdscript-syntax = { workspace = true }   # the CST + SyntaxKind it prints
gdscript-base   = { workspace = true }   # TextEdit/TextRange/SourceChange POD (for format_range)
text-size       = { workspace = true }   # the CST's native TextRange/TextSize
# deliberately NO salsa, NO gdscript-db, NO gdscript-hir — the formatter is analysis-free.
```

**Do not pull in an external pretty-printer** (`pretty`, `dprint-core`) at 1.0: the Doc algebra we
need is ~150 lines, vendoring it keeps the wasm bundle small (§Workstream 4 budget) and avoids a
semver-coupling to a third-party Doc model in the contract path. (Re-evaluate post-1.0 if the
hand-rolled renderer grows.)

### 3.2 The `Doc` IR (Wadler/Prettier algebra)

The intermediate representation between the CST and the rendered string. This is what makes
line-width reflow and idempotence *tractable* instead of ad-hoc — exactly the plan's §3.1 mandate
("not ad-hoc string concatenation").

```rust
// crates/gdscript-fmt/src/doc.rs   (sketch — illustrative, ~150 lines incl. renderer)

/// A layout document. The renderer (§3.5) chooses flat-vs-broken per `Group` against the width.
pub enum Doc {
    /// Literal text that never contains a newline (an identifier, a keyword, an operator).
    Text(EcoString),
    /// Concatenation in order.
    Concat(Vec<Doc>),
    /// A soft line break: a single space when the enclosing group fits flat, else a newline+indent.
    Line,
    /// Like `Line` but renders to *nothing* when flat (for trailing-comma-before-`]` style).
    SoftLine,
    /// A line break that is ALWAYS a newline (forces the enclosing group to break) — a statement
    /// terminator, a `:`-block body separator.
    HardLine,
    /// Increase the indent level for the contained doc (one tab / `indent_size` spaces).
    Indent(Box<Doc>),
    /// A group: render flat if it fits the remaining width on one line, else broken.
    Group(Box<Doc>),
    /// Verbatim trivia (a comment, a preserved blank line) — emitted as-is, never reflowed.
    Trivia(EcoString),
    /// A blank line (collapses runs of ≥2 source blanks to exactly one — gdformat's rule).
    BlankLine,
}
```

Constructors mirror Prettier: `text`, `concat`, `group`, `indent`, `line`, `softline`, `hardline`,
`join(sep, items)`. (`EcoString`/`smol_str` is already a workspace dep — `smol_str = "0.3"` in
`Cargo.toml:73` — so small tokens stay cheap and `Copy`-ish.)

### 3.3 CST → Doc (the builder — the bulk of the work)

A recursive walk of the **resolved red tree** (`GdNode`), matching `node.kind()` and emitting `Doc`.
The walk reads `children_with_tokens()` so it sees nodes, tokens, **and trivia** in source order —
this is how comments and blank lines are placed correctly. Sketch of the dispatch (matching the
`folding_ranges` idiom at `features.rs:124`):

```rust
// crates/gdscript-fmt/src/build.rs   (sketch)
fn doc_for(node: &GdNode, cx: &Cx) -> Doc {
    match node.kind() {
        SyntaxKind::SourceFile  => top_level(node, cx),          // decls separated by BlankLine policy
        SyntaxKind::FuncDecl    => func(node, cx),               // header group + HardLine + Indent(body)
        SyntaxKind::Block       => block(node, cx),              // stmts joined by HardLine
        SyntaxKind::IfStmt      => if_stmt(node, cx),            // if/elif/else clause chain
        SyntaxKind::MatchStmt   => match_stmt(node, cx),
        SyntaxKind::CallExpr    => call(node, cx),               // Group(callee + arg_list)
        SyntaxKind::ArgList | SyntaxKind::ArrayLit | SyntaxKind::DictLit
                                => bracketed_list(node, cx),     // fit-or-explode + trailing comma
        SyntaxKind::BinExpr     => bin_expr(node, cx),           // operand op operand, spaced
        SyntaxKind::Annotation  => annotation(node, cx),         // @export ... placement rule
        // … one arm per node kind; tokens within a node are spaced by a token-pair rule table.
        _ => fallback_verbatim(node, cx),                        // unknown/ErrorNode → original bytes
    }
}
```

**Trivia attachment** is the subtle part (the plan's §3.1 "comments live in CST trivia"). Rules,
matching gdformat's behavior:
- A **leading** `LineComment`/`DocComment` on its own line attaches to the following node as a
  `Trivia` + `HardLine` prefix.
- A **trailing** `LineComment` after code on the same line attaches as ` ` + `Trivia` suffix to that
  statement (no break before it).
- `RegionComment`/`EndRegionComment` are preserved verbatim at their own indentation (the body is
  reindented but the marker line is kept).
- **Blank lines:** a run of ≥1 `NewlinePhys`-only lines between two top-level decls → one `BlankLine`;
  inside a block, gdformat allows up to one blank between statements — collapse ≥2 to 1, drop blanks
  immediately after a `:` header.

### 3.4 The GDScript rule set (what the builder encodes)

Grounded against `syntax_kind.rs` node/token kinds and gdformat's observed behavior. **The parity
corpus (§5) is the source of truth; this table is the spec the corpus validates.**

| Rule | Detail | CST anchor |
|---|---|---|
| **Indent unit** | One `\t` per level by default (config `indent`); never mixed. | the `Indent(Box<Doc>)` Doc node; config `indent_size` only when `indent = spaces`. |
| **`:` block headers** | `func f():`, `if c:`, `for x in xs:`, `match e:`, `class C:` — `:` tight to the header, body on `HardLine`+`Indent`. | `FuncDecl`/`IfStmt`/`ForStmt`/`MatchStmt`/`InnerClassDecl` → `Block`/`ClassBody`. |
| **One-line bodies** | gdformat **expands** `func f(): return x` to a two-line form by default (a single simple statement after `:` moves to its own indented line). Preserve a single-line *lambda* body only inside an expression where breaking is illegal-ish. | the inline-block case the parser handles (`round_trips_inline_function`). |
| **Operator spacing** | binary ops `a + b`, `a == b`, `a and b` get one space each side; unary `-x`, `not x` tight; `:=`/`=`/`->`/augmented (`+=` …) one space each side. | `BinExpr`/`UnaryExpr` + the operator token kinds (`Plus`…`ShrEq`, `ColonEq`, `Arrow`). |
| **Comma spacing** | `f(a, b)`, `[1, 2]`, `{k: v, …}` — no space before `,`, one after. | `ArgList`/`ArrayLit`/`DictLit`/`ParamList` + `Comma`. |
| **Type annotations** | `var x: int`, `func f(a: int) -> Type:` — `:` tight to name, one space after; `->` spaced. | `Param`/`VarDecl`/`TypeRef` (`token_text`/`type_ref` accessors). |
| **Annotation placement** | `@export`, `@onready`, `@warning_ignore(...)` on their **own line** above a declaration; a same-line annotation (`@export var x`) is normalized to gdformat's choice (keep inline for the simple-arg export family, own-line otherwise). | `Annotation`/`AnnotationArgList`; the statement-level-annotation case (`statement_level_annotation_in_a_body_parses_clean`). |
| **`##` doc comments** | Preserved verbatim, re-indented to the member they document; never merged with `#` comments. | `DocComment` trivia. |
| **`#region`/`#endregion`** | Preserved; body re-indented; the marker line kept at its level. | `RegionComment`/`EndRegionComment`. |
| **Trailing commas in collections** | When a bracketed list **breaks** (explodes to multiline), gdformat adds a trailing comma before the closer; when flat, none. (Magic-comma *trigger* deferred — §1.) | `bracketed_list` + `SoftLine` before closer. |
| **String quotes** | gdformat normalizes to **double quotes** unless the string contains a `"` (then keep as-is). Conservative: preserve quote style at 1.0 unless the parity corpus shows a divergence we must match. | `String`/`StringName`/`NodePath` tokens (opaque text). |
| **Blank-line policy** | ≤2 blank lines between top-level decls → gdformat uses **exactly one** (or two before a top-level func — match the corpus); ≤1 inside a body; none right after a `:` header. | `BlankLine` collapsing. |
| **Trailing newline** | File ends with exactly one `\n`. | the `SourceFile` render epilogue. |
| **Line endings** | Normalize to `\n` (gdformat's behavior). A `Bom` is preserved iff present at input (Godot tolerates it; the CST keeps it — `Bom` trivia at `syntax_kind.rs:54`). | the renderer's newline emission + BOM pass-through. |

### 3.5 The width-budget renderer

A standard Wadler "fits" renderer over the `Doc` tree: maintain `(remaining_width, indent_stack)`;
for each `Group`, **try flat** — if the flattened content fits in `remaining_width` with no
`HardLine`, render flat (`Line`→space, `SoftLine`→nothing); else render broken (`Line`/`SoftLine`→
newline + current indent). `Indent` pushes/pops the indent level; `HardLine` always breaks and forces
every enclosing group broken; `Trivia`/`BlankLine` are emitted verbatim. ~80 lines. This is the
*only* place `line_width` is consulted.

### 3.6 The two entry points + the `text_size`↔`base` boundary

```rust
// crates/gdscript-fmt/src/lib.rs   (sketch)

/// Whole-file format: CST → normalized String. The CLI `--write`/`--check` path.
pub fn format(parse: &Parse, cfg: &FmtConfig) -> String;

/// Range format: format only the node(s) covering `range`, returning the minimal edit(s).
/// `None` when `range` covers nothing formattable (or `safe_mode` refuses). The LSP
/// `rangeFormatting` + guitkx embedded-GDScript path.
pub fn format_range(
    parse: &Parse,
    file: FileId,
    range: text_size::TextRange,
    cfg: &FmtConfig,
) -> Option<SourceChange>;
```

- **Whole-file** is conceptually `SourceChange::single(file, [TextEdit{ range: 0..len, new_text }])`,
  but the query (`§3.7`) also exposes the raw `String` for the CLI (which writes the file directly).
- **`format_range`** finds the smallest **enclosing statement/declaration node** whose `text_range()`
  contains `range` (right-biased, like `ast::token_at` at `ast.rs:366`), formats *that subtree* with
  the indent level it sits at, and emits a single `TextEdit` replacing that node's span. Critically,
  the byte range it emits is a **`gdscript_base::TextRange`** (`{start,end}` POD), converted from the
  CST's **`text_size::TextRange`** via `TextRange::new(r.start().into(), r.end().into())` — this is
  the formatter's load-bearing boundary conversion (the analogue of the LSP's offset work; do it in
  exactly one helper, like `features.rs:to_base_range`).
- **Idempotence for range-format:** formatting a sub-span with the wrong base indentation is the #1
  range-format bug. The renderer takes a **starting indent level** derived from the node's depth in
  the tree, so the produced text slots back in without shifting surrounding lines.

### 3.7 The `gdscript-ide` query (the contract surface)

Add to `crates/gdscript-ide/src/lib.rs` `impl Analysis`, mirroring the existing `syntax_tree`/
`diagnostics` shape (read `file_text`, run the pure function, wrap in `catch`):

```rust
/// Format the whole file. `None` if the file is unknown. The text is the formatted source.
pub fn format(&self, file: FileId, cfg: &FmtConfig) -> Cancellable<Option<String>> {
    catch(|| self.db.file_text(file).map(|ft| {
        let parse = gdscript_db::parse(&self.db, ft);
        gdscript_fmt::format(&parse, cfg)
    }))
}

/// Format a byte range → a minimal `SourceChange` (LSP `rangeFormatting` + guitkx).
pub fn format_range(
    &self, file: FileId, range: text_size::TextRange, cfg: &FmtConfig,
) -> Cancellable<Option<SourceChange>> {
    catch(|| self.db.file_text(file).and_then(|ft| {
        let parse = gdscript_db::parse(&self.db, ft);
        gdscript_fmt::format_range(&parse, file, range, cfg)
    }))
}
```

**Where does `FmtConfig` live?** It is a small POD; put the type in `gdscript-base` (next to
`TextEdit`/`SourceChange`) so it is part of the documented `gdscript-ide` contract (§Workstream 6
`#[non_exhaustive]` audit applies — adding a future option stays minor). `gdscript-fmt` re-exports
it. **It is passed as an argument, not a salsa input** — so changing formatter config never
invalidates `parse`/`infer` (the same "gating is cheap, doesn't touch expensive queries" principle
Workstream 1 uses for warning settings). A salsa-cached `format(file)` is **not** worth it (format is
on-demand, not on-keystroke); keep it a plain wrapper over the cached `parse`.

---

## 4. Config (`FmtConfig`)

Four options. Discovered via the **existing** `gdscript-analyzer.toml` plumbing
(`crates/gdscript-cli/src/config.rs` — nearest-wins walk-up, inline `--config k=v`, forward-compat
unknown-key tolerance already there); the LSP reads it from the workspace root.

| Option | Default | Notes | gdformat parity |
|---|---|---|---|
| `line_width` | `100` | the width budget the renderer targets. | gdformat default = 100; match it. |
| `indent` | `tabs` | `tabs` \| `spaces`. | gdformat = tabs (Godot's own convention — and the prepass already treats a tab as a flat `TAB_SIZE=4`, `prepass.rs:38`). |
| `indent_size` | `4` | only meaningful when `indent = spaces`. | matches the engine's tab width. |
| `safe_mode` | `true` | refuse to reformat (return input unchanged) when the parse contains an `ErrorNode` whose reformatting could change the AST. | a safety gate, not a gdformat option — documented as our addition. |

Add a `[format]` table to `Config` (the struct at `config.rs:21`); fields `Option<_>` for the same
layered merge the existing `error_on_warning` uses. **Compatibility statement** ships in docs
(Workstream 5): where we match `gdformat` exactly, and every documented deviation (the "superset"
parts) — generated/checked against the parity corpus so the statement can't drift.

---

## 5. Test plan

The plan's Testing §4 is the spec: idempotence + gdformat parity + semantics-preservation +
range-format. Concretely:

1. **Golden corpus** — `crates/gdscript-fmt/test_data/` (or `fixtures/format/`): `*.gd` (input) +
   `*.expected` (formatted). Reviewed via `expect_test::expect_file!` — the **same harness**
   `gdscript-syntax` already uses for `golden_small_class` (`parser.rs:755`). Seeded from:
   - (a) the **godot-demo-projects** corpus already referenced by the parser's regression tests
     (the `multiline_lambda_*` / `statement_level_annotation_*` cases came from it),
   - (b) a **`gdformat` parity set** — files run through *real* `gdformat` (gdtoolkit 4.x) whose
     output we must reproduce; any deviation is an explicit golden exception with a one-line reason
     in a `DEVIATIONS.md`,
   - (c) **adversarial** cases: deep nesting, a 200-char call that must explode, comment-heavy files,
     mixed tabs/spaces (the prepass already diagnoses these — `prepass.rs:328`), `#region` blocks,
     trailing-comma collections, doc-comment-decorated members, multiline lambdas in call args.
2. **Idempotence property test** — `format(format(x)) == format(x)` over the **whole corpus** plus a
   small `proptest`/`arbitrary` generator of valid-ish GDScript (or fuzzer output). This is the core
   invariant; it must be a hard CI gate.
3. **Semantics-preservation** — for every corpus file, re-parse `input` and `format(input)` and
   assert the **node-only S-expression is equal** (the `node_sexpr`/`structure` helper already in
   `parser.rs:411` — reuse it). Formatting must never change the AST modulo trivia. (This is what
   makes `safe_mode` provable: on an `ErrorNode`, refuse rather than risk a structural change.)
4. **gdformat differential (parity)** — a harness (gated behind an env flag / a `gdformat-available`
   feature, like the parser's tree-sitter oracle) that shells out to real `gdformat` over the parity
   set and diffs. **Documented divergences are expected, not failures** (recorded in `DEVIATIONS.md`).
   Run in a dedicated CI job that installs gdtoolkit; the in-tree golden `.expected` files are the
   committed snapshot so the unit tests don't need Python.
5. **Range-format** — format a sub-span (a single statement, a single function) and assert (a) the
   produced `SourceChange` replaces exactly that node's span, (b) applying it leaves the rest of the
   file **byte-identical** (the guitkx requirement), and (c) the result equals the corresponding slice
   of the whole-file format (range and whole-file agree). Multi-byte/CRLF cases included (the
   `text_size`↔`base` boundary, §3.6).
6. **Round-trip on already-formatted input** — `format(expected) == expected` for every golden
   (a stronger idempotence anchor; catches "the formatter fights its own output").
7. **Error-tolerance** — a file with a broken region (the `broken_code_recovers_and_round_trips`
   shape, `parser.rs:731`): the well-formed siblings format, the `ErrorNode` span is byte-identical,
   and under `safe_mode = true` a structure-threatening error refuses the whole file cleanly.
8. **CLI integration** — extend `crates/gdscript-cli/tests/cli.rs` (currently
   `format_passthrough_exits_0` at `:176`): `format --check` exits `1` on an unformatted file and `0`
   on a formatted one; `format --write` rewrites in place and is idempotent on a second run.
9. **LSP integration** — an in-process `Connection::memory()` test (the harness at `lsp.rs:837`):
   advertise `documentFormattingProvider`, send `textDocument/formatting`, assert the returned
   `TextEdit[]` reformat the doc; same for `rangeFormatting`.

---

## 6. Risks + mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **`gdformat` parity is a moving, externally-owned target** | **High** | Make parity a **golden corpus + an explicit `DEVIATIONS.md`**, not a promise of byte-identity. The hard invariants are **idempotence + semantics-preservation** (§5.2/§5.3); style parity is "best-effort, documented." A dedicated differential CI job surfaces drift when gdtoolkit releases. |
| **Idempotence bugs (the formatter fights its own output)** | **High** | The §5.2 property test over the whole corpus + fuzzer is a hard gate; the §5.6 "format(expected)==expected" anchor catches the common case early; build the renderer flat-fits logic carefully (Wadler's algorithm is provably stable). |
| **Semantics change (esp. significant indentation)** | **High** | §5.3 AST-equality test on every file; **`safe_mode`** refuses on `ErrorNode`; the formatter re-emits structure from the *node hierarchy*, never from the byte indentation, so it can't desync indent from nesting. GDScript's indentation-significance is exactly why semantics-preservation is non-optional. |
| **Comment/trivia misattachment** (a comment jumps to the wrong line) | **Med** | The CST retains every comment as a positioned trivia token (`is_trivia()`), and the builder reads `children_with_tokens()` in source order; the leading/trailing/region attachment rules (§3.3) are corpus-tested with comment-heavy adversarial fixtures (§5.1c). This is the class of bug `research/02` §5 says kills tree-sitter `extras` formatters — and exactly why we own a real CST. |
| **Range-format base-indent bug** (a sub-span reformats at the wrong indent) | **Med** | The renderer takes an explicit starting indent level from the node's tree depth (§3.6); §5.5 asserts the range result equals the whole-file slice. |
| **Config bikeshedding** | **Low** | Four options, hard stop (§4); the deferred-tail table (§1) names what's intentionally *not* configurable. |
| **wasm bundle bloat from a Doc library** | **Low** | Vendor the ~150-line Doc algebra; no external pretty-printer dep (§3.1). Stays under the Workstream-4 wasm budget. |
| **Formatting an error-recovered tree produces garbage** | **Med** | `fallback_verbatim` emits the **original bytes** for any `ErrorNode`/unknown kind (the CST is lossless, so the bytes are right there); `safe_mode` is the belt-and-suspenders refusal. |

**Biggest correctness risk:** semantics-preservation under significant indentation (§5.3 + `safe_mode`).
**Biggest fuzzy risk:** gdformat parity — handled by making it a documented golden target, not a hard
contract. **Biggest leverage:** the lossless CST already exists and round-trips byte-for-byte — the
formatter is a pure transform with no analysis dependency, so it's the cheapest 1.0 workstream to land
correctly once the Doc IR is in place.

---

## 7. Dependencies on other workstreams

- **Phase 1 (done) — the lossless CST.** Hard prerequisite, fully met. The formatter prints
  `gdscript-syntax`'s tree; no change to the parser is required. *Possible small asks:* if the
  builder needs a token-with-trivia iterator helper not already on the AST, add it to
  `gdscript-syntax/src/ast.rs` (cheap, additive).
- **Phase 5 (done) — the consumers.** The CLI `format` shell (`cli.rs`/`lib.rs`), the LSP shell
  (`lsp.rs`), and the napi/wasm session (`gdscript-session`/`gdscript-ffi`) all exist; this workstream
  fills their bodies. The LSP edit→`WorkspaceEdit` + `range_to_lsp` machinery (`handlers.rs`/
  `convert.rs`) is reused verbatim.
- **Workstream 6 (API stabilization)** — `FmtConfig`, `Analysis::format`, and `Analysis::format_range`
  become part of the frozen `gdscript-ide` contract. They **must** land before the API-review pass and
  carry `#[non_exhaustive]` on `FmtConfig` (so adding the magic-trailing-comma option post-1.0 is a
  minor bump). `cargo-semver-checks` then guards them. **Sequencing: the formatter API must be settled
  before the 1.0 freeze.**
- **Workstream 5 (docs)** — the formatter needs: a Configuration page section (the four options +
  the `[format]` TOML), the **gdformat compatibility statement + `DEVIATIONS.md`**, and a CLI
  `format` reference entry. The playground (live docs) can expose a "Format" button calling the wasm
  `format` method.
- **Workstream 7 (≥1 external consumer)** — the formatter **is** the named second consumer: guitkx
  range-formats embedded GDScript via `format_range` + its Volar-style source-map adapter (no Python
  `gdformat` runtime dependency), and "a Rust/WASM `gdformat`" is itself an adoption hook. The
  `format_range` signature (`SourceChange` over a byte range) is designed for exactly this — validate
  it against the guitkx adapter during M2.
- **No dependency on Workstreams 1, 2, or 4.** The formatter is analysis-free: it does not read
  warnings, narrowing, or the engine API, and its only salsa touchpoint is the already-cached `parse`
  query. It can be built fully in parallel with the warning-set and narrowing work.

---

## 8. Milestones (each green through `cargo xtask ci`)

- **M0 — the Doc spine.** `gdscript-fmt` crate + the `Doc` IR + the width renderer + the
  whole-file builder for the core constructs (decls, blocks, `if`/`for`/`while`/`match`, calls,
  binary/unary exprs, collections). Idempotence + semantics-preservation property tests wired.
  *Exit:* `format` round-trips the parser's `CORPUS` (`parser.rs:571`) to a stable, idempotent,
  AST-preserving output.
- **M1 — trivia + the rule set.** Comment/doc-comment/region attachment, blank-line policy, trailing
  commas, annotation placement, operator/comma spacing, the golden corpus + the `gdformat` parity set
  + `DEVIATIONS.md`. *Exit:* the differential job matches `gdformat` modulo documented deviations.
- **M2 — the consumers.** `Analysis::format`/`format_range` + `FmtConfig` in `gdscript-base`/`-ide`;
  CLI `format --check`/`--write` (real, replacing the passthrough at `lib.rs:149`); LSP
  `formatting`/`rangeFormatting` (advertise the caps at `lsp.rs:91`, add the request arm at
  `lsp.rs:326`); the napi/wasm `format` delegators; the guitkx range-format validation. *Exit:*
  `gdscript format --write` reformats a project; the LSP formats a document and a range; guitkx
  range-formats embedded GDScript through `format_range`.
- Per-milestone **adversarial bug-hunt** (find→verify→fix), like every prior milestone.

## Sources (verified)
gdtoolkit `gdformat` (the parity target — `research/05` §2, gdtoolkit 4.5.0, MIT, Python/lark);
GDQuest GDScript-formatter (Rust/tree-sitter+Topiary — the comment-attachment cautionary tale,
`research/02` §5); gdstyle (`formatter::format_source`); Wadler "A prettier printer" / Prettier's
`group`/`line`/`indent` algebra; rust-analyzer/Biome lossless-CST formatting (`research/02` §5).
**Grounded in:** `crates/gdscript-syntax/src/{syntax_kind,parser,ast,prepass}.rs` (the CST the
formatter prints), `crates/gdscript-ide/src/{lib,features}.rs` (the query surface + CST-walk idiom),
`crates/gdscript-base/src/lib.rs:329-363` (the `TextEdit`/`SourceChange` POD), `crates/gdscript-cli/src/{cli,lib,engine,config}.rs`
(the `format` shell + batch engine + config plumbing), `crates/gdscript-lsp/src/{lib,handlers,convert}.rs`
(the capability + edit-conversion path).
