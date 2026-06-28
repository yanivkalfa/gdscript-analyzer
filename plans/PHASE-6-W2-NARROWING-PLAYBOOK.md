# Phase 6 · Workstream 2 — Full Control-Flow Narrowing Playbook

> Build plan for the **real CFG + flow-facts narrowing** that beats the Godot engine checker on
> `is`/`as`/`!= null` guards ([#93510](https://github.com/godotengine/godot/issues/93510), proposal
> [#8530](https://github.com/godotengine/godot-proposals/issues/8530)). Replaces the Phase-3/M4
> **widen-only, lexical, tree-walking** narrowing in `crates/gdscript-hir/src/infer.rs` with a
> per-body control-flow graph, a `Place`/`FlowFacts` dataflow, and a salsa-cached `flow(body)` query.
> Matches the depth/format of the Phase-5 playbooks; grounded in the **actual current code** (paths +
> line ranges + real symbols), not the plan's sketches.
>
> **Parent docs:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 2,
> [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) §3.2 (the
> binder/checker CFG split), §1.6 (`is`/`as` weakness in Godot), §1.7 (`UNSAFE_*` family + verbatim
> messages), §7 (the Tier-3 multi-year tail).
>
> **⚠️ Plan-vs-code corrections.** The plan's §2.1 sketch says `EcoString` and a fresh `flow.rs` with a
> `Place::Field(Box<Place>, EcoString)`. **The workspace uses `smol_str::SmolStr`, never `EcoString`**
> (same correction the Phase-4 M0 playbook flagged). Read every `EcoString` below as `SmolStr`. The
> plan also implies CFG facts replace *all* narrowing; in fact the code **already has** a working
> `narrowing: FxHashMap<String, Ty>` + `apply_narrowing` + `in_branch` + `is_subtype` we **extend and
> formalize**, not greenfield. The 1.0 cut is "make the existing intuition a real, sound, multi-construct
> dataflow," not "rewrite inference."

---

## 0. The one-line thesis

The hard machinery the plan assumes is **partly already here**: `infer.rs` already does flow-scoped
`is`-narrowing via a string-keyed `narrowing` map, a save/restore `in_branch` frame, an `is_subtype`
that composes `ScriptRef` chains with the engine table, and the load-bearing **`is_uninformative`
guard** (`Variant`/`Unknown`/`Error` never narrows, never fires `UNSAFE_*`). What's missing is the
**graph**: there is no CFG, so narrowing dies at the lexical edge — it doesn't survive an `else`, an
early `return`, an `and`/`or` short-circuit, a `match` arm, or feed `UNREACHABLE_*`. W2 builds the
graph **once per body** (`flow(body)`), threads `FlowFacts` along edges, and has the checker consult
in-scope facts instead of the ad-hoc lexical map. Soundness is fixed at 1.0 (conservative widen,
never narrow wrongly); precision is the post-1.0 tail.

---

## 1. Goal & scope — the 1.0 cut vs the deferred tail

### 1.0 ships (the contractual subset)

A real **control-flow graph per function body**, a forward dataflow producing **`FlowFacts`** per
program point, and a checker that consults those facts. Concretely, flow-sensitive narrowing of
**locals and shallow `self`/field places** through:

| Construct | 1.0? | Beats engine? |
|---|---|---|
| `if x is T:` → then-branch `x: T` | ✅ | **yes** (#93510): `UNSAFE_*` on `x.member` suppressed in then |
| `if x is T:` → else-branch `x: Not(T)` (best-effort) | ✅ | parity |
| `var t := x as T` / `(x as T).m()` | ✅ (already works) | parity (idiomatic) |
| `if x != null:` / `if x:` (object) → then `x: NotNull` | ✅ | yes |
| `if x == null: return` → after-guard `x: NotNull` (early return) | ✅ | **yes** — the early-return idiom |
| `x is T and x.foo` (and short-circuit) | ✅ | **yes** |
| `x == null or x.foo` (or short-circuit) | ✅ | **yes** |
| `match x: T(): … _: …` arm body → `x: Is(arm type)` | ✅ | **yes** |
| reassignment `x = other` → invalidate / re-narrow from `other` | ✅ (soundness) | — |
| opaque call possibly mutating `self`/by-ref → invalidate `self`-members | ✅ (soundness) | — |
| `UNREACHABLE_CODE` (stmts after `return`/`break`/`continue`/exhaustive) | ✅ — falls out of CFG | feeds W1 |
| `UNREACHABLE_PATTERN` (`match` arm after wildcard/bind) | ✅ — falls out of CFG | feeds W1 |

### Deferred (post-1.0, continual — [`research/09`](research/09-type-system-and-inference.md) §7)

- **Narrowing through arbitrary call results.** `get_thing() as T` is fine (the `as` produces the
  type directly); `if get_thing() is T:` is **not** narrowed — the place isn't stable (no `Place`).
- **Loop-carried fixpoints over back-edges.** We do **one** forward pass; a `while`/`for` body is
  entered with facts *widened* to the pre-loop join (no iteration to fixpoint). We never narrow a
  loop body on the strength of a previous iteration.
- **Aliasing.** Two locals pointing at the same object; mutating through one does not narrow/invalidate
  the other beyond the conservative opaque-call rule.
- **Discriminated-union / enum-tag narrowing** (`if tag == FOO:` refining a union by an `int`/enum
  discriminant).

### The 1.0 soundness invariant (the honest line)

> **Narrowing is conservative: when unsure, widen to the declared/`Variant` type, so we NEVER narrow
> wrongly.** A wrong narrowing would *hide a real `UNSAFE_*`* or *assert a member that isn't there* —
> both are correctness regressions worse than the engine's over-warning. **Soundness is frozen at 1.0;
> precision is the multi-year tail and ships as a quality change (MINOR/PATCH, not an API break — see
> [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §6.2).** This is the existing `is_uninformative` +
> widen-only discipline (infer.rs:1743-1763), generalized to the whole CFG.

---

## 2. Current state — what EXISTS today vs the gap

### 2.1 What exists (real paths + symbols)

**The narrowing that ships today lives entirely in `crates/gdscript-hir/src/infer.rs`** and is
lexical (tree-walking), not graph-based:

| Symbol | Location | What it does |
|---|---|---|
| `Cx.narrowing: FxHashMap<String, Ty>` | infer.rs:492 | the active narrowing env — **string-keyed dotted path** (`"x"`, `"self.field"`, `"a.b.c"`) |
| `Cx::apply_narrowing(cond)` | infer.rs:1743-1763 | reads an `Expr::Is { negated: false, ty: Some }` cond, computes `narrow_key(operand)`, **widen-only** inserts (only if `narrowed` is a subtype of current, or current is uninformative) |
| `Cx::narrow_key(id)` | infer.rs:1767-1777 | builds the dotted-path `String` key for `Name`/`SelfExpr`/`Paren`/`Field`; `None` for anything else |
| `Cx::in_branch(f)` | infer.rs:1818-1823 | clone `narrowing` on enter, **restore on exit** — the per-branch frame |
| `Cx::is_subtype(sub, sup)` | infer.rs:1412-1419 | `Object/Object` via `api.is_subclass`; `ScriptRef/ScriptRef` via `script_is_subtype`; `ScriptRef/Object` via `script_extends_engine` (all depth-bounded ≤32) |
| `Cx::resolve_name` narrowing check | infer.rs:1648-1653 | **narrowing wins over the binding's declared type**: `if narrow_key && narrowing.get(key) → return it` |
| reassignment invalidation | `infer_local_var` infer.rs:720 (`self.narrowing.remove(v.name)`); `infer_assign` infer.rs:1106-1113 (re-narrow bounded by slot) | already invalidates/re-narrows on assignment |

**Where `apply_narrowing` is called** (the only narrowing entry points today): `Stmt::If` then-branch
(infer.rs:599-601) and each `elif` (infer.rs:603-609). **Not** the `else` branch (infer.rs:610-612 is
a bare `in_branch` with no facts), **not** `while` (infer.rs:614-617), **not** `match`
(infer.rs:635-659 — arms get a fresh `in_branch` but **zero** scrutinee narrowing), **not** `and`/`or`
(no short-circuit narrowing anywhere), **not** early-return (no flow-past-guard reasoning at all).

**The `Stmt`/`Expr`/`Block` shape the CFG lowers from** (`crates/gdscript-hir/src/body.rs`):

- `Stmt` (body.rs:371-414): `Expr(ExprId)`, `Var(LocalVar)`, `Return(Option<ExprId>)`,
  `If { cond, then_branch: Block, elifs: Vec<(ExprId, Block)>, else_branch: Option<Block> }`,
  `While { cond, body }`, `For(ForLoop)`, `Match { scrutinee, arms: Vec<MatchArm> }`, `Break`,
  `Continue`, `Pass`, `Assert(Option<ExprId>)`. **`Block = Vec<StmtId>`** (body.rs:27).
- `Expr` (body.rs:184-303): includes `Is { operand, ty: Option<AstPtr>, negated }` (body.rs:244-252),
  `Cast { operand, ty }`, `Bin { op: BinOp, lhs, rhs }` where `BinOp::{And,Or,Eq,Ne}` exist
  (body.rs:52-93), `Field { receiver, name, name_range }`, `SelfExpr`, `Name(SmolStr)`, `Literal`
  (incl. `Literal::Null`, body.rs:45).
- `Body` (body.rs:452-467): `exprs`, `stmts`, `params`, `block: Block`, `tail: Option<ExprId>`,
  `source_map: BodySourceMap`. `body.stmt(id)` / `body.expr(id)` accessors (body.rs:472-481).
- `BodySourceMap::expr_range(id)` (body.rs:426-428) maps `ExprId → TextRange` — the diagnostic anchor.
  **There is no `StmtId → range` map today** — W1's `UNREACHABLE_CODE` needs a statement range; see §3.6.

**The salsa wiring** (`crates/gdscript-hir/src/queries.rs`): `#[salsa::tracked] fn analyze_file(db,
file: FileText) -> Arc<FileInference>` (queries.rs:36-47) is the cache boundary; it calls the pure
`crate::infer::analyze_file` which lowers each func body (`body::body_of_func`) and runs `infer(...)`
inline. **There is no separate per-body query** — bodies are lowered and inferred within
`analyze_file`. The `script_class` (queries.rs:252-304) and `scene_context` (queries.rs:392-403)
queries show the **offset-free firewall idiom** the new `flow` query must respect.

**The Db + durability model** (`crates/gdscript-db/src/lib.rs`): `FileText` is the salsa input
(text LOW, `res_path` MEDIUM); `parse(db, file)` (db lib.rs:172-175) is the CST query. `analyze_file`
is the heaviest derived query and the firewall test `body_edit_does_not_invalidate_signature_queries`
(queries.rs:471-499) is the gate the flow query must not break.

**Consumption** (`crates/gdscript-ide/src/semantic.rs:47-50`): `type_diagnostics` = `analyze_file(db,
file).diagnostics.clone()`. `Analysis::diagnostics` (gdscript-ide/src/lib.rs:188-194) merges
`features::diagnostics` (syntax) + `semantic::type_diagnostics`. So **anything the new flow facts emit
must land in `FileInference::diagnostics`** to surface — no new client wiring needed.

### 2.2 The gap (what's genuinely missing)

1. **No CFG.** No `BasicBlock`/`Terminator`, no successor edges. Narrowing is implicit in the recursive
   `infer_stmt`/`infer_block` walk, so it cannot survive a join, an else, an early return, or model
   reachability.
2. **No `Place` abstraction.** The narrow key is a `String` (`"a.b.c"`), which (a) allocates per lookup,
   (b) can't be compared structurally for invalidation ("does this assignment touch `x` or any
   `x.*`?"), and (c) conflates a local `x` with a member `x` shadowing it.
3. **No `FlowFacts` algebra.** Only `Is(T)` is modeled (as a bare `Ty` in the map). There is no
   `NotNull`, no `Not(T)`, no join/intersection at merge points — `else`/`elif` chains and `match`
   defaults get nothing.
4. **No reachability → no `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN`.** These warnings (W1 needs them;
   [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §1.2 marks both "needs the CFG") have **no
   implementation** — grep confirms only the `SyntaxKind`/mentions exist, no emitter.
5. **No salsa cache for flow.** Lowering+inference is monolithic inside `analyze_file`; there's no
   `flow(body)` the checker (or W1) can query independently.

---

## 3. Design — the CFG, the Place, the FlowFacts, the query

The design borrows TypeScript's **binder/checker split** ([`research/09`](research/09-type-system-and-inference.md)
§3.2): a *binder* builds the per-body CFG; the *checker* computes a narrowing environment per reachable
point. We keep the existing **bottom-up `infer.rs` walk** as the checker and bolt the CFG + facts onto
it — the minimum disruption that gets the cross-construct behavior.

### 3.1 New module: `crates/gdscript-hir/src/flow.rs`

A new sibling of `body.rs`/`infer.rs`. Pure (`fn(&Body) -> FlowGraph`), no engine API, no `&dyn Db`, no
types — exactly like `body.rs` lowering, so it stays the **cache body** of the salsa query (§3.5).

```rust
//! Per-body control-flow graph + flow-fact dataflow (Phase-6 W2). Pure over a lowered `Body`:
//! no engine API, no types — types are layered by the checker (`infer.rs`) consulting the facts.

use crate::body::{Body, Block, Expr, ExprId, Stmt, StmtId, BinOp, Literal, UnOp};
use crate::cst::AstPtr;
use smol_str::SmolStr;

/// A block id into `FlowGraph::blocks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// One basic block: straight-line stmts then a terminator that fans out successors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub stmts: Vec<StmtId>,
    pub term: Terminator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    /// Fall through to one successor.
    Goto(BlockId),
    /// A two-way branch on a condition (if / while-head / and-or / ternary). The condition `ExprId`
    /// is what the checker re-evaluates to derive the then/else fact sets.
    Branch { cond: ExprId, then_bb: BlockId, else_bb: BlockId },
    /// A `match`: each arm has its pattern set + body block; `default` is the wildcard/`_` arm.
    Match { scrutinee: ExprId, arms: Vec<MatchEdge>, default: Option<BlockId> },
    /// `return` — no in-body successor (an exit edge).
    Return,
    /// Statically unreachable (block with no predecessors). Drives UNREACHABLE_CODE.
    Unreachable,
}

/// One `match` arm's edge: the patterns guarding it + the body block it enters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEdge {
    /// Type-test patterns in this arm (`SomeType:` → narrow scrutinee to that type). Non-type
    /// patterns (literals, bindings, arrays) carry `None` — no narrowing, but still an edge.
    pub pat_ty: Option<AstPtr>,
    /// `true` if this arm is a wildcard/bind (`_` or `var x`) — makes later arms UNREACHABLE_PATTERN.
    pub is_wildcard: bool,
    pub body: BlockId,
}

pub struct FlowGraph {
    pub blocks: Vec<BasicBlock>,
    pub entry: BlockId,
    /// Per-stmt reachability (filled by the reachability pass). Indexed by `StmtId.0`.
    pub reachable: Vec<bool>,
}
```

**Lowering algorithm** (one pass over `Body::block`, mirroring `body.rs`'s `Lowerer` shape): maintain
a "current block" being filled; on a control-flow stmt, seal the current block with a terminator and
start fresh blocks for each branch + a merge block. `if`/`while`/`and`/`or`/ternary → `Branch`;
`match` → `Match`; `return` → `Return` (current block sealed, a fresh **unreachable-until-proven**
block follows). `break`/`continue` are gotos to the loop's break/continue targets (tracked on a small
loop-context stack, like a mini version of `Lowerer`). This is ~200-300 lines and is the bulk of the
new code.

### 3.2 The `Place` abstraction (replaces the `String` key)

```rust
/// A narrowable place: a local/param, or a (shallow) dotted access rooted at a local or `self`.
/// Deliberately shallow — we narrow `x`, `x.y`, `self.y` but NOT arbitrary call results
/// (`f().y`), array indices (`a[i].y`), or anything whose identity isn't stable under re-eval.
/// This is what keeps narrowing sound under mutation/aliasing (the 1.0 cut).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    /// A function local / param, by name (GDScript locals are function-scoped — infer.rs:489).
    Local(SmolStr),
    /// `self.member` (or a bare member resolving to `self`).
    SelfMember(SmolStr),
    /// A field access on another place (`x.y`, `self.y.z`). Boxed for the recursive case.
    Field(Box<Place>, SmolStr),
}

impl Place {
    /// Derive a `Place` from an expression, or `None` for a non-narrowable expression.
    /// The direct successor to today's `Cx::narrow_key` (infer.rs:1767) — but structured, not a String.
    pub fn of(body: &Body, id: ExprId) -> Option<Place> {
        match body.expr(id) {
            Expr::Name(n) => Some(Place::Local(n.clone())),
            Expr::SelfExpr => None, // `self` itself isn't narrowed; `self.m` is
            Expr::Paren(inner) => Place::of(body, *inner),
            Expr::Field { receiver, name, .. } => match body.expr(*receiver) {
                Expr::SelfExpr => Some(Place::SelfMember(name.clone())),
                _ => Some(Place::Field(Box::new(Place::of(body, *receiver)?), name.clone())),
            },
            _ => None,
        }
    }

    /// Whether assigning to `assigned` may invalidate a narrowing of `self`. Conservative:
    /// any prefix match invalidates (assigning `x` clears `x` and `x.*`; assigning `x.y` clears
    /// `x.y` and `x.y.*` but not `x`).
    pub fn invalidated_by(&self, assigned: &Place) -> bool { /* prefix check */ }
}
```

> **Why `SmolStr` not `EcoString`:** the workspace uses `smol_str::SmolStr` everywhere (`Expr::Name`,
> `LocalVar::name`, `Field::name` are all `SmolStr` — body.rs). The plan's `EcoString` is a synthesis
> artifact; using it would be a foreign dependency. `Place` is `Hash` + `Eq` so it keys an
> `FxHashMap<Place, NarrowedTy>` directly (no per-lookup `String` allocation like today's key).

### 3.3 `FlowFacts` and `NarrowedTy`

```rust
/// Narrowing facts that hold at a program point (a CFG edge). The then/else edges of a Branch
/// carry *different* fact sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowFacts(rustc_hash::FxHashMap<Place, NarrowedTy>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarrowedTy {
    /// The place is statically `T` — from `is T`, `x as T` assign, or a `match T():` arm.
    /// Carried as an `AstPtr` to the `TypeRef` (resolved lazily by the checker against `&dyn Db`
    /// + `EngineApi`, exactly like today's `apply_narrowing` resolves `ptr` — infer.rs:1755).
    Is(AstPtr),
    /// Proven non-null (from `!= null`, `if x:` on an object, or a prior `is`).
    NotNull,
    /// Proven NOT `T` (else-branch of `is T`). Best-effort: only usable to *re-widen*, never to
    /// assert a member; in 1.0 we record it but the checker treats it as "no positive info".
    Not(AstPtr),
}
```

**Join (merge points)** — the dataflow's correctness core: a place is narrowed at a merge **only if
narrowed compatibly on every incoming edge** (intersection). Differing `Is(T1)`/`Is(T2)` → drop
(widen to declared). `Is(T)` on one edge, nothing on another → drop. `NotNull` ∩ `NotNull` → `NotNull`.
This is the formal version of "the existing `join` (infer.rs:1795-1815) but over facts, not result
types" — and it is what makes the **else-branch and early-return idioms** correct.

### 3.4 How the checker consults facts (the `infer.rs` change)

The checker keeps its bottom-up walk but replaces the ad-hoc `narrowing` map with **the facts for the
current program point**, computed by the dataflow. Two integration options, pick **A** (lower risk):

- **Option A (recommended): keep the walk, feed it precomputed per-stmt facts.** The dataflow runs
  first and produces `entry_facts: FxHashMap<StmtId, FlowFacts>` (the facts holding *before* each
  stmt). `infer_block`/`infer_stmt` look up `entry_facts[stmt]` and install it as the active env
  before typing the stmt's exprs. `resolve_name`/`infer_field` consult the active facts via
  `Place::of` instead of `narrow_key` (infer.rs:1648-1653, 1316-1354). The `in_branch` save/restore
  (infer.rs:1818) is **deleted** — facts come from the graph, not from manual frames. This reuses
  ~90% of `infer.rs` unchanged.

- **Option B: drive typing from the CFG block order.** A bigger rewrite (worklist over blocks); defer
  to post-1.0. Not the 1.0 cut.

The fact→type resolution mirrors the existing widen-only rule **verbatim** (infer.rs:1756-1762):
resolve the `AstPtr` to a `Ty`; **bail if `is_uninformative()`**; apply only if the narrowed type is a
subtype of the place's current declared type *or* the current type is uninformative. **This is the
soundness gate — do not relax it.**

```rust
// in infer.rs, replacing the narrow_key/narrowing lookup at resolve_name (infer.rs:1648):
fn narrowed_ty(&self, id: ExprId) -> Option<Ty> {
    let place = Place::of(self.body, id)?;
    match self.facts.get(&place)? {
        NarrowedTy::Is(ptr) => {
            let narrowed = self.resolve_ptr_ty(*ptr);          // infer.rs:1779
            if narrowed.is_uninformative() { return None; }     // soundness guard (unchanged)
            let cur = self.expr_ty.get(&id).cloned().unwrap_or(Ty::Variant);
            (cur.is_uninformative() || self.is_subtype(&narrowed, &cur)).then_some(narrowed)
        }
        NarrowedTy::NotNull => None, // NotNull doesn't change the Ty, only suppresses null-access (post-MVP hook)
        NarrowedTy::Not(_)  => None, // best-effort; no positive type in 1.0
    }
}
```

### 3.5 The salsa query: `flow(body)` as a tracked query

The plan calls for `flow(body)` salsa-cached. **Caveat grounded in the code:** salsa tracked queries
key on salsa *inputs/ingredients* (`FileText`), not on a plain `Body` struct (which isn't a salsa
entity and isn't `Eq`-cheap to key on). Two faithful options:

- **Option 1 (recommended): cache at file granularity, compute flow inside the existing
  `analyze_file`.** `analyze_file` already lowers every body and runs `infer`. Add the flow pass
  inline (lower body → `FlowGraph` → dataflow → `entry_facts`) and feed it to `infer`. The cache unit
  stays `analyze_file(db, file)` — which **already backdates across signature-only edits** and is the
  firewall the tests guard. No new salsa entity, no risk to the durability model. The flow graph is a
  per-body local, recomputed only when the body's parse changes (i.e. when `analyze_file` re-runs).

- **Option 2 (if per-body caching is later needed for perf): introduce a salsa tracked struct per
  function.** This requires a `FunctionId` salsa entity (interned per `(file, item-index)`), which the
  codebase does **not** have today (bodies are lowered ad hoc from `AstPtr`s in `analyze_file`). That's
  a Phase-6 Workstream-4 (perf) concern; **do not build it for W2** unless the `bench` job shows flow
  recompute is hot. The pure `fn flow(&Body) -> FlowGraph` stays the cache body either way.

**Decision for 1.0: Option 1.** It satisfies "`flow` is computed on demand and cached" (it's inside the
cached `analyze_file`), keeps the firewall green, and adds zero new salsa surface. Revisit Option 2
only under a measured perf need.

### 3.6 `UNREACHABLE_CODE` / `UNREACHABLE_PATTERN` fall out of the CFG (feeds W1)

The reachability pass is a trivial graph reachability from `entry` over the terminator successors:

- **`UNREACHABLE_CODE`**: any block reachable in *lowering order* but with **no CFG predecessor** after
  a `Return`/`Break`/`Continue` (or an exhaustive `match`) — its stmts are dead. Emit at the **first**
  unreachable stmt's range. **Blocker: there is no `StmtId → TextRange` map today.** Add
  `stmt_ranges: Vec<TextRange>` to `BodySourceMap` (body.rs:418-421) and populate it in `Lowerer::
  alloc_stmt` (body.rs:554-558) — a 4-line change, additive. The message must match Godot verbatim:
  `"Unreachable code (statement after return, break, continue, or exhaustive match)."` — verify exact
  wording against `gdscript_warning.cpp` during W1 (cite [`research/09`](research/09-type-system-and-inference.md)
  §1.7 for the verbatim-message discipline).
- **`UNREACHABLE_PATTERN`**: in a `Terminator::Match`, any arm after one with `is_wildcard: true` is
  unreachable. Emit at the arm's range.

W2 **produces the reachability data** (`FlowGraph::reachable` + the `Match` wildcard flags); **W1 owns
the actual `RawWarning` emission + gating** (these are two of the 48 codes). The contract between the
workstreams: W2 exposes a `pub fn unreachable_stmts(graph, body) -> Vec<TextRange>` and
`unreachable_patterns(...)`; W1's checker calls them. Keep the **emission** in W1 so the gating layer
(`@warning_ignore`, project-setting levels) owns it uniformly.

---

## 4. Step-by-step implementation plan

Each step ends green through the crate tests + `cargo xtask ci` (the workspace gate). Order minimizes
risk: build + test the pure CFG **before** touching the checker.

**M0 — the pure CFG + facts (no checker change yet).**
1. Add `crates/gdscript-hir/src/flow.rs`; declare `mod flow;` in `lib.rs`. Implement `BlockId`,
   `BasicBlock`, `Terminator`, `MatchEdge`, `FlowGraph`, `Place`, `FlowFacts`, `NarrowedTy` (§3.1-3.3).
2. Implement `fn lower(body: &Body) -> FlowGraph` — the single-pass CFG builder (if/while/for/match/
   and-or/ternary/return/break/continue). Pure, no db.
3. Implement the **reachability pass** (`FlowGraph::reachable`) and the forward **dataflow** producing
   `entry_facts: FxHashMap<StmtId, FlowFacts>`, with the **join = intersection** at merges.
4. Add `stmt_ranges` to `BodySourceMap` (body.rs) + populate in `alloc_stmt`. *Exit:* unit tests on the
   graph shape + facts for each construct (golden CFG dumps), `xtask ci` green, **no behavior change**
   in `infer` yet (the graph is built but unused).

**M1 — wire the checker to the facts (Option A, §3.4).**
5. Thread `entry_facts` into `infer(...)` (extend the `Cx` struct: replace `narrowing: FxHashMap<String,
   Ty>` with `facts: &FlowFacts` set per-stmt; delete `in_branch`/`apply_narrowing`/`narrow_key`).
6. Replace the `resolve_name` narrowing lookup (infer.rs:1648-1653) and add the `narrowed_ty` consult
   in `infer_field` (infer.rs:1316). Keep the **`is_uninformative` + widen-only** gate verbatim.
7. Run the existing narrowing tests (`is_narrowing_suppresses_unsafe`,
   `is_narrowing_flags_real_missing_member`, `is_narrows_to_a_user_class_cross_file`,
   `as_casts_to_a_user_class_cross_file`) — they **must stay green** (parity bar). *Exit:* `if x is T:`
   parity with today, now via the CFG.

**M2 — the new constructs (where we beat the engine).**
8. else-branch `Not(T)` (best-effort), `!= null` / `if x:` → `NotNull`, **early-return** flow-past-guard
   (`if x == null: return` → `x: NotNull` after).
9. `and`/`or` short-circuit: the RHS of `a and b` is typed under `a`'s then-facts; `a or b`'s RHS under
   `a`'s else-facts.
10. `match` arm narrowing: a `T():` arm narrows the scrutinee `Place` to `T` in its body.
    *Exit:* every row of the §1 table holds; the #93510 differential cases pass.

**M3 — reachability handoff to W1.**
11. Expose `unreachable_stmts` / `unreachable_patterns` (§3.6). Coordinate with W1 to wire the emission +
    gating. *Exit:* `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` fire under W1's gating; the soundness
    property test (below) is green.

**M4 — the adversarial bug-hunt** (every prior milestone in this repo ends with one — see the Phase-5
playbooks): find→verify→fix, focused on *unsound* narrowing (a wrong narrow hiding a real `UNSAFE_*`).

---

## 5. Test plan

Mirror the existing in-file test style (`infer_first_func` / `file_codes` harnesses, infer.rs:1874-1907)
plus a fixture corpus and a differential harness.

1. **CFG-shape unit tests** (`flow.rs` `#[cfg(test)]`): golden assertions on block count + terminators
   for each construct (if/elif/else, while, for, match w/ default, nested, early return,
   break/continue). Assert `reachable` flags on dead-code-after-return.
2. **Narrowing golden fixtures** — one per §1 row, as `fixtures/narrowing/<case>.gd` + `.expected`
   (matching the warning-corpus convention in [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Testing):
   - `is`-guard suppresses `UNSAFE_*` in then (the headline #93510 win) — extend
     `is_narrowing_suppresses_unsafe` (infer.rs:1970).
   - early-return narrows past the guard; `and`/`or` short-circuit; `match` arms.
   - **reassignment invalidates** (`x = other` then `x.m()` re-checks against `other`) — extend the
     existing `infer_assign` re-narrow behavior (infer.rs:1106).
   - **opaque call invalidates `self`-member narrowing** — assert we *do not* wrongly suppress after a
     mutating call (soundness, not precision).
3. **The #93510 differential corpus.** Run the same typed fixtures through the real engine
   (`godot --check-only` / editor export) per supported minor; diff. The `UNSAFE_PROPERTY_ACCESS` /
   `UNSAFE_METHOD_ACCESS` cases we suppress via narrowing are **documented expected divergences**, not
   failures (per [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Testing #1 + §1.5).
4. **Soundness property test** (the load-bearing one, [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md)
   §Testing #3): over a generated/fuzzed body corpus, assert **narrowing never produces a member access
   the un-narrowed type couldn't justify** — i.e. every member we resolve via a narrowed type is a real
   member of that type. Equivalently: turning narrowing *off* may add `UNSAFE_*` warnings but must never
   *remove* a true one. (A wrong narrow = a hidden real warning; this catches it.)
5. **`UNREACHABLE_*` fixtures** (coordinated with W1): code after `return`/`break`/exhaustive `match`;
   `match` arm after `_`. Assert range = first dead stmt / the shadowed arm.
6. **The firewall regression test must stay green:** `body_edit_does_not_invalidate_signature_queries`
   (queries.rs:471) — the flow pass lives inside `analyze_file`, which is *already* downstream of body
   edits, so signature-only queries (`item_tree`, `file_class_name`, `script_class`) must remain
   untouched. Add a query-recount test asserting a body edit re-runs `analyze_file` (flow included) but
   **not** `global_registry`/`script_class`.
7. **No-regression sweep:** the full `infer.rs` + `queries.rs` test modules must pass unchanged (the
   existing `is`/`as` cross-file tests at infer.rs:1239-1271 are the parity bar).

---

## 6. Risks & mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **Unsound narrowing** (wrong narrow → hidden real `UNSAFE_*`, or asserting an absent member). | **Critical** | Conservative-by-construction: the **`is_uninformative` + widen-only** gate (infer.rs:1756) is preserved verbatim and applied at every fact→type step; join = intersection (drop on disagreement); the **soundness property test** (§5.4) is a CI gate. When unsure → widen. |
| **CFG complexity** (break/continue targets, nested loops, ternary-in-condition). | High | Build + golden-test the **pure** CFG (M0) before any checker change; loop-context stack like `Lowerer`; defer back-edge fixpoints (loop bodies entered with widened facts) — explicitly out of the 1.0 cut (§1). |
| **Firewall / durability regression** (flow recompute invalidating cross-file data). | High | Cache flow **inside `analyze_file`** (§3.5 Option 1) — no new salsa entity, no durability change; the existing firewall test guards it; add a query-recount test (§5.6). **Do not** add a `FunctionId` salsa entity for W2. |
| **`Place` shallowness misses real idioms** (`get_thing() is T`, `arr[i] is T`). | Med | Deliberate: these are **out of the 1.0 cut** (§1, the call-result/aliasing tail). Document; the `as`-cast path already covers `(get_thing() as T).m()`. |
| **Message/behavior drift vs engine for `UNREACHABLE_*`.** | Med | W2 only produces reachability data; **W1 owns emission + verbatim message + gating** (§3.6). Verify wording against `gdscript_warning.cpp` in W1; key on the symbolic code, not the text ([`research/09`](research/09-type-system-and-inference.md) §1.7). |
| **`EcoString` drift from the plan.** | Low | Use `SmolStr` throughout (the workspace convention) — already corrected in §3.2. |
| **Perf of the extra pass.** | Low | Flow is O(body size), once per `analyze_file` recompute; only runs when a body's parse changes. Add to the W4 `bench` fixture; gate on >10% regression. |

---

## 7. Dependencies on other workstreams

- **Workstream 1 (full warning set)** — **bidirectional, the tightest coupling.** W2 *produces* the
  reachability + match-wildcard data; **W1 emits** `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` and owns
  their gating/`@warning_ignore`/level resolution (§3.6). W2's whole *point* for `UNSAFE_PROPERTY_ACCESS`
  / `UNSAFE_METHOD_ACCESS` is that correct narrowing **suppresses** them (the #93510 win) — those two
  codes are in W1's opt-in "type-strictness" group (default IGNORE; the analyzer/CLI may default them on
  — [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §1.2/§1.5). **The differential corpus's two
  documented divergences are exactly these.** Sequence W2's M1-M2 before W1 finalizes its `UNSAFE_*`
  fixtures so the narrowing-suppression cases are encoded as expected.
- **Workstream 4 (perf)** — owns the decision on whether per-body flow caching (§3.5 Option 2) is ever
  needed; W2 ships Option 1 and hands W4 the `bench`-fixture coverage. The firewall recount test is
  shared.
- **Workstream 5 (docs)** — the narrowing pages + the live playground demo of the #93510 suppression
  (the visible win) consume W2's behavior; the warning-reference pages for `UNSAFE_*` document the
  divergence W2 creates.
- **No dependency on Workstreams 3/6/7.** The formatter, API-freeze, and governance are orthogonal —
  W2 emits the same `Diagnostic` POD shape (`crates/gdscript-base/src/lib.rs:72-89`), so the frozen
  `gdscript-ide` surface is unchanged (new warnings are additive under `#[non_exhaustive]`).

---

## 8. Provenance / grounding

Verified against the actual tree at `C:\Yanivs\GameDev\gdscript-analyzer` (branch state at authoring):
`crates/gdscript-hir/src/infer.rs` (the lexical narrowing: `Cx.narrowing` :492, `apply_narrowing`
:1743, `narrow_key` :1767, `in_branch` :1818, `is_subtype` :1412, the widen-only + `is_uninformative`
gate :1756); `crates/gdscript-hir/src/body.rs` (Body/Stmt/Expr/`BodySourceMap`, no stmt-range map);
`crates/gdscript-hir/src/queries.rs` (`analyze_file` tracked query :36, the firewall test :471, the
`script_class`/`scene_context` offset-free idioms); `crates/gdscript-db/src/lib.rs` (the `FileText`
salsa input + durability model); `crates/gdscript-base/src/lib.rs` (the `Diagnostic` POD :72); the
consumption path `crates/gdscript-ide/src/semantic.rs:47` → `lib.rs:188`. Plan source:
[`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 2; research:
[`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) §3.2 / §1.6 /
§1.7 / §7. **Confirmed missing (genuine gaps):** no CFG, no `Place`, no `NotNull`/`Not`/join,
no `UNREACHABLE_*` emitter, no `StmtId→range` map. **Confirmed already-present (extend, don't
greenfield):** `is`-narrowing, reassignment invalidation, `is_subtype`, the soundness guard.
