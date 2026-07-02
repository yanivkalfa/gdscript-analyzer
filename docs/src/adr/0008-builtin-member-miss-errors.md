# ADR-0008: Member misses on built-in receivers are errors (`UNDEFINED_METHOD` / `UNDEFINED_PROPERTY`)

- **Status:** Accepted
- **Date:** 2026-07-02

## Context

`c.casll()` on a `Callable` emitted only the opt-in `UNSAFE_METHOD_ACCESS` — silent under project
defaults — while Godot itself hard-errors (`Function "casll()" not found in base Callable`).
Probed on 4.7 with `--check-only`: builtin receivers error for methods **and** properties, typed
**and** `:=`-inferred alike; `Object` and script receivers stay silent (a script can attach
members at runtime); Dictionary property access is silent for **any** name, reads and writes
(keyed-subscript sugar), while a Dictionary **method** miss still errors. The TECH_DEBT
"closed-builtin receiver severity study" asked exactly this question.

## Decision

We will emit **`UNDEFINED_METHOD` / `UNDEFINED_PROPERTY` (ERROR by default)** from
`builtin_member_ty` when a member miss occurs on a built-in receiver. Unlike
`UNDEFINED_FUNCTION`/`UNDEFINED_IDENTIFIER` (ADR-0005), these need **no workspace-completeness
claim**: the builtin member tables ship with the analyzer and are closed. Gates: a project
declaring a **newer** engine than the bundled model falls back to the opt-in `UNSAFE_*` (the
member may exist there); `Nil` receivers stay silent (a nullability question, not member
existence). Keyed builtins short-circuit the **whole property path**: `d.some_key` on a
`Dictionary` is subscript sugar typed as the dictionary's **value type**, and the key wins even
over real method names (`{"size": 99}.size` is `99` at runtime — probed) — never a diagnostic.
A Dictionary **method** miss still errors: method-call sugar does not dispatch to callable
values (`d.greet()` crashes at runtime even when the key holds a `Callable` — probed), so
flagging it is a true positive. `Object` and script receivers keep the opt-in `UNSAFE_*`
(Godot parity).

## Consequences

Easier: the `.casll()` class of typo is a default-on error with a precise squiggle — including
Godot-3→4 renames (`upper()` vs `to_upper()`) — and `dict.key` now types as the value type
instead of `Variant`. Harder: strict-mode users lose the (false) `UNSAFE_PROPERTY_ACCESS` they
previously saw on Dictionary keyed access — that was a pre-existing false positive, now fixed;
and a project on a newer engine gets the weaker opt-in signal for genuinely-new members, by
design. Corpus proof: the first run surfaced 111 `UNDEFINED_PROPERTY` hits across 138
godot-demo-projects — all of them the Dictionary sugar shape — and zero after the keyed
short-circuit; zero `UNDEFINED_METHOD` false positives throughout.
