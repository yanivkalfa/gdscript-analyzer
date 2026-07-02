# ADR-0007: Initializer narrowing for effectively-single-assignment untyped locals

- **Status:** Accepted
- **Date:** 2026-07-02

## Context

An untyped `var s = useState(0)` local is a `Variant` variable by GDScript semantics (only `:=`
infers), so nothing checks through it — the verbatim user shape that motivated the `@return-tuple`
work (`sliced[1].casll()`) stayed silent even after ADR-0006 landed. Options considered:
(a) assignment re-narrowing through the flow framework — assessed post-1.0: flow runs
*pre-inference*, so carrying the RHS's inferred type into the facts needs a flow⇄inference
fixpoint; (b) telling users to write `:=` — a documentation answer to a tooling problem; (c) a
sound subset that needs no fixpoint.

## Decision

We will narrow an untyped local's binding type to its **initializer's inferred type** exactly when
the local is **effectively single-assignment**: one pre-pass over the body's expression arena
(`collect_rebound_names`, lambda bodies included) proves the name is never the root of a plain
rebind (`x = …`, compound included) nor of an index-store chain (`s[1] = …`, which can re-type a
tuple position). A `Field` step anywhere in a write target (`v.x = 1`) mutates the value, not the
binding, and does not invalidate. Uninformative initializers (`null`, the cross-file seam,
`Variant`) never narrow — Godot is silent on untyped nulls (verified on 4.7) and nullability is
not this ADR's question. Because eligible locals are single-assignment, the narrowed type is the
only type the binding can ever hold: it is stored directly as the binding type, with no joins, no
invalidation points, and no interaction with the flow framework.

## Consequences

Easier: `var s = useState(0)` projects `s[1]` as a checkable `Callable` (with ADR-0006 +
ADR-0008 this makes the typo'd setter a default-on error, cross-file); hover/inlay show the real
type instead of `Variant`. This is a beyond-Godot value-add of the same class as `is`-narrowing
(Godot itself never checks through untyped locals). Harder: a rebound or index-stored local
soundly falls back to `Variant` — checking those needs the assessed flow⇄inference fixpoint,
which this ADR deliberately does not attempt. Aliased content mutation (`var t = s; t[1] = x`,
or `f(s)` where the callee index-stores the array it received by reference) cannot rebind the
local but CAN re-type a tuple position, so a second arena pass (`collect_escaping_names`) widens
any tuple-typed binding whose bare name appears outside the alias-free read positions (an
`Index` base, a `Field` receiver, a `Call` callee, a `return` operand) to its runtime
`Array[Variant]` form — positional projections never survive a possible aliased store, for
`:=`-inferred tuples as well. Corpus over 138 godot-demo-projects: zero new diagnostics versus
the 0.5.3 baseline.
