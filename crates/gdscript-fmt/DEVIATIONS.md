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

As of Phase 4C (blank-line insertion + boundary-comment indentation + EOL preservation +
length-driven reflow), EOL-normalised:

- exact match: **~51%** over `godot-demo-projects` (455 files; up from ~14% before 4C), **~33%** over
  the denser `ReactiveUI-Gadot` library code (up from ~2%)
- fixpoint `format(gold)==gold`: **426 / 455** token-compatible godot files (was 70)

Corpus safety (all 544 clean files, `safe_mode` OFF): **0** non-parsing outputs, **0**
token-sequence changes, **0** idempotence breaks. The safety net is never the thing that makes us
correct on this corpus — the passes are correct on their own.

## What we match

- Block **indentation** (to the configured unit; tabs by default).
- **Intra-line spacing** (increment A): one space around binary operators / assignments / `->`
  / `:=`, after `,` and dict/type-annotation `:`, hugged brackets, tight member access, tight unary.
- **Blank-line collapse** (increment B): ≤2 at top level, ≤1 nested; leading blanks stripped.
- **Blank-line insertion** (increment C): 2 blank lines around top-level `func`/`class`/`static func`,
  1 around nested ones; comment/annotation prefixes move with their def; no blank before the first
  member of a block.
- **Block-boundary comment indentation** (increment C): a comment is placed at its intended depth
  (authored indentation clamped to the surrounding structure), matching gdformat — a column-0 comment
  stays at column 0, an over-indented one snaps to the block.

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

Still not implemented:

2. **Magic trailing comma.** A source trailing comma (`call(a, b, c,)`) forces a collection to stay
   **exploded one-per-line, with** the trailing comma kept — even when it would fit on one line. We do
   not yet add/keep trailing commas (the reflow leaves such a statement unwrapped).
3. **Operator-chain wrapping.** A long bracketless boolean/arithmetic condition is wrapped by
   **injecting parentheses** and breaking operator-leading:
   ```gdscript
   if (
       condition_one
       and condition_two
       and condition_three
   ):
   ```
   This adds `(`/`)` tokens (needs the net extended to ignore redundant grouping parens). Not yet done.
4. **Leading-dot padding on wrapped method chains.** In a wrapped dot-chain gdformat emits `. method`
   (a space after the leading `.`). We keep member access tight.

### Smaller gaps

- `#region` / `#endregion` comments are treated as ordinary comments by the blank-line policy; we do
  not special-case them.
