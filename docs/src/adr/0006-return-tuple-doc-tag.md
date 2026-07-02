# ADR-0006: `## @return-tuple(...)` doc-tag and the synthesized `Ty::Tuple`

- **Status:** Accepted
- **Date:** 2026-07-02

## Context

React-style GDScript libraries return fixed-shape pairs — ReactiveUI's `useState` returns
`[value, setter: Callable]` — but GDScript has no tuple syntax: the best possible annotation,
`-> Array`, erases the per-position types, so `useState(0)[1]` types as `Variant` and a typo'd
setter method (`.casll()`) is uncheckable. Options considered: (a) hardcode the known library
signatures in the analyzer (couples a standalone "Roslyn for Godot" to one third-party library);
(b) infer function return shapes from `return [a, b]` bodies (violates the annotation-only return
invariant and changes typing project-wide); (c) a declaration channel libraries opt into.

## Decision

We will support a **`## @return-tuple(T0, T1, …)` doc-comment tag** on any `func` — inert in
Godot (a comment), so annotating never breaks a real build — parsed into the item tree and
resolved by one shared mapping (`resolve::resolve_tuple_return`) on both the same-file and the
cross-file (script member table) call paths. It produces a new **`Ty::Tuple(Vec<Ty>)`**: a
synthesized, source-only positional type no annotation can name, **widen-only** everywhere
non-positional — it assigns exactly as its runtime `Array[Variant]` form, labels as `Array`,
iterates as `Variant`, and exposes `Array`'s methods — while a **constant integer index**
projects the element's real type.

## Consequences

Easier: `sliced[1].casll()` is checkable wherever the call resolves to the tagged signature
(direct indexing, `:=`-inferred locals, cross-file member calls), and any library can adopt the
convention without analyzer changes. Harder: an UNTYPED `var s = useState(0)` local is a
`Variant` variable by GDScript semantics (only `:=` infers) — Godot cannot check through it and
neither do we until assignment-carried flow narrowing lands (tracked in `TECH_DEBT.md`); and the
widen-only rule means a tuple never *rejects* anything its array form would accept, so the tag
can sharpen but never break existing code.
