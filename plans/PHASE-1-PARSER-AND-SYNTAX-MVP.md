# PHASE 1 — Parser & Syntax MVP (Tier 0)

> **⚠️ Read [`PHASE-1-IMPLEMENTATION-PLAYBOOK.md`](PHASE-1-IMPLEMENTATION-PLAYBOOK.md) first.**
> That companion doc is the **current-API authority** (mid-2026 crate versions, the
> resolved napi/wasm-bindgen split, the cstree build recipe, the dependency-ordered
> build sequence). Where this design doc and the Playbook disagree, **the Playbook
> wins** — its §8 lists the itemized corrections to this file (notably: `#[repr(u32)]`
> not `u16`; indentation `tab_size = 4` not `8`; `true`/`false`/`null` are literals,
> not keywords; the Node binding (napi `gdscript-ffi`) and the browser binding
> (wasm-bindgen `bindings/wasm`) are **separate crates**; `tree-sitter` is an optional
> native-only dev-dependency; `proptest` is the local robustness harness and
> `cargo-fuzz` is CI-only). The inline `tab = 8` and `#[repr(u16)]` below are corrected
> in place; the rest stand as design intent, refined by the Playbook.

> Execution-ready plan for the first feature phase. Delivers the lexer, the
> indentation pre-pass, the lossless hand-written recursive-descent parser →
> `cstree` CST + typed AST, error recovery, the `AnalysisHost`/`Analysis`
> skeleton, and the napi+wasm binding returning **parse diagnostics, document
> symbols, folding ranges, and by-name (no-type) completion**.
>
> Obeys [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§6 parser model, §1 crate
> stack, §2 public API, §7 portability) and the Phase 1 row of
> [`ROADMAP.md`](ROADMAP.md). Primary evidence:
> [`research/02-parsing-strategy.md`](research/02-parsing-strategy.md),
> [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md),
> [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md).
>
> **Hard rule, restated:** no Godot editor in the loop, *ever*. No port 6005, no
> headless engine. This phase is pure text → tree → syntactic features.

---

## Goal & scope (Tier 0)

Tier 0 is **"parse + symbol table + by-name completion (no inference)"** — high
value, low risk, ~80% of perceived "it works," ships fast
([`ROADMAP.md`](ROADMAP.md) Tier table). Everything in this phase is derivable
from the **syntax tree alone**; nothing here needs types, the engine API model,
or cross-file resolution.

### What ships

| Deliverable | Crate | Notes |
|---|---|---|
| `logos` token set + trivia retention | `gdscript-syntax::lexer` | WS1 |
| Indentation pre-pass (INDENT/DEDENT/NEWLINE injection) | `gdscript-syntax::prepass` | WS2 — the riskiest module |
| Hand-written resilient recursive-descent parser → `cstree` CST | `gdscript-syntax::parser` | WS3, behind a `Parser` trait |
| Typed AST layer over the CST | `gdscript-syntax::ast` | WS4 |
| tree-sitter-gdscript optional backend + permanent differential oracle | `gdscript-syntax` (`feature = "tree-sitter-backend"`) | WS5 |
| `AnalysisHost` / `Analysis` skeleton (whole-file reparse, no salsa) | `gdscript-ide` | WS6 |
| POD result types (`FileId`, `TextRange`, `Diagnostic`, `DocumentSymbol`, `FoldRange`, `CompletionItem`, …) | `gdscript-base` | WS6 |
| napi-rs v3 `AnalysisHandle` → Node `.node` **and** wasm; flat JSON API | `gdscript-ffi` | WS7 |
| "hello analyzer" Node script + browser page (document symbols) | `bindings/node`, `playground/` (stub) | WS7 |

### Tier-0 IDE features (the only four query methods that return real data)

1. **`diagnostics(file)`** — **parse errors only** (lexer + parser recovery).
   No type/lint diagnostics (that is Phase 2's `UNSAFE_*` / 48-warning set).
2. **`document_symbols(file)`** — classes, inner classes, funcs, vars, consts,
   signals, enums (+ members) from the AST. (research/04 §4: "Low" difficulty,
   "AST + scopes".)
3. **`folding_ranges(file)`** — indented blocks + `#region`/`#endregion` +
   multi-line brackets. (research/04 §4: "Low", "AST + `#region` lexer tokens".)
4. **`completions(pos)`** — **by-name only**: the ~35 keyword table, the
   36-annotation table after `@`, and document-local symbol names in scope.
   **No member completion** (no `.` receiver typing), **no types**.

### What this phase explicitly does NOT do

- ❌ **No type inference** of any kind — no `:=` resolution, no member-access
  typing, no `is`/`as` narrowing. (Phase 2 / Tier 1.)
- ❌ **No member completion** (`button.` → `Button` members needs the engine API
  model + inference — Phase 2).
- ❌ **No `gdscript-api` engine model**, **no `gdscript-hir`**. (Phase 2.)
- ❌ **No cross-file anything** — no `class_name` registry, no `preload`/`extends`
  resolution, no autoloads, no project scan, no `.tscn`. (Phase 3 / Phase 4.)
- ❌ **No salsa.** Whole-file reparse on every change; a plain
  `HashMap<FileId, Arc<Parsed>>`. ([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §3.)
- ❌ **No hover, signature help, goto, references, rename, semantic tokens,
  inlay hints, code actions, formatting.** Those `Analysis` methods exist as
  signatures (per the architecture §2 surface) but return `Ok(empty)`/`Ok(None)`
  in Phase 1.
- ❌ **No Godot editor, no LSP server crate, no CLI.** (`gdscript-lsp` /
  `gdscript-cli` are Phase 5.)

---

## Prerequisites (Phase 0 complete)

Phase 1 assumes [`PHASE-0`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) landed:

- The virtual `crates/*` workspace exists with **compiling stubs** for
  `gdscript-base`, `gdscript-syntax`, `gdscript-ide`, `gdscript-ffi` (the only
  crates this phase fills). `gdscript-api`/`db`/`hir` may be empty stubs.
- **CI** runs `cargo xtask ci`: `fmt` + `clippy -D warnings` + the test matrix +
  MSRV + **`cargo check -p gdscript-ide --target wasm32-unknown-unknown`** +
  coverage. This wasm-check is a Phase-1 gate (see Testing).
- A **fixtures harness** exists: `fixtures/` holds `.gd` inputs and golden
  outputs; an `xtask` subcommand (e.g. `cargo xtask test-fixtures`,
  `--bless` to regenerate goldens) drives them. Phase 1 populates it heavily.
- The release toolchain + dual-license headers + `getrandom`/wasm feature
  plumbing exist but are inert here.

If any of the above is missing, it is Phase-0 work — do not solve it inside
Phase 1.

---

## Workstream 1 — Lexer (`gdscript-syntax::lexer`)

**Tool:** `logos` derive lexer ([`research/02`](research/02-parsing-strategy.md)
§3: "ridiculously fast … compiles token specs to a single DFA"). The lexer
produces a **flat stream of `(SyntaxKind, TextRange)`** over the raw bytes,
**including trivia** (whitespace + comments) as real tokens — losslessness
starts here. The lexer does **not** know about indentation; that is WS2.

### Token kinds (the `logos` token set)

The lexer's `Token` enum is a subset of `SyntaxKind` (WS3 owns the full
`SyntaxKind`; the lexer maps 1:1 into it). Enumerated:

| Group | Token kinds |
|---|---|
| **Keywords (~35)** (research/04 §1.1) | `if elif else for while match when break continue pass return` · `var const enum func static signal class class_name extends` · `is in as self super void` · `await preload assert breakpoint` · `not and or` · `true false null` · (deprecated, still lexed) `yield` · (built-in const names like `PI TAU INF NAN` are lexed as `IDENT` and resolved later, not keywords) |
| **Identifiers** | `IDENT` (`[A-Za-z_][A-Za-z0-9_]*`) |
| **Int literals** | `INT` — decimal `45`, hex `0x8f51`, binary `0b101010`, digit-separators `12_345` |
| **Float literals** | `FLOAT` — `3.14`, `58.1e-10`, `.5`, `1.` |
| **String literals** | `STRING` — `"…"`, `'…'`; **multiline** `"""…"""` / `'''…'''`; **raw** `r"…"` / `r'…'`. (Lexer must handle escapes + unterminated-string recovery → an error token spanning to EOL/EOF.) |
| **StringName literal** | `STRING_NAME` — `&"…"` / `&'…'` |
| **NodePath literal** | `NODE_PATH` — `^"…"` / `^'…'` |
| **Node sugar** | `DOLLAR` `$`, `PERCENT` `%` (unique-name; also the `%` operator — disambiguated by the parser, lexer emits one `PERCENT`), `$"…"` is `DOLLAR` + `STRING` |
| **Annotation sigil** | `AT` `@` (the annotation *name* is the following `IDENT`; `@export` = `AT` + `IDENT("export")`) |
| **Operators** | arithmetic `+ - * / % **`; compare `== != < > <= >=`; logical symbols `&& \|\| !`; bitwise `~ & \| ^ << >>`; assign `=`; compound `+= -= *= /= **= %= &= \|= ^= <<= >>=`; arrow `->`; walrus/infer `:=` |
| **Punctuation** | `( ) [ ] { }` · `, : ; . .. ...` (`..` open-ended match / slice; `...` varargs rest) · `?` is not used (ternary is keyword-based `x if c else y`) |
| **Comments (trivia)** | `COMMENT` `# …` to EOL; `DOC_COMMENT` `## …` (feeds hover later); `#region` / `#endregion` recognized as `COMMENT` but flagged (a sub-kind or a post-scan) for folding |
| **Whitespace (trivia)** | `WHITESPACE` (spaces/tabs runs), `LINE_CONTINUATION` (`\` immediately before newline) |
| **Newline (physical)** | `NEWLINE_PHYS` — a physical line break the **pre-pass** consumes/transforms (WS2). The parser never sees `NEWLINE_PHYS`; it sees the synthetic `NEWLINE`/`INDENT`/`DEDENT` WS2 injects. |
| **Error** | `ERROR` — `logos` fallback for any unlexable byte; carried into the tree as an error token (never dropped — losslessness). |

### Trivia retention for losslessness

- **Every byte is a token.** Whitespace, comments, and line continuations are
  emitted as `WHITESPACE` / `COMMENT` / `DOC_COMMENT` / `LINE_CONTINUATION`
  tokens, never skipped. This is the rust-analyzer/Biome model
  ([`research/02`](research/02-parsing-strategy.md) §5: "trivia are first-class
  tokens we place deliberately in the green tree").
- The **parser** decides trivia *attachment* (leading vs trailing) when building
  the CST (WS3); the lexer only guarantees nothing is lost.
- **Invariant:** `concat(token.text for token in lex(src)) == src`, byte for
  byte. This is the lexer-level half of the round-trip guarantee tested in WS3.

### Deliverable

`lexer::tokenize(&str) -> Vec<Token>` (or an iterator), wasm-safe (no `std::fs`,
no clocks). A unit-test table of (input → token kinds) covering each row above,
plus the lexer round-trip invariant.

---

## Workstream 2 — Indentation pre-pass (the riskiest module)

GDScript has **Python-like significant indentation**
([`research/02`](research/02-parsing-strategy.md) §4). Godot's own tokenizer
emits `INDENT`/`DEDENT`/`NEWLINE`, tracks a line-continuation flag, and
**suppresses indentation significance inside brackets**. We isolate ~80% of the
language's quirk-risk into **one standalone, golden-tested module** that consumes
the WS1 token stream and produces a token stream the parser can treat as
block-structured. It is modeled on Godot's tokenizer semantics (so we match the
engine) — column math uses Godot's flat `tab_size` (default **4**) per tab, **not**
tree-sitter `scanner.c`'s 8-column tab-stops (scanner.c informs the indent-stack /
`within_brackets` structure, but its tab width is the wrong oracle — see Playbook §2).

### Synthetic tokens injected

- **`NEWLINE`** — a *logical* statement terminator (emitted at a physical line
  break that is **not** inside brackets and **not** preceded by a `\`
  continuation, and not on a blank/comment-only line).
- **`INDENT`** — emitted once when the new logical line's indentation column is
  **greater** than the top of the indent stack (push).
- **`DEDENT`** — emitted (possibly multiple times) when indentation is **less**
  than the top of stack (pop until match); a mismatch (no equal level on the
  stack) is flagged as an indentation diagnostic but recovery continues.

These three replace `NEWLINE_PHYS`; the parser in WS3 sees only `NEWLINE` /
`INDENT` / `DEDENT` + non-trivia tokens (trivia still threaded through for the
CST).

### The algorithm (pseudocode)

```text
state:
  indent_stack: Vec<u32> = [0]      # column widths; base guard 0
  bracket_depth: u32 = 0            # ( [ { open minus close
  pending_continuation: bool = false
  at_line_start: bool = true
  out: Vec<Token> = []

fn column_width(ws_text):           # Godot gdscript_tokenizer.cpp convention
  col = 0
  for ch in ws_text:
    if ch == '\t': col += tab_size           # FLAT add of tab_size (default 4), NOT
                                             # tree-sitter scanner.c's `8 - col%8` tab-stops.
    else: col += 1                           # space = +1
  return col
  # NOTE: also detect tab-after-space / space-after-tab → emit MIXED_INDENT
  #       diagnostic (Godot 4 forbids mixing) but DO NOT abort — recover.

for each physical line L in token stream:
    leading_ws = run of WHITESPACE at start of L (may be empty)
    rest       = tokens after leading_ws

    # 1. Skip lines that must not change indentation state:
    if rest is empty                      # blank line
       or rest starts with COMMENT/DOC_COMMENT and nothing else:  # comment-only
        emit leading_ws + rest (as trivia/comment); CONTINUE   # no NEWLINE/INDENT/DEDENT

    # 2. Bracket / continuation suppression:
    if bracket_depth > 0 or pending_continuation:
        emit tokens of L verbatim (incl. their newline as trivia, NOT NEWLINE)
        update bracket_depth on each ( [ {  +1 / ) ] }  -1
        pending_continuation = (last meaningful token == LINE_CONTINUATION)
        CONTINUE

    # 3. Normal logical line → compute indentation delta:
    col = column_width(leading_ws)
    top = indent_stack.last()
    if col > top:
        indent_stack.push(col); emit INDENT
    elif col < top:
        while indent_stack.last() > col:
            indent_stack.pop(); emit DEDENT
        if indent_stack.last() != col:
            diagnostic("unindent does not match any outer indentation level")
            # recovery: push col to resync, keep parsing
            indent_stack.push(col)
    # (col == top → no INDENT/DEDENT)

    emit leading_ws (trivia) + rest-of-line tokens
    update bracket_depth across the line
    pending_continuation = (last token == LINE_CONTINUATION)
    if not pending_continuation and bracket_depth == 0:
        emit NEWLINE

# 4. Dedent-to-EOF: close all open blocks at end of file
while indent_stack.last() > 0:
    indent_stack.pop(); emit DEDENT
emit final NEWLINE (if file non-empty and last wasn't already NEWLINE)
emit EOF
```

Key rules made explicit (each is a corpus case below):

- **`\` line continuation:** `LINE_CONTINUATION` token before a newline joins the
  logical line — no `NEWLINE`/`INDENT`/`DEDENT`. (research/02 §4 case 1.)
- **Bracket suppression:** inside `()`/`[]`/`{}`, newlines/indentation are not
  significant — `bracket_depth` gate. (case 2.)
- **Blank / comment-only / whitespace-only lines** keep indentation state — no
  spurious `DEDENT`. (case 7.)
- **Tab vs space:** tab = 8-col stops; mixing within one indent run → recover +
  flag (`MIXED_INDENT`), never abort. (case 6.)
- **Dedented comments:** a column-0 comment inside a body must **not** close the
  scope — comment-only lines are skipped in step 1 *before* indentation math, so
  they cannot emit `DEDENT`. (case 8; matches scanner.c's re-attachment intent.)
- **The `:`-block ambiguity** (scanner.c's noted "breaks if elses" wart, case 9):
  the pre-pass does **not** try to understand `:` — it only tracks columns. A
  trailing `:` introducing a block is handled by the **parser** expecting an
  `INDENT` after it; a single-line block (`func f(): return 1`) simply never
  produces an `INDENT` and the parser accepts the inline form. This deliberately
  keeps the dict-`{}` vs block ambiguity out of the pre-pass.

### The golden edge-case corpus (build all of these)

Stored under `fixtures/lexer-prepass/`, each `.gd` paired with a golden token
dump (`*.tokens`). Blessed via `cargo xtask test-fixtures --bless`.

| # | Case | What it proves |
|---|---|---|
| 1 | `\` continuations (mid-expr, in arg list, chained) | no NEWLINE/INDENT inside a continued line |
| 2 | Multiline brackets `( [ {` (calls, arrays, dicts, nested) | `bracket_depth` suppresses significance |
| 3 | `func` bodies, nested `if`/`for`/`while` | core INDENT/DEDENT driver, multi-DEDENT close |
| 4 | Lambdas — inline `func(x): print(x)` + multiline lambda bodies | block starting mid-expression |
| 5 | `match` blocks — patterns + nested bodies under `:` | nested indentation context |
| 6 | Mixed tabs/spaces (tab-then-space, space-then-tab, tab=8 alignment) | `MIXED_INDENT` recovery, column math |
| 7 | Blank / whitespace-only / comment-only lines between statements | no spurious DEDENT |
| 8 | Column-0 (dedented) comments inside a body; `#region`/`#endregion` | comment lines never close scope |
| 9 | Trailing-`:` blocks vs inline single-line blocks vs dict literals `{}` | the `:`-ambiguity stays out of the pre-pass |
| 10 | Property `get:`/`set:` blocks | accessor blocks indent like func bodies |
| 11 | Dedent-to-EOF (file ends mid-nest, no trailing newline) | step 4 closes all blocks |
| 12 | Empty file / only-comments file | EMPTY_FILE-shaped input parses to a trivial tree |

### Deliverable

`prepass::run(Vec<Token>) -> (Vec<Token>, Vec<IndentDiagnostic>)`, wasm-safe,
with the full golden corpus green. This module is the single highest-risk item
in Phase 1; budget the most review here.

---

## Workstream 3 — Parser → CST (`cstree`)

A **hand-written, lossless, error-recovering recursive-descent parser** producing
a **`cstree`** green/red CST. `cstree` over `rowan` for **`Send + Sync` realized
trees + token interning** under an LSP threadpool
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §6;
[`research/02`](research/02-parsing-strategy.md) §6 "Crate choice"). The parser
emits a flat **`Open`/`Close`/`Advance` event stream** (matklad's resilient-LL
model) that a `TreeSink` turns into the `cstree` green tree — decoupling grammar
logic from tree construction.

### The `Parser` trait (backends swap)

```rust
// gdscript-syntax::parse

/// Output of any parser backend: a lossless tree + the diagnostics gathered
/// during parsing. Parsing NEVER fails — it always returns a tree.
pub struct Parse {
    pub green: cstree::green::GreenNode,      // the lossless green tree
    pub errors: Vec<SyntaxError>,             // byte-ranged "expected X" diagnostics
}

impl Parse {
    pub fn syntax_node(&self) -> SyntaxNode;  // red tree root (Send + Sync via cstree)
    pub fn errors(&self) -> &[SyntaxError];
}

/// Pluggable backend so the hand-written parser and the tree-sitter MVP/oracle
/// are interchangeable behind one API. (research/02 §6 migration path.)
pub trait Parser {
    fn parse_file(&self, text: &str) -> Parse;
}

/// The destination backend (default).
pub struct HandWrittenParser;
impl Parser for HandWrittenParser {
    fn parse_file(&self, text: &str) -> Parse { /* lex → prepass → RD parse */ }
}

/// Optional week-1 backend AND permanent differential oracle (WS5).
#[cfg(feature = "tree-sitter-backend")]
pub struct TreeSitterParser;
```

The default entry point `gdscript_syntax::parse(text) -> Parse` calls the
hand-written backend. The whole crate "knows nothing about salsa or LSP"
([`research/06`](research/06-analyzer-architecture.md) §1, Layer 1).

### `SyntaxKind` (representative subset)

A single `#[repr(u32)]` enum covering **tokens** (from WS1/WS2) and **nodes**
(grammar productions). `cstree` keys green nodes by this. Representative slice:

```rust
// NOTE: cstree's RawSyntaxKind is u32 (not rowan's u16) — use #[repr(u32)] and
// #[derive(cstree::Syntax)] (see Playbook §4.1). The hand-written enum below is
// illustrative of the *kinds*; the real one derives Syntax + uses #[static_text].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum SyntaxKind {
    // ---- tokens: trivia ----
    WHITESPACE, COMMENT, DOC_COMMENT, LINE_CONTINUATION,
    // ---- tokens: synthetic block structure (from the pre-pass) ----
    NEWLINE, INDENT, DEDENT,
    // ---- tokens: literals ----
    INT, FLOAT, STRING, STRING_NAME, NODE_PATH, IDENT,
    // ---- tokens: keywords (one variant each; ~35) ----
    IF_KW, ELIF_KW, ELSE_KW, FOR_KW, WHILE_KW, MATCH_KW, WHEN_KW,
    BREAK_KW, CONTINUE_KW, PASS_KW, RETURN_KW,
    VAR_KW, CONST_KW, ENUM_KW, FUNC_KW, STATIC_KW, SIGNAL_KW,
    CLASS_KW, CLASS_NAME_KW, EXTENDS_KW,
    IS_KW, IN_KW, AS_KW, SELF_KW, SUPER_KW, VOID_KW,
    AWAIT_KW, PRELOAD_KW, ASSERT_KW, BREAKPOINT_KW,
    NOT_KW, AND_KW, OR_KW, TRUE_KW, FALSE_KW, NULL_KW,
    // ---- tokens: punctuation / operators ----
    L_PAREN, R_PAREN, L_BRACK, R_BRACK, L_BRACE, R_BRACE,
    COMMA, COLON, SEMICOLON, DOT, DOTDOT, ELLIPSIS,           // .. for open match, ... varargs
    AT, DOLLAR, PERCENT, AMP,                                  // @ $ % &
    ARROW, COLON_EQ,                                           // ->  :=
    PLUS, MINUS, STAR, SLASH, STARSTAR,                        // + - * / **
    EQ, EQEQ, NEQ, LT, GT, LE, GE,
    AMPAMP, PIPEPIPE, BANG, TILDE, PIPE, CARET, SHL, SHR,
    PLUS_EQ, MINUS_EQ, STAR_EQ, SLASH_EQ, STARSTAR_EQ, PERCENT_EQ,
    AMP_EQ, PIPE_EQ, CARET_EQ, SHL_EQ, SHR_EQ,
    ERROR,                                                     // unlexable byte
    EOF,

    // ---- nodes: top level ----
    SOURCE_FILE,
    EXTENDS_CLAUSE, CLASS_NAME_DECL, ANNOTATION,
    // ---- nodes: declarations ----
    CLASS_DECL, INNER_CLASS_DECL, FUNC_DECL, PARAM_LIST, PARAM, VARARG_PARAM,
    VAR_DECL, CONST_DECL, ENUM_DECL, ENUM_VARIANT, SIGNAL_DECL,
    PROPERTY_BODY, GETTER, SETTER,                            // get:/set: blocks
    // ---- nodes: types ----
    TYPE_REF, TYPED_ARRAY, TYPED_DICT,                        // Array[T], Dictionary[K,V]
    // ---- nodes: statements ----
    BLOCK, IF_STMT, ELIF_CLAUSE, ELSE_CLAUSE, FOR_STMT, WHILE_STMT,
    MATCH_STMT, MATCH_ARM, RETURN_STMT, BREAK_STMT, CONTINUE_STMT,
    PASS_STMT, ASSERT_STMT, BREAKPOINT_STMT, EXPR_STMT, VAR_STMT,
    // ---- nodes: match patterns ----
    PATTERN_LITERAL, PATTERN_BIND, PATTERN_WILDCARD, PATTERN_ARRAY,
    PATTERN_DICT, PATTERN_REST, PATTERN_GUARD,                // [1,..] {..} when ...
    // ---- nodes: expressions ----
    BIN_EXPR, UNARY_EXPR, TERNARY_EXPR, CAST_EXPR, IS_EXPR, IN_EXPR,
    CALL_EXPR, ARG_LIST, INDEX_EXPR, FIELD_EXPR, AWAIT_EXPR,
    LAMBDA_EXPR, PAREN_EXPR, ARRAY_LIT, DICT_LIT, DICT_ENTRY,
    NAME_REF, LITERAL, GET_NODE_EXPR, UNIQUE_NODE_EXPR,       // $Path  %Unique
    PRELOAD_EXPR,
    // ---- nodes: error recovery ----
    ERROR_NODE,
    // (keep last; count drives the u32<->kind cast)
    TOMBSTONE,
}
```

> The enum is the *single source of truth* shared by lexer, pre-pass, parser, and
> AST. tree-sitter's node types map **into** this set (WS5).

### Error-recovery strategy (resilient LL — matklad)

[`research/02`](research/02-parsing-strategy.md) §2: **"parsing never fails: the
parser returns `(Tree, Vec<Error>)`."** Two goals: **localize** errors (a broken
`func` must not corrupt highlighting of siblings) and **recognize valid
prefixes** (an incomplete `func` is still a `FUNC_DECL`). Mechanisms:

- **`MarkOpened`/`MarkClosed`** event API so nodes can be wrapped retroactively
  (e.g. promote a parsed expression into a `BIN_EXPR` once an operator is seen).
- **Recovery sets** (FIRST/FOLLOW-derived) per construct: at each loop the parser
  decides **consume** / **skip-as-error** / **bubble up**. Statement-level
  recovery resyncs on `NEWLINE` / `DEDENT` / the start-keyword set
  (`func var const class enum signal if for while match @`).
- **`ERROR_NODE`** wraps skipped tokens; a homogeneous tree means error nodes
  drop in anywhere with no special-casing.
- **`SyntaxError { range: TextRange, message: String }`** with **"expected X,
  found Y"** style messages — diagnostics quality is the point of owning the
  parser.

### Lossless guarantee

Every token (incl. **all trivia**) lands in the green tree at a deliberate
attachment point. The defining test (WS Testing): for the whole corpus,
**`parse(src).syntax_node().to_string() == src`** byte-for-byte. This is what
makes formatting/rename possible later
([`research/02`](research/02-parsing-strategy.md) §5).

### Grammar-production checklist (each maps to a parse function)

Every construct from [`research/04`](research/04-gdscript-semantics-and-features.md)
§1 → a `parse_*` fn. Expressions use a **Pratt** parser for precedence.

| GDScript construct (research/04) | Node kind | Parse fn |
|---|---|---|
| File: extends / class_name / annotations / members | `SOURCE_FILE` | `parse_source_file` |
| `class_name X` (+ optional `extends`, `@icon`) | `CLASS_NAME_DECL` | `parse_class_name` |
| `extends Base` / `extends "res://x.gd"` / `extends "x.gd".Inner` (3 forms) | `EXTENDS_CLAUSE` | `parse_extends` |
| `@name(args)` annotations (36 of them; arity-agnostic parse) | `ANNOTATION` | `parse_annotation` |
| `class Inner:` inner class; `@abstract class` | `INNER_CLASS_DECL` | `parse_inner_class` |
| `func f(a, b:=1, ...rest) -> T:` (defaults/typed/inferred/varargs) | `FUNC_DECL`/`PARAM_LIST`/`VARARG_PARAM` | `parse_func`, `parse_params` |
| single-line func body `func f(): return 1` | `BLOCK` (inline) | `parse_block` |
| Lambdas `func(x): …` / `func(x:int)->void: …` | `LAMBDA_EXPR` | `parse_lambda` |
| `var x`, `var x: T`, `var x := e`, `static var` | `VAR_DECL` | `parse_var` |
| Property `var h: int = 0: get: … set(v): …` (inline) and `get = f, set = f` (named) | `PROPERTY_BODY`/`GETTER`/`SETTER` | `parse_property` |
| `const X`, `const X: T = e`, `const X := e` | `CONST_DECL` | `parse_const` |
| `enum {…}` (anon) / `enum Named {A, B = -1}` | `ENUM_DECL`/`ENUM_VARIANT` | `parse_enum` |
| `signal s` / `signal s(a: int, b: String)` (typed) | `SIGNAL_DECL` | `parse_signal` |
| Type refs: `T`, `Array[T]`, `Dictionary[K,V]` (4.4+) | `TYPE_REF`/`TYPED_ARRAY`/`TYPED_DICT` | `parse_type` |
| `if/elif/else` | `IF_STMT`/`ELIF_CLAUSE`/`ELSE_CLAUSE` | `parse_if` |
| `for x in e:` / `for x: T in e:` | `FOR_STMT` | `parse_for` |
| `while e:` | `WHILE_STMT` | `parse_while` |
| `match` + full pattern grammar (literal/expr/`_`/`var bind`/array/dict/open-ended/multi/`when` guard) | `MATCH_STMT`/`MATCH_ARM`/`PATTERN_*` | `parse_match`, `parse_pattern` |
| `return [e]` / `break` / `continue` / `pass` / `assert(c,m)` / `breakpoint` | `*_STMT` | `parse_simple_stmt` |
| Expressions: arith/compare/logical/bitwise/compound-assign (Pratt) | `BIN_EXPR`/`UNARY_EXPR` | `parse_expr` (Pratt) |
| `is` / `is not` / `as` / `in` / `not in` | `IS_EXPR`/`CAST_EXPR`/`IN_EXPR` | (in Pratt) |
| ternary `a if c else b` (right-assoc) | `TERNARY_EXPR` | `parse_ternary` |
| `await e` | `AWAIT_EXPR` | `parse_unary` |
| call `f()` / index `x[i]` / field `x.y` | `CALL_EXPR`/`INDEX_EXPR`/`FIELD_EXPR` | `parse_postfix` |
| literals: int/float/string/multiline/raw/bool/null/array/dict (+ Lua `{k = v}`) | `LITERAL`/`ARRAY_LIT`/`DICT_LIT` | `parse_primary` |
| node sugar: `$Path`, `$"x"`, `%Unique`, `&"name"`, `^"path"` | `GET_NODE_EXPR`/`UNIQUE_NODE_EXPR`/`LITERAL` | `parse_primary` |
| `preload("…")` (parse-time keyword) | `PRELOAD_EXPR` | `parse_primary` |
| `self` / `super` / `super.m()` / `super(args)` | `NAME_REF`/`CALL_EXPR` | `parse_primary` |
| comments / `##` doc / `#region` (trivia + folding signal) | (trivia tokens) | (lexer; attached in tree) |

### Deliverable

`gdscript_syntax::parse(&str) -> Parse` on the hand-written backend, all corpus
files parsing **panic-free**, **lossless round-trip green**, every checklist row
covered by a golden CST fixture.

---

## Workstream 4 — Typed AST layer

A thin, **zero-cost typed view** over the `cstree` CST — the rust-analyzer model
([`research/06`](research/06-analyzer-architecture.md) §1: "a typed AST as a thin,
zero-cost view over the CST"). AST nodes wrap a `SyntaxNode`; accessors are
filtered child lookups. **No data is copied**; the AST is just typed navigation.

```rust
// gdscript-syntax::ast

/// Every typed node wraps a red node of the matching SyntaxKind.
pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

// generated-or-hand-written wrappers, e.g.:
pub struct FuncDecl(SyntaxNode);
impl AstNode for FuncDecl { /* can_cast == kind == FUNC_DECL ; … */ }
impl FuncDecl {
    pub fn name(&self) -> Option<Name>;            // first IDENT child
    pub fn param_list(&self) -> Option<ParamList>;
    pub fn return_type(&self) -> Option<TypeRef>;  // child after ARROW
    pub fn body(&self) -> Option<Block>;
    pub fn is_static(&self) -> bool;               // has STATIC_KW
    pub fn annotations(&self) -> impl Iterator<Item = Annotation>;
    pub fn doc_comment(&self) -> Option<String>;   // leading DOC_COMMENT trivia
}

pub struct SourceFile(SyntaxNode);
impl SourceFile {
    pub fn parse(text: &str) -> (Self, Vec<SyntaxError>);   // convenience
    pub fn items(&self) -> impl Iterator<Item = Item>;      // Item = enum of decls
}
```

- **`AstNode` for** every node kind a Tier-0 feature needs: `SourceFile`,
  `ClassDecl`/`InnerClassDecl`, `FuncDecl`, `Param`, `VarDecl`, `ConstDecl`,
  `EnumDecl`/`EnumVariant`, `SignalDecl`, `ClassNameDecl`, `ExtendsClause`,
  `Annotation`, `Block`, plus the statement/expression nodes used by folding.
- **Visitor / traversal utilities:** `preorder(&SyntaxNode)` iterator;
  `node.descendants()` / `node.children()` (cstree-provided); a small
  `ast::visit` helper that walks `SourceFile` collecting declarations (drives
  `document_symbols`). Token-at-offset (`node.token_at_offset(offset)`) drives
  completion context detection.

### Deliverable

The AST wrapper module + traversal helpers, with unit tests asserting accessors
return the right children on golden trees. wasm-safe.

---

## Workstream 5 — tree-sitter oracle & differential testing

tree-sitter-gdscript (MIT, `tree-sitter-gdscript` crate, v6.1.0 2025-11-02) is
**(a)** an *optional* week-1 MVP backend behind the `Parser` trait and **(b)** a
**permanent differential-test oracle** — **never** the grammar-of-record
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §6;
[`research/02`](research/02-parsing-strategy.md) §1, §6). It is single-maintainer,
manually synced to Godot ("some commits may have been missed"), and models
comments lossily — fine to check *against*, wrong to *depend on*.

### Wiring

- **Backend (feature-gated):** `#[cfg(feature = "tree-sitter-backend")]`
  `TreeSitterParser` implements `Parser`. It runs `tree_sitter::Parser` with the
  `tree-sitter-gdscript` language and maps tree-sitter node types → our
  `SyntaxKind` via a translation table. Feature-gated so it **never becomes
  load-bearing** and never compiles into the default/wasm build unless asked.
- **Oracle (test-only):** an `xtask differential` (and a `#[cfg(test)]` harness)
  parses each corpus file with **both** backends and compares **structurally** —
  walk both trees, compare the sequence of (kind, text-span) for **non-trivia**
  nodes (tree-sitter's `extras` comment model differs, so trivia is excluded from
  the structural diff but checked separately for our round-trip). Divergences are
  **reported, triaged, and either fixed in our parser or recorded as a known
  tree-sitter limitation** in `fixtures/differential/KNOWN_DIVERGENCES.md`.

```rust
// xtask / tests
fn assert_structurally_equivalent(src: &str) {
    let ours = HandWrittenParser.parse_file(src);
    let ts   = TreeSitterParser.parse_file(src);
    let a = normalize_skeleton(&ours.syntax_node()); // drop trivia, map kinds
    let b = normalize_skeleton(&ts.syntax_node());
    assert_eq!(a, b, "divergence in:\n{src}");
}
```

### Corpus sources

- **Godot demo projects** (`godotengine/godot-demo-projects`) — the breadth
  oracle; the Phase-1 exit corpus.
- **The guitkx examples** — real embedded-GDScript snippets our first downstream
  consumer cares about (research/06 §5).
- **The `fixtures/` suite** — hand-authored edge cases (WS2 corpus + each grammar
  row + adversarial broken-code recovery cases).
- Optionally **`atelico/gdstyle`** test inputs (another hand-written GDScript
  parser) as extra adversarial material.

### Deliverable

`cargo xtask differential` green (modulo a documented known-divergence list);
the tree-sitter backend behind its feature flag; the oracle wired into CI as a
non-blocking-then-blocking gate.

---

## Workstream 6 — `AnalysisHost`/`Analysis` skeleton (`gdscript-ide`)

The public API boundary, modeled **exactly** on rust-analyzer's
`ide::AnalysisHost`/`ide::Analysis`
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2;
[`research/06`](research/06-analyzer-architecture.md) §3). **No salsa** in Phase 1
— a plain VFS map + whole-file reparse — but **every derived computation is a
pure `(db, file) -> value` function** so the Phase-3 salsa swap is localized
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §3).

### The minimal host + snapshot

```rust
// gdscript-ide

/// MVP "database": a VFS of FileId -> text. No salsa yet.
#[derive(Default, Clone)]
struct RootDatabase {
    files: HashMap<FileId, Arc<str>>,
    // MVP parse cache; pure fn parse(db, file) memoized by hand.
    parsed: HashMap<FileId, Arc<Parse>>,
}

/// The single mutable owner of analysis state. One per project.
pub struct AnalysisHost { db: RootDatabase }

impl AnalysisHost {
    pub fn new() -> Self;
    /// The ONLY mutation entry point. MVP whole-file reparse: applying a change
    /// invalidates the cached Parse for the touched files.
    pub fn apply_change(&mut self, change: Change);
    /// Cheap, cloneable, Send snapshot for read queries.
    pub fn analysis(&self) -> Analysis;
}

/// Inputs pushed by the client (no std::fs in the library).
#[derive(Default)]
pub struct Change { pub files: Vec<(FileId, Option<Arc<str>>)> } // None = remove

/// Immutable snapshot. Cancellable<T> is preserved on the surface even though
/// MVP never actually cancels (Phase 3 makes it real).
#[derive(Clone)]
pub struct Analysis { db: Arc<RootDatabase> }

impl Analysis {
    // ---- Tier-0: implemented, real data ----
    pub fn parse(&self, file: FileId) -> Cancellable<SyntaxTreeHandle>;
    pub fn diagnostics(&self, file: FileId) -> Cancellable<Vec<Diagnostic>>;       // parse errors ONLY
    pub fn document_symbols(&self, file: FileId) -> Cancellable<Vec<DocumentSymbol>>;
    pub fn folding_ranges(&self, file: FileId) -> Cancellable<Vec<FoldRange>>;
    pub fn completions(&self, pos: FilePosition) -> Cancellable<Vec<CompletionItem>>; // by-name, NO types

    // ---- present in the architecture §2 surface; Phase-1 returns empty/None ----
    pub fn hover(&self, pos: FilePosition) -> Cancellable<Option<HoverResult>> { Ok(None) }
    pub fn signature_help(&self, pos: FilePosition) -> Cancellable<Option<SignatureHelp>> { Ok(None) }
    pub fn goto_definition(&self, pos: FilePosition) -> Cancellable<Vec<NavTarget>> { Ok(vec![]) }
    pub fn find_references(&self, pos: FilePosition) -> Cancellable<Vec<Reference>> { Ok(vec![]) }
    pub fn semantic_tokens(&self, file: FileId) -> Cancellable<SemanticTokens> { Ok(default) }
    pub fn inlay_hints(&self, file: FileId) -> Cancellable<Vec<InlayHint>> { Ok(vec![]) }
    // rename / code_actions / format: signatures present, Ok(empty) in Phase 1.
}
```

### Derived computations (pure, salsa-shaped)

```rust
// free functions, db-shaped from day one — Phase 3 wraps these in #[salsa::tracked]
fn parse(db: &RootDatabase, file: FileId) -> Arc<Parse>;
fn document_symbols(db: &RootDatabase, file: FileId) -> Vec<DocumentSymbol>;
fn folding_ranges(db: &RootDatabase, file: FileId) -> Vec<FoldRange>;
fn completions(db: &RootDatabase, pos: FilePosition) -> Vec<CompletionItem>;
```

### The four Tier-0 features

- **`diagnostics`** — map `Parse::errors` (parser) + the WS2 indentation
  diagnostics into `Diagnostic { range, code: "GDSCRIPT_SYNTAX", severity:
  Error, message }`. **Parse errors only** (research/04 §4: the one Low-difficulty
  diagnostic row).
- **`document_symbols`** — AST visitor over `SourceFile` → a nested
  `DocumentSymbol` tree (class → funcs/vars/consts/signals/enums; inner classes
  nest; enum variants nest). Kinds: Class/Method/Field/Constant/Event/Enum/EnumMember.
- **`folding_ranges`** — one `FoldRange` per indented `BLOCK` (func/class/if/for/
  while/match bodies, property accessor blocks), per multi-line bracket span, and
  per `#region`…`#endregion` pair (from the flagged comment tokens).
- **`completions`** — **by-name, no types**: (1) the ~35 **keyword** table; (2)
  after `@`, the **36-annotation** table; (3) **document-local symbol names**
  visible in the enclosing scope (params, locals, funcs, members, consts, enums,
  signals, `class_name`), gathered from the AST. Detect context from
  `token_at_offset(pos)` (e.g. immediately after `AT` → annotations only). **No
  member completion** — a `.`-receiver returns nothing in Phase 1 because it
  needs inference.

### POD result types (in `gdscript-base`)

All `serde`, byte-offset based, **no `lsp-types`**
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §2):

```rust
// gdscript-base
pub struct FileId(pub u32);
pub struct TextSize(pub u32);
pub struct TextRange { pub start: TextSize, pub end: TextSize }
pub struct FilePosition { pub file: FileId, pub offset: TextSize }
pub struct FileRange { pub file: FileId, pub range: TextRange }
pub struct LineIndex { /* newline offsets; byte<->(line,col) + byte<->UTF-16 */ }

pub enum Severity { Error, Warning, Info, Hint }
pub struct Diagnostic { pub range: TextRange, pub severity: Severity,
                        pub code: String, pub message: String }
pub struct DocumentSymbol { pub name: String, pub kind: SymbolKind,
                            pub range: TextRange, pub selection_range: TextRange,
                            pub children: Vec<DocumentSymbol> }
pub struct FoldRange { pub range: TextRange, pub kind: FoldKind } // Region | Comment | Block | Brackets
pub struct CompletionItem { pub label: String, pub kind: CompletionKind,
                            pub insert_text: Option<String> }
pub type Cancellable<T> = Result<T, Cancelled>;
```

`LineIndex` carries the **byte↔UTF-16** converter
([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §4 "bake it into `gdscript-base`").

### Deliverable

`gdscript-ide` compiling to **`wasm32-unknown-unknown`** (CI gate), the four
features returning correct data on fixtures, the rest stubbed, all queries
`Cancellable`.

---

## Workstream 7 — FFI binding (`gdscript-ffi`)

The **only** crate with napi/wasm glue (napi-rs v3 → a Node `.node` addon **and**
a wasm build from one source — [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §4;
[`research/06`](research/06-analyzer-architecture.md) §6). A **stateful
`AnalysisHandle`** keeps the host alive across edits; the JS side calls
`applyChange` + the Tier-0 queries and gets **flat JSON POD** back (serde).

```rust
// gdscript-ffi  (napi-rs v3)
#[napi]
pub struct AnalysisHandle { host: gdscript_ide::AnalysisHost }

#[napi]
impl AnalysisHandle {
    #[napi(constructor)]
    pub fn new() -> Self { Self { host: AnalysisHost::new() } }

    /// Add/replace/remove a file. text: null removes. Keeps the cache alive.
    #[napi]
    pub fn apply_change(&mut self, file_id: u32, text: Option<String>);

    // ---- Tier-0 queries: return JSON (serde -> napi/serde-wasm-bindgen) ----
    #[napi] pub fn diagnostics(&self, file_id: u32) -> Vec<Diagnostic>;
    #[napi] pub fn document_symbols(&self, file_id: u32) -> Vec<DocumentSymbol>;
    #[napi] pub fn folding_ranges(&self, file_id: u32) -> Vec<FoldRange>;
    #[napi] pub fn completions(&self, file_id: u32, offset: u32) -> Vec<CompletionItem>;
}
```

- **Flat & by-copy:** strings/structs cross the boundary **by value** (serde) —
  **never** return a full AST/CST per call, only the feature result
  ([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §4). `offset` is a **byte** offset.
- **Position note (the footgun):** the core emits **byte** offsets; the **client**
  converts to **UTF-16** (LSP/JS default) using `LineIndex`. The "hello" scripts
  demonstrate the conversion so consumers copy the right pattern
  ([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §4;
  [`research/06`](research/06-analyzer-architecture.md) §6).
- **wasm feature plumbing:** `getrandom`'s `wasm_js` backend enabled **only** in
  the wasm binding, never in core ([`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) §7).

### Acceptance artifacts

1. **`bindings/node/hello.mjs`** — a Node script: `new AnalysisHandle()`,
   `applyChange(0, "<a .gd file>")`, prints `documentSymbols(0)`. Proves the
   native `.node` path.
2. **`playground/hello.html`** — a browser page loading the wasm build,
   `applyChange` + `documentSymbols`, rendering the symbol list in the DOM.
   Proves the wasm path with **no Godot, no server-side anything**.

These two are the literal Phase-1 exit demo (ROADMAP: "a Node script and a
browser page both load the binding and get document symbols for a `.gd` file").

### Deliverable

The napi-rs v3 crate building both targets; both hello artifacts working.

---

## Testing strategy

| Layer | Test | Mechanism |
|---|---|---|
| Lexer | byte-exact round-trip; token-kind tables | `concat(tokens)==src`; per-row unit tests |
| Pre-pass | the 12-case golden corpus | `fixtures/lexer-prepass/*.tokens`, `xtask --bless` |
| Parser | **golden CST trees** per grammar row | `fixtures/parser/*.cst` (pretty-printed S-expr), blessed |
| Parser | **lossless round-trip** | `parse(src).syntax_node().to_string()==src` over the whole corpus |
| Parser | **error recovery** | broken-code fixtures assert (a) a tree is produced, (b) expected `SyntaxError`s, (c) siblings still parse |
| Differential | structural parity vs **tree-sitter** | `xtask differential` over the corpus; known-divergence allowlist |
| Robustness | **panic-free fuzzing** | `cargo fuzz` / a corpus-mutation harness — parsing **never panics** on any input (incl. random bytes, truncation, huge nesting) |
| IDE | feature correctness | golden `DocumentSymbol`/`FoldRange`/`CompletionItem`/`Diagnostic` JSON per fixture |
| Portability | **wasm32 build check** | `cargo check -p gdscript-ide --target wasm32-unknown-unknown` (CI gate, every PR) |
| FFI | smoke | the Node `hello.mjs` + the browser `hello.html` run in CI (node + a headless-browser/wasm smoke) |

Golden fixtures are the backbone: a pretty-printer renders the `cstree` tree to a
stable S-expression with byte spans; `xtask test-fixtures --bless` regenerates
goldens on intentional grammar changes, and the diff is the review surface.

---

## Exit criteria (mirrors ROADMAP Phase 1)

A testable checklist; all must be green to close Phase 1:

- [ ] **Parses the entire Godot demo-projects corpus + the `fixtures/` suite with
      zero panics** (fuzz + corpus run).
- [ ] **Lossless round-trip:** every corpus file's CST serializes back to
      **byte-identical** source.
- [ ] **Differential test vs tree-sitter passes** (structural parity, modulo the
      documented known-divergence list).
- [ ] **Error recovery** verified on the broken-code fixtures: a tree is always
      produced, `SyntaxError`s carry sensible "expected X" messages, and an error
      in one declaration does not corrupt its siblings.
- [ ] The **four Tier-0 features** (`diagnostics` = parse errors, `document_symbols`,
      `folding_ranges`, by-name `completions`) return correct golden output on
      fixtures.
- [ ] **`cargo check -p gdscript-ide --target wasm32-unknown-unknown` passes**
      (the whole core stays wasm-safe).
- [ ] A **Node script** (`hello.mjs`) and a **browser page** (`hello.html`) both
      load the binding and get **document symbols** for a `.gd` file.
- [ ] **No Godot editor anywhere in the loop** (no port 6005, no headless engine,
      no engine dependency in any crate).
- [ ] `cargo xtask ci` green locally and in Actions.

---

## Risks & mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| **Indentation edge cases** (the `:`-block/dict ambiguity, mixed tabs/spaces, dedented comments, multiline-bracket suppression) — the single biggest Phase-1 risk | High | Isolate **all** of it in the WS2 pre-pass; model on Godot's tokenizer + scanner.c; lock behavior with the 12-case golden corpus *before* writing the parser; keep `:`-semantics out of the pre-pass (parser owns block-start). |
| **`cstree` learning curve** (no in-place mutation; green/red split; interning; `TreeSink` plumbing) | Medium | Follow matklad's resilient-LL tutorial + Biome/rust-analyzer source as direct templates; build a tiny `TreeSink` + tree pretty-printer first; spike a 3-node grammar end-to-end before the full surface. |
| **tree-sitter divergence noise** (manual Godot sync, lossy comment model, `extras`) | Medium | Compare **non-trivia skeletons only**; treat divergences as triage items not hard failures; maintain `KNOWN_DIVERGENCES.md`; never let tree-sitter become load-bearing (feature-gated backend, oracle-only by default). |
| **Accidentally breaking wasm-safety** (`std::fs`, `Instant::now`, `thread::spawn`, `getrandom`) | Medium | The CI wasm32 gate runs on **every PR**; VFS-only inputs (no path reads in core); feature-gate any parallelism; `getrandom` `wasm_js` only in the binding. |
| **Scope creep into Tier 1** (someone "just adds" member completion or `:=` typing) | Medium | The "what this phase does NOT do" list is a contract; member completion / inference / engine API / cross-file are **Phase 2+** and must be rejected in review here. |
| **Grammar drift across Godot 4.x minors** (typed dicts 4.4, `@abstract`/varargs 4.5) | Low | Encode each version's surface as fixtures; the parser is permissive (parses the union) and version-specific *validation* is a later phase. |

---

## References

- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — §1 crate stack, §2 `AnalysisHost`/`Analysis` API, §6 parser model, §7 portability.
- [`ROADMAP.md`](ROADMAP.md) — Phase 1 deliverable + exit criteria; Tier 0 row.
- [`research/02-parsing-strategy.md`](research/02-parsing-strategy.md) — **primary**: lexer/parser/CST decision, the `Parser`-trait bootstrap, indentation handling (§4), lossless-vs-AST (§5), the tree-sitter verdict + migration path (§6).
- [`research/06-analyzer-architecture.md`](research/06-analyzer-architecture.md) — the layered crates, the `AnalysisHost`/`Analysis` skeleton (§3), LSP-agnostic POD boundary (§4), FFI shape (§6), MVP-vs-v1 (§8).
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — the full GDScript 2.0 grammar surface the parser must cover (§1), the 36 annotations (§1.10), the Tier-0 syntactic LSP features (§4 "MVP" rows).
- Sibling phase docs: [`PHASE-0-ECOSYSTEM-AND-TOOLING.md`](PHASE-0-ECOSYSTEM-AND-TOOLING.md) (prereqs), [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) (what Tier 1 adds on top).
