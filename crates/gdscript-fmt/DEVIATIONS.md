# `gdscript-fmt` vs `gdformat` — deviations & parity status

The reference formatter for GDScript is **`gdformat`** (part of `gdtoolkit` /
`Scony/godot-gdscript-toolkit`). This crate aims for behavioural parity while keeping its
**safe-by-construction** guarantee (re-emit the token stream; never change the significant token
sequence; re-parse and fall back to the verbatim source if anything looks off). This document
records where we match, where we deliberately differ, and what is not yet implemented.

## How parity is measured

`gdformat` is run as a golden oracle (via `uvx --from gdtoolkit gdformat`) over a corpus of real
`.gd` files (`godot-demo-projects` + `ReactiveUI-Gadot`, ~545 files). Two metrics:

- **exact match** — `format(original)` byte-equals `gdformat(original)` (EOL-normalised, see §1).
- **fixpoint** — `format(gdformat(original))` equals `gdformat(original)`: do we *preserve*
  gdformat's own output? This isolates our remaining gaps from the wrapping we don't yet do.

As of the CST-driven wrapping port (a faithful port of gdformat 4.5's `expression.py`; see
`src/wrap.rs`) plus the full normalization set (inline-suite expansion, blank-line + comment rules,
node-path / triple-quote / BOM, dict-entry & magic-comma chains), EOL-normalised:

- **exact match**: **454/456 (99.6%)** over `godot-demo-projects` (up from ~14% at the start of
  Phase 4), **88/88 (100%)** over the denser, React-like `ReactiveUI-Gadot` library code (up from ~2%)
- **fixpoint** `format(gold)==gold`: **455/456 (99.8%)** godot, **88/88 (100%)** ReactiveUI

**Comments are threaded through a reshaped statement** (a faithful port of gdformat's comment model):
a standalone comment keeps its own line at the block / element indent, a trailing inline comment is
appended to its statement's line with the two-space offset, a comment trailing the open bracket of a
list stays on the open line, and a comment trailing the whole statement lands on the rendered last
line. A *standalone* comment strictly inside a construct forces it multi-line (gdformat's
`_has_standalone_comments`). It is made safe-by-construction by a **comment-multiset net** in
`render()` (separate from the meaning net, which treats comments as trivia): if any comment cannot be
placed, the whole statement falls back to verbatim — so this can only improve, never lose a comment.
**Multi-line strings** are rendered verbatim inside an exploded operator chain / paren-wrap
(`x = (\n"""…"""\n% [...]\n)`), with the throwaway-parse re-indent skipping string-interior lines.

Comment-around-reshape placement is now matched across the board: a blank line at the **start of a
block** (between a compound header and its body) is stripped; a block-boundary **standalone comment**
is placed at the depth of the deepest open block containing it; standalone comments are threaded
through an **operator-chain** wrap (at the operand indent) and through a **call argument list**,
including the hard case where a comment **hangs off a lambda argument's body** (dedented between the
lambda and the next argument — re-indented to the body's depth, with the argument separator `,` landing
on the lambda's last body line before its trailing comment); a **trailing comment never forces a wrap**
(gdformat measures width without it, so a fitting author-wrapped statement collapses); and a **column-0
comment amid a function body** (commented-out code) is kept inside the function rather than gaining
def-separating blank lines.

The remaining exact-match gap on `godot-demo-projects` is **2 files**, both **deep wrap-choice
nuances** (not comment / blank-line): `town_scene.gd` — gdformat keeps redundant sub-expression parens
in `(A) or (B)` that our `strip_parens` removes, then threads the operator-chain comments around them;
`os_test.gd` — a subscript on a large array literal (`[ … big array … ][index]`) where gdformat
explodes the array one-element-per-line while we keep it compact and drop the subscript below. Plus
gdformat's own **BOM** limitation (it errors on a leading BOM and leaves the file unchanged, so its
"gold" is just the source — we reformat it and legitimately differ; these are excluded from the counts
above).

The CST wrapper additionally renders **lambdas** (inline `func(p): body`, multi-line bodies with
recursively-formatted nested blocks, and the contains-a-lambda dot-chain bottom-up rule), drops a
colon dict-entry's value below its key when the entry overflows, splits a long annotated declaration's
annotation onto its own line, and re-lays-out multi-line lambda-bearing statements. Two parse-tree
pre-passes were added: an inner class's `extends` is moved to its own body line
(`class C extends B:` → `class C:` / `extends B`), and `;`/inline-suite expansion now descends
correctly through inner-class and lambda bodies. A parser fix lets a dict value sit on the next line
(`"k":\n v`).

Corpus safety (all 545 clean files, `safe_mode` OFF): **0** non-parsing outputs, **0**
token-sequence changes, **0** idempotence breaks. The safety net is never the thing that makes us
correct on this corpus — the passes are correct on their own.

The remaining exact-match gap is almost entirely **deep wrap-choice nuances**: on a heavily-nested
expression (3–5 levels of mixed call / operator / collection), gdformat's wrapper sometimes injects a
redundant grouping paren or picks a different (but valid, ≤ width, meaning-preserving) split point
than our simpler recursion. These show up as a single differing sub-wrap deep inside an otherwise
byte-identical statement, and `format(gold)==gold` stays ~98–99% (we *preserve* gdformat's choice
even where we wouldn't make it on raw input).

## What we match

- Block **indentation** (to the configured unit; tabs by default).
- **Intra-line spacing** (increment A): one space around binary operators / assignments / `->`
  / `:=`, after `,` and dict/type-annotation `:`, hugged brackets, tight member access, tight unary.
- **Blank-line collapse**: every run of blank lines is squeezed to **one** (gdformat's
  `_squeeze_lines`); the second / first blank is then re-added only around definitions.
- **Blank-line insertion**: 2 blank lines around top-level `func`/`class`/`static func`, 1 around
  nested ones; comment/annotation prefixes move with their def; no blank before the first member of a
  block; an **annotation-prefixed** def is forced apart only from a *preceding* def; a *trailing*
  comment (a closing `#endregion`) keeps its source blanks.
- **Inline-suite expansion**: a compound statement's inline body (`if c: x`, `func f(): return`,
  `else: c()`), an inline `match` arm body, a property getter/setter body / shorthand, and
  `;`-separated statements are each moved to their own indented line — while an inline **lambda** body
  (`func(): x`) is preserved (parse-tree-driven; see `expand_inline_blocks`).
- **Node paths** (`$%Unique`, `$A/%Unique`) stay tight, **triple-single-quoted single-line strings**
  collapse to regular (`'''x'''` → `"x"`), a leading **BOM** is preserved, and a soft-keyword member
  call (`obj.match(x)`) hugs its `(`.
- **Redundant grouping parens** are stripped from a standalone-expression position (`return (x)` →
  `return x`, `for i in (a):` → `for i in a:`, `g((x))` → `g(x)`), while precedence-bearing parens
  (`(a + b) * c`) are kept; an **empty signal parameter list** is removed (`signal s()` → `signal s`);
  **backslash line continuations** are collapsed and the statement re-wrapped (gdformat converts a
  `\`-continued operator chain to paren-wrapped multi-line).
- **Block-boundary comment indentation** (increment C): a comment is placed at its intended depth
  (authored indentation clamped to the surrounding structure), matching gdformat — a column-0 comment
  stays at column 0, an over-indented one snaps to the block.
- **Inline-comment offset**: a trailing `# comment` is offset by exactly two spaces (gdformat's
  `INLINE_COMMENT_OFFSET`), regardless of the original spacing.
- **String-quote normalisation**, **magic trailing comma**, **operator-chain wrapping**, and
  **enum-brace spacing** — see the token-mutating section below; all byte-identical to gdformat.

## Deliberate deviations

1. **Line endings (EOL).** We **preserve** the source's line-ending style (LF stays LF, CRLF stays
   CRLF). `gdformat` normalises to the *platform* line ending — on Windows it emits CRLF even for an
   LF source. Preserving the input never churns every line of a checkout, so we deviate on purpose.
   (This is also why the corpus "exact match" is measured EOL-normalised: the Windows oracle output is
   CRLF while the LF originals are not.)

## Implemented — line reflow / wrapping (length-driven)

A **single-line** statement that does not fit in `line_width` (default 100) and contains a bracketed
group (`(...)`, `[...]`, `{...}`, or a function parameter list) is wrapped via a `Doc`-IR that tries
**flat → compact → exploded** with a width check — **token-preserving** (no trailing comma added),
byte-identical to gdformat on the corpus:

- **compact** — open bracket stays on the line, **all** elements on **one** indented continuation
  line (`", "`-separated), close bracket on its own line. Used when that line fits.
  ```gdscript
  var x = some_function(
      argument_one, argument_two, argument_three, argument_four, arg_five
  )
  ```
- **exploded** — when even the compact line is too long: **one element per line**, close on its own
  line, **no** trailing comma; elements are reflowed recursively (a nested group that fits stays
  inline).

Gated behind `FmtConfig::reflow` (default on). *Only single-physical-line statements are reflowed*
(an already-wrapped statement is preserved) — this keeps the pass trivially idempotent. Statements
that are **left unwrapped** (a documented gap, not a bug): those with a magic trailing comma (see §2),
a long bracketless operator chain (§3), an inline comment, or no bracket group at all.

## Token-mutating behaviours — guarded by the meaning-equivalence net

Once the formatter rewrites the token sequence, the safety net relaxes from exact-token-equality to
**meaning-equivalence** (`meaning_preserved`): it normalises away exactly the differences gdformat is
allowed to introduce — a **trailing comma** before a closing bracket is dropped, and **string
literals are compared by their canonical quote form** — while still catching a dropped/added real
token or a changed string *value*.

- **String-quote normalisation — IMPLEMENTED.** gdformat's (Black's) rule: prefer `"`, fall back to
  `'` only when the body has more `"` than `'` (fewer escapes); prefixes (`r`/`&`/`^`/`$`) and the
  decoded value are preserved; re-escaping handled. Byte-identical to gdformat on the probe cases.
  Gated behind `FmtConfig::normalize_strings`. *Gap:* triple-quoted strings (`'''…'''`) are left
  verbatim (rare; gdformat would switch them to `"""`).

- **Magic trailing comma — IMPLEMENTED.** A source trailing comma (`call(a, b, c,)`) forces the
  group **exploded one-per-line with** the comma kept, even when it fits; a magic comma anywhere forces
  every enclosing group multi-line (an enclosing group that is not itself magic gets no trailing comma).
  Byte-identical to gdformat on the probe cases (incl. nested + lua-style dicts). Guarded by the
  meaning-equivalence net (which treats trailing commas as removable, so the rewrite is safe).

- **Operator-chain wrapping — IMPLEMENTED.** A too-long statement whose expression (an `if`/`elif`/
  `while` condition, a `return` value, or an assignment RHS) is a top-level **binary-operator chain**
  is wrapped in injected parens, breaking at the **lowest-precedence** operator, operator-leading:
  ```gdscript
  if (
      condition_one
      and condition_two
  ):
  ```
  Mixed precedence keeps the tighter groups inline (`a and b` / `or` / `c and d`). An expression with
  no top-level binary operator but a bracketed group (a long method chain) wraps **compact** (one
  indented continuation line). Byte-identical to gdformat on the probe cases. Guarded by the
  meaning-equivalence net (which unwraps the redundant parens). **Node-paths are never split** — a
  `$Node/Path`'s `/` are path separators, not division.

- **Enum-brace spacing — IMPLEMENTED.** An enum body is spaced inside (`enum E { A, B }`) while a dict
  stays tight (`{"k": v}`) and an empty enum is tight (`{}`). Byte-identical to gdformat.

- **Layout ownership — IMPLEMENTED (CST-driven port of gdformat's wrapper).** The reflow re-lays-out
  *every* statement. A statement the author wrapped across lines is collapsed when it now fits; one
  that does not is re-wrapped by a faithful port of gdformat 4.5's `expression.py` algorithm (see
  `src/wrap.rs`), driven from our own parse tree: each (sub-)expression is rendered single-line if it
  fits, else exploded — a call/array/dict/parameter-list explodes its comma-separated elements
  (compact on one continuation line when they fit, else one-per-line); an operator chain wraps in
  injected parens and breaks operator-leading; a method chain either wraps its final call's arguments
  (bottom-up) or, when even the compact chain overflows, explodes at each `.` (leading-dot, `. method`).
  A `func`/`signal` header wraps its parameter list with the `-> ReturnType:` kept as a suffix. Every
  output is self-validated as meaning-equivalent to the input (redundant parens / trailing commas /
  string quotes allowed) before use, falling back to the previous heuristic otherwise. Corpus-safe
  (0 non-parsing / 0 token changes / 0 idempotence breaks, safe_mode off). Byte-exact match ~66%
  (godot) / ~46% (ReactiveUI); `format(gdformat-output)` fixpoint ~95% (godot) / ~94% (ReactiveUI).

The remaining `format(gold) != gold` tail (each low-frequency):

- **`#region` / `#endregion` comment indentation.** gdformat keeps a column-0 comment at column 0; our
  block-boundary policy re-indents it to the enclosing block.
- **`$%UniqueName` node-path spacing.** The combined `$%` sigil is spaced to `$ %` by the intra-line
  spacing pass (the node-path state machine does not carry through a `%` after `$`).
- **Assorted wrap-choice nuances** on deeply-nested mixed call/operator statements, where gdformat's
  per-construct tuning picks a different (but valid, ≤ width, meaning-preserving) split than ours.
- **Triple-quoted strings** (`'''…'''`) are left verbatim; gdformat rewrites them to `"""…"""`.
