# PHASE 6 — v1.0 Release (Tier 3 full)

> **Status:** plan. **Tier:** 3 (full). **Completes:** v1.0 — the destination.
> **Canonical parents this doc obeys:** [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) (§6 the v1.0 success bar; §5 scope), [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) (§1 crate stack, §2 the `gdscript-ide` public API = the semver contract, §7 portability), [`ROADMAP.md`](ROADMAP.md) (Phase 6 = v1.0, Tier 3 full; deliverable + exit criteria).
> **Primary evidence:** [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) (the full 48-warning set + `debug/gdscript/warnings/*` gating; the complete LSP feature list), [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) (full flow narrowing; beating the engine checker on `is`/`as` guards per #93510; the Tier-3 tail), [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) (the 1.0 semver commitment, docs completeness, governance maturity).

This is the last planned phase. By its start everything *functional* exists — parse, single-file inference, project-wide resolution, scene-typed node paths, the LSP/CLI/playground, published 0.x packages. Phase 6 does **not** add a new pillar of capability; it **finishes** four things to a 1.0 standard (the complete warning set, real flow narrowing, a formatter, performance) and then makes an **irreversible promise** (a documented, semver-stable, supported `gdscript-ide` public API). The honest framing from [`ROADMAP`](ROADMAP.md): full Tier-3 narrowing is *"multi-year polish."* Phase 6's job is to **draw the 1.0 cut line crisply** — what is complete and contractual at 1.0, and what is explicitly a continual post-1.0 effort.

---

## Goal & scope (v1.0 = the destination; Tier 3 full; what "1.0" commits us to)

**v1.0 is a quality + stability milestone, not a feature milestone.** The features are largely in place after Phases 2–5; v1.0 means they are *complete enough to depend on* and *frozen enough to build on*.

### What ships (the seven workstreams)

1. **The full Godot warning set — all 48** ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.2) with engine-matching message strings, codes, and default levels (IGNORE/WARN/ERROR), gated by `debug/gdscript/warnings/*` from `project.godot`, and honoring `@warning_ignore` / `@warning_ignore_start` / `@warning_ignore_restore`. (Phase 2 shipped a *curated subset* on hard-wired defaults; Phase 6 completes the set and wires real gating.)
2. **Full control-flow narrowing** — a real CFG over function bodies; narrowing through `if`/`match`/`is`/`as`/`!= null`/`and`-`or` short-circuit, suppressing `UNSAFE_*` inside proven-safe branches — **beating** the engine checker, which fails to suppress these inside `is`/`as` guards (Godot [#93510](https://github.com/godotengine/godot/issues/93510), proposal [#8530](https://github.com/godotengine/godot-proposals/issues/8530)). (Phase 2 did *local, syntactic* narrowing only.)
3. **A formatter** — `gdformat`-compatible (or a documented superset), operating on the lossless CST we already produce, idempotent, golden-corpus-tested, shared by the CLI (Phase 5) and the LSP `formatting` capability.
4. **Performance hardening** — large-project benchmarks against a real Godot game fixture, memory profiling, salsa cache tuning, parse/infer throughput targets, the wasm bundle-size budget, regression guards in CI.
5. **Documentation completeness** — the mdBook user guide finished (install, consume-from-Rust/Node/browser, configuration, the warning reference, the "add a client" guide), polished docs.rs API docs, the playground as live docs, examples, the contract page.
6. **API stabilization & the 1.0 commitment** — freeze + document `gdscript-ide`'s public surface; declare what is `#[non_exhaustive]`, the deprecation policy, the supported-Godot-version matrix, the semver guarantees for crates.io + npm consumers; an API-review pass.
7. **Ecosystem maturity** — graduate governance toward an RFC process *if contributor volume warrants*; issue triage; a public roadmap; first external-consumer outreach (the ≥1-external-consumer criterion).

### What "1.0" commits us to (the contract)

Crossing 0.x → 1.0 flips the SemVer reading ([`research/07`](research/07-ecosystem-and-release-tooling.md) §3.3): in 0.x, `0.Y.0` was our "breaking" lever and consumers expected churn; at 1.0 we adopt **standard SemVer 2.0.0** — `feat`→minor, `fix`→patch, **breaking→major**. Concretely:

- **`gdscript-ide`'s public surface is the contract** ([`01`](01-ARCHITECTURE.md) §2: *"this is the crate we semver most carefully; it is the contract every consumer builds on"*). A breaking change to it now forces a **2.0** — so we freeze it deliberately and only after an API-review pass (Workstream 6).
- **The napi + wasm POD result shape is part of the contract.** npm consumers (`@gdscript-analyzer/core`, `@gdscript-analyzer/wasm`) depend on the JSON shape exactly as Rust consumers depend on the structs. One shared version across both registries ([`research/07`](research/07-ecosystem-and-release-tooling.md) §3.3) means the contract is unified.
- **Engine-matching diagnostics are a documented promise, not an accident.** The warning **codes** (e.g. `GDSCRIPT_UNSAFE_CALL`) are stable identifiers consumers key on; the **messages** track Godot (and may drift *with* Godot — see Risks).
- **What is explicitly NOT frozen:** internal crates (`gdscript-hir`, `gdscript-db`, `gdscript-syntax`, …) remain free to change — only `gdscript-ide` (+ the FFI POD) is the contract. Inference *precision* may improve (a `Variant` becoming a concrete type) without a major bump — that is a *quality* change, not an API break (see Risks: "is sharpening inference a breaking change?").

### Explicit non-goals at 1.0 (the Tier-3 tail, deferred — see Post-1.0)

| Deferred | Why it is not in 1.0 |
|---|---|
| **Perfect** flow narrowing across every aliasing/mutation/loop-carried case | The narrowing tail is multi-year ([`research/09`](research/09-type-system-and-inference.md) §7 Tier 3). 1.0 ships a **well-defined, tested subset** (below) that already beats the engine; the rest is continual. |
| GDScript 3.x / Godot 3 | Out of scope project-wide ([`00`](00-VISION-AND-SCOPE.md) §5 non-goals). |
| Deeper refactorings (extract method, inline, change-signature), call hierarchy | Beyond the 1.0 feature bar; post-1.0 ([`research/04`](research/04-gdscript-semantics-and-features.md) §4 marks these "Later"/Phase 3+). |
| More language bindings (PyO3/PyPI, C ABI) | "cheap optionality, post-v1" ([`01`](01-ARCHITECTURE.md) §4). |
| Parallelism for throughput | Feature-gated, native-only, post-1.0 ([`01`](01-ARCHITECTURE.md) §7 rule 3). |

---

## Prerequisites (Phases 0–5 complete)

Phase 6 composes everything below; it adds **no new architectural layer**.

**From Phase 0 — ecosystem & tooling.** The workspace, CI (fmt/clippy/test-matrix/MSRV/wasm-check/coverage/deny), the release toolchain (release-plz + Changesets + version-sync), the mdBook + docs.rs scaffold, governance files, dual licensing, `xtask`, and the **Godot-sync** Action producing the `gdscript-api` data blob ([`GODOT-SYNC.md`](GODOT-SYNC.md)). Phase 6 *fills* the docs scaffold and *graduates* governance.

**From Phase 1 — parser & syntax (Tier 0).** The **lossless `cstree` CST** + typed AST with error recovery. **The formatter (Workstream 3) depends on this directly** — it pretty-prints the CST, which retains every token + trivia.

**From Phase 2 — single-file semantics (Tier 1).** The HIR (`ItemTree`/`Body`), binder (symbols + scope chain), the forward gradual type checker, `is`/`as`/`!= null` **syntactic** narrowing, and the **curated warning subset** on hard-wired defaults. Phase 6 extends the checker to the **full 48** and replaces syntactic narrowing with a **real CFG** (Workstreams 1–2). Critically, Phase 2 already routed every warning through a stable `code` and noted that *"the checker always emits; a later filter layer gates"* — Phase 6 builds that gate.

**From Phase 3 — project-wide & incremental (Tier 2).** The project model (`project.godot` parse → autoloads + Godot-version detection), the `class_name` registry, `preload`/`load`/`extends` resolution, cross-file goto/refs/rename/workspace-symbols, and **salsa** adopted with durability. Phase 6's **gating reads `[debug] gdscript/warnings/*` from the already-parsed `project.godot`**; perf hardening (Workstream 4) tunes the salsa graph this phase introduced.

**From Phase 4 — scene awareness (Tier 3 slice).** `.tscn` parsing → typed `$Path`/`%Unique`/`get_node(...)`. Phase 6 adds the warnings that *depend on* scene typing (e.g. `GET_NODE_DEFAULT_WITHOUT_ONREADY` reasoning) and includes scene-heavy projects in the perf fixture.

**From Phase 5 — clients & distribution.** The standalone `gdscript-lsp`, the guitkx swap (proxy deleted), `gdscript-cli`, the WASM playground, and **0.x GA on crates.io + npm**. Phase 6 turns 0.x into **1.0**: the CLI `format` and LSP `formatting` consume Workstream 3; the playground becomes *live docs* (Workstream 5); the published packages get the 1.0 contract (Workstream 6).

**Sanity gate before starting:** the Phase-5 tree is green on `cargo xtask ci`, the wasm portability check (`cargo check -p gdscript-ide --target wasm32-unknown-unknown`) passes, and a 0.x release dry-run succeeds.

---

## Workstream 1 — The full Godot warning set (all 48)

Phase 2 shipped ~7 warnings on hard-wired "on" defaults. Phase 6 ships **all 48** ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.2, from `gdscript_warning.h` `master`) with **engine-matching messages + codes + default levels**, real **project-setting gating**, and **annotation suppression**. Three sub-pieces: the checks, the gating layer, the suppression layer.

### 1.1 Architecture — emit-then-gate (the Phase-2 seam, now realized)

```
checker (gdscript-hir)                  gating (gdscript-hir/ide)             client
  produces RawWarning { code,    ──▶   apply: enable master switch,    ──▶   Diagnostic (POD)
    range, args }  (ALWAYS, on             per-code level override,            severity already
    every applicable site)                 treat_as_errors, directory_rules,   resolved; code stable
                                           @warning_ignore regions
```

- **The checker always emits** a `RawWarning` at every applicable site, keyed on the **symbolic code** (never Godot's shifting integer — [`research/04`](research/04-gdscript-semantics-and-features.md) §2.2 note). This is the Phase-2 invariant (*"the checker always emits; a later filter layer gates"*) finally exercised across all 48.
- **A pure gating function** `gate(raw, settings, ignores) -> Option<Diagnostic>` resolves the final severity (or drops it). It is the **only** place project settings and `@warning_ignore` touch warnings — keeping the checker oblivious to configuration, which keeps it incrementally cacheable (gating is cheap and re-run per snapshot; the expensive `infer`/`item_tree` queries don't depend on warning settings, so editing a `project.godot` warning level **never** invalidates inference).

```rust
// crates/gdscript-hir/src/warnings.rs   (sketch — illustrative)

/// Stable symbolic identity. Serialized as e.g. "GDSCRIPT_UNSAFE_CALL"; the lowercased
/// setting-name form ("unsafe_call_argument") is derived for project.godot lookups.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]                         // adding warnings post-1.0 is non-breaking
pub enum WarningCode {
    UnassignedVariable, UnusedVariable, ShadowedVariable, UnreachableCode,
    UnsafePropertyAccess, UnsafeMethodAccess, UnsafeCast, UnsafeCallArgument,
    InferenceOnVariant, NativeMethodOverride, GetNodeDefaultWithoutOnready,
    OnreadyWithExport, /* …all 48… */
}

pub enum WarnLevel { Ignore, Warn, Error }    // mirrors Godot's Ignore(0)/Warn(1)/Error(2)

impl WarningCode {
    pub fn default_level(self) -> WarnLevel;   // from default_warning_levels[] — table below
    pub fn setting_name(self) -> &'static str; // "unsafe_method_access" (project.godot key tail)
    pub fn message(self, args: &WarnArgs) -> String; // verbatim engine string, args filled
    pub fn since(self) -> ApiVersion;          // e.g. MISSING_AWAIT is master-only; gate by version
}
```

### 1.2 The complete warning table (category → code → default level)

Grouped per [`research/04`](research/04-gdscript-semantics-and-features.md) §2.2. **Default level** is from Godot's `default_warning_levels[]`. Version notes flag codes absent on 4.3-stable or compiled out under `DISABLE_DEPRECATED`.

| Category | Code (symbolic) | Default | Notes |
|---|---|---|---|
| **Unassigned / unused** | `UNASSIGNED_VARIABLE` | WARN | |
| | `UNASSIGNED_VARIABLE_OP_ASSIGN` | WARN | |
| | `UNUSED_VARIABLE` | WARN | |
| | `UNUSED_LOCAL_CONSTANT` | WARN | |
| | `UNUSED_PRIVATE_CLASS_VARIABLE` | WARN | |
| | `UNUSED_PARAMETER` | WARN | |
| | `UNUSED_SIGNAL` | WARN | |
| **Shadowing** | `SHADOWED_VARIABLE` | WARN | |
| | `SHADOWED_VARIABLE_BASE_CLASS` | WARN | |
| | `SHADOWED_GLOBAL_IDENTIFIER` | WARN | |
| **Control-flow** | `UNREACHABLE_CODE` | WARN | needs the CFG (Workstream 2) |
| | `UNREACHABLE_PATTERN` | WARN | `match` after wildcard/bind |
| | `STANDALONE_EXPRESSION` | WARN | |
| | `STANDALONE_TERNARY` | WARN | |
| | `INCOMPATIBLE_TERNARY` | WARN | mismatched arm types |
| **Type-safety (returns/calls)** | `UNSAFE_VOID_RETURN` | WARN | |
| | `STATIC_CALLED_ON_INSTANCE` | WARN | |
| **Tool/static/await** | `MISSING_TOOL` | WARN | base `@tool` without local `@tool` |
| | `REDUNDANT_STATIC_UNLOAD` | WARN | |
| | `REDUNDANT_AWAIT` | WARN | |
| **Assertions** | `ASSERT_ALWAYS_TRUE` | WARN | |
| | `ASSERT_ALWAYS_FALSE` | WARN | |
| **Numeric/enum** | `INTEGER_DIVISION` | WARN | |
| | `NARROWING_CONVERSION` | WARN | float→int |
| | `INT_AS_ENUM_WITHOUT_CAST` | WARN | |
| | `INT_AS_ENUM_WITHOUT_MATCH` | WARN | |
| | `ENUM_VARIABLE_WITHOUT_DEFAULT` | WARN | |
| **File/keyword** | `EMPTY_FILE` | WARN | |
| | `DEPRECATED_KEYWORD` | WARN | e.g. `yield` |
| **Confusables** | `CONFUSABLE_IDENTIFIER` | WARN | |
| | `CONFUSABLE_LOCAL_DECLARATION` | WARN | |
| | `CONFUSABLE_LOCAL_USAGE` | WARN | |
| | `CONFUSABLE_CAPTURE_REASSIGNMENT` | WARN | |
| | `CONFUSABLE_TEMPORARY_MODIFICATION` | WARN | **master only** (not 4.3) |
| **Deprecated misuse** | `PROPERTY_USED_AS_FUNCTION` | WARN | compiled out under `DISABLE_DEPRECATED` |
| | `CONSTANT_USED_AS_FUNCTION` | WARN | deprecated |
| | `FUNCTION_USED_AS_PROPERTY` | WARN | deprecated |
| **Type-strictness (opt-in)** | `UNTYPED_DECLARATION` | **IGNORE** | the "enforce static typing" group |
| | `INFERRED_DECLARATION` | **IGNORE** | |
| | `UNSAFE_PROPERTY_ACCESS` | **IGNORE** | suppressed by correct narrowing (W2) |
| | `UNSAFE_METHOD_ACCESS` | **IGNORE** | suppressed by correct narrowing (W2) |
| | `UNSAFE_CAST` | **IGNORE** | |
| | `UNSAFE_CALL_ARGUMENT` | **IGNORE** | |
| | `RETURN_VALUE_DISCARDED` | **IGNORE** | |
| | `MISSING_AWAIT` | **IGNORE** | **master only** (not 4.3) |
| **Hard-fail (opt-out)** | `INFERENCE_ON_VARIANT` | **ERROR** | |
| | `NATIVE_METHOD_OVERRIDE` | **ERROR** | |
| | `GET_NODE_DEFAULT_WITHOUT_ONREADY` | **ERROR** | needs scene typing (Phase 4) |
| | `ONREADY_WITH_EXPORT` | **ERROR** | |

**Not modeled as warnings** ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.2): `ABSTRACT_CLASS_INSTANTIATED` (a hard error, emitted as a parse/semantic error, not a gateable warning) and `RENAMED_IN_GODOT_4_HINT` (≤4.3 only; we do not model it). **Counts to 48 on master / 47 on 4.3** — the master-only codes (`CONFUSABLE_TEMPORARY_MODIFICATION`, `MISSING_AWAIT`) are gated by `WarningCode::since()` against the active engine version (Phase 3's version detection), so a 4.3 project never sees a master-only warning and vice-versa.

### 1.3 Project-setting gating (`debug/gdscript/warnings/*`)

Reading from the already-parsed `project.godot` ([`research/04`](research/04-gdscript-semantics-and-features.md) §2.3), in precedence order inside `gate(...)`:

1. **Master switch** `debug/gdscript/warnings/enable` (bool, default `true`) — off ⇒ no warnings at all.
2. **Per-code override** `debug/gdscript/warnings/<lowercase_name>` (enum `Ignore(0)/Warn(1)/Error(2)`) — overrides the default level for that code.
3. **`treat_warnings_as_errors`** (bool) — escalate every surviving WARN → ERROR.
4. **Scope rules** — `exclude_addons` (4.3/4.4, default `true`; suppress `res://addons/...`) *or* `directory_rules` (master; per-dir Include/Exclude, `res://addons` excluded by default). Pick by active engine version; the file's path decides inclusion.

```rust
pub struct WarningSettings {
    pub enabled: bool,
    pub treat_as_errors: bool,
    pub per_code: FxHashMap<WarningCode, WarnLevel>,   // explicit project.godot overrides
    pub scope: WarnScope,                              // ExcludeAddons | DirectoryRules(rules)
}
// gate(): if !enabled -> None; level = per_code.get(code).unwrap_or(code.default_level());
//         if level == Ignore -> None; if treat_as_errors && level == Warn -> Error;
//         if !scope.includes(file_path) -> None; else Some(Diagnostic{ code, severity, msg, range })
```

**Analyzer-default deviation (documented):** like Phase 2, the *standalone* analyzer/CLI may default the type-strictness group **on** (its whole value proposition is exactly those `UNSAFE_*`/`UNTYPED_*` diagnostics — [`research/04`](research/04-gdscript-semantics-and-features.md), the "enforce static typing" workflow). But when a `project.godot` is present, **its settings win** — so an editor session matches what Godot itself would report. The CLI exposes `--engine-defaults` (use Godot's `default_warning_levels[]` verbatim) vs `--strict` (turn the opt-in group on) so CI authors choose.

### 1.4 Annotation suppression (`@warning_ignore[_start|_restore]`)

Three annotations ([`research/04`](research/04-gdscript-semantics-and-features.md) §1.10 #34–36, §2.3), names = the lowercased setting names:

- `@warning_ignore("unsafe_method_access", …)` — suppress the listed codes on the **single following statement/declaration**.
- `@warning_ignore_start("name", …)` / `@warning_ignore_restore("name", …)` — suppress over a **region** (start until matching restore, or EOF).

Implementation: a lexer/parser pass already retains annotations; a **suppression map** `Vec<(TextRange, SmallSet<WarningCode>)>` is built per file (regions for start/restore, single-stmt spans for the one-shot form) and consulted in `gate(...)` *after* level resolution but *before* emit. Unknown names in an ignore annotation → a meta-diagnostic (Godot warns on unknown ignore names; match that). Suppression spans are **CST-derived** (stable byte ranges), so they survive incremental edits cleanly.

### 1.5 Where we exceed the engine

Two codes — `UNSAFE_PROPERTY_ACCESS`, `UNSAFE_METHOD_ACCESS` — are the *direct beneficiaries* of Workstream 2. The engine emits them even inside a proven `if x is T:` guard ([#93510](https://github.com/godotengine/godot/issues/93510)); **our CFG narrows the receiver to `T` inside the guard, so the access is provably safe and we emit nothing.** This is a **deliberate, documented divergence**: on the same file with the same settings, we produce *fewer false* `UNSAFE_*` than Godot. The warning-reference docs (Workstream 5) call this out per code, and the differential test corpus (Testing) treats these specific divergences as **expected**, not failures.

---

## Workstream 2 — Full control-flow narrowing

Phase 2 narrowed *syntactically* — it knew the guarded sub-tree of an `is`/`as`/`!= null` test but had no notion of flow across statements, `else` branches, early returns, or `match`. Phase 6 builds a **real CFG** over each function body and threads **narrowing facts** along edges. This is the single place worth real CFG work ([`research/09`](research/09-type-system-and-inference.md) §3.2), and the place we **beat the engine** ([#93510](https://github.com/godotengine/godot/issues/93510)).

### 2.1 The flow-analysis design

Borrowed from TypeScript's binder/checker split ([`research/09`](research/09-type-system-and-inference.md) §3.2): the **binder** builds a per-body **control-flow graph** of basic blocks; the **checker** computes, for each reachable point, a **narrowing environment** mapping a *narrowable place* to a refined `Ty`.

```rust
// crates/gdscript-hir/src/flow.rs   (sketch — illustrative)

/// One basic block: straight-line stmts + the condition that fans out its successors.
pub struct BasicBlock { stmts: Vec<StmtId>, term: Terminator }
pub enum Terminator {
    Goto(BlockId),
    Branch { cond: ExprId, then_bb: BlockId, else_bb: BlockId }, // if / while / and-or / ternary
    Match  { scrutinee: ExprId, arms: Vec<(PatternId, BlockId)>, default: Option<BlockId> },
    Return, Unreachable,
}

/// A place we can narrow: a local, or a (dotted) access chain rooted at a local/self.
/// Kept deliberately shallow — narrowing `x` and `x.y` (when y is a known field) but NOT
/// arbitrary call results, to stay sound under mutation/aliasing.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Place { Local(LocalId), Field(Box<Place>, EcoString), SelfMember(EcoString) }

/// Narrowing facts that hold on a specific CFG edge (true-branch vs false-branch differ).
#[derive(Clone)]
pub struct FlowFacts(FxHashMap<Place, NarrowedTy>);
pub enum NarrowedTy {
    Is(Ty),          // place is statically T   (from `is T`, `as T` assign, scene typing)
    NotNull,         // place proven non-null   (from `!= null`, prior `is`)
    Not(Ty),         // place is NOT T          (else-branch of `is T`)  — best-effort
}
```

**Algorithm (per body, on demand, salsa-cached as `flow(body)`):**
1. Lower the `Body` to a CFG (one pass over statements; `if`/`while`/`for`/`match`/`and`/`or`/ternary create branch terminators).
2. Forward dataflow: each edge carries a `FlowFacts`. A `Branch` on `cond` produces **two** environments — the `then` edge assumes `cond` true, the `else` edge assumes it false. Join points **intersect** facts (a place is narrowed only if narrowed on *all* incoming edges).
3. The checker, when typing an expression, consults the in-scope `FlowFacts` for the expression's `Place` and uses the **narrowed** `Ty` instead of the declared one.

### 2.2 The narrowing rules (what 1.0 covers)

| Source construct | Fact produced | Beats engine? |
|---|---|---|
| `if x is T:` | `then`: `x → Is(T)`; `else`: `x → Not(T)` | **yes** (#93510) — `UNSAFE_*` on `x.member` suppressed in `then` |
| `var t := x as T` | `t → Is(T)` (and the line is "safe") | parity (idiomatic) |
| `if x != null:` / `if x:` (object) | `then`: `x → NotNull` | yes — avoids spurious nullable access |
| `x == null: return` (early return) | after the `if`, `x → NotNull` (flow past the guard) | **yes** — the early-return idiom |
| `and` short-circuit: `x is T and x.foo` | RHS typed with `x → Is(T)` | **yes** |
| `or` short-circuit: `x == null or x.foo` | RHS typed with `x → NotNull` | **yes** |
| `match x: T(): … _: …` arm | arm body: `x → Is(arm pattern type)` (where the pattern is a type/binding) | **yes** |
| reassignment `x = other` | **invalidate** `x`'s narrowing (re-narrow from `other`'s type) | soundness |
| call that could mutate `x` (passed by ref / `self` method) | **conservatively invalidate** narrowing of `self`-members across opaque calls | soundness over precision |

It also **feeds `UNREACHABLE_CODE` and `UNREACHABLE_PATTERN`** (Workstream 1): a block with no predecessors after a `return`/`break`/exhaustive `match` is unreachable; a `match` arm after a wildcard/bind is unreachable. These two warnings *require* the CFG, which is why they land here.

### 2.3 The 1.0 cut vs the multi-year tail (be honest)

Full Tier-3 narrowing is *"multi-year polish"* ([`ROADMAP`](ROADMAP.md) Tier table; [`research/09`](research/09-type-system-and-inference.md) §7). The **1.0 cut** is the table above: **flow-sensitive narrowing of locals and shallow `self`/field places through branches, early returns, short-circuit, and `match`**, with **conservative invalidation** on reassignment and opaque calls. Deliberately **out of the 1.0 cut** (post-1.0, continual):

- Narrowing through **arbitrary call results** (`get_thing() as T` is fine; `if get_thing() is T:` is not narrowed — the place isn't stable).
- **Loop-carried** refinement fixpoints beyond a single conservative pass (we don't iterate to a fixpoint over back-edges; we widen to the declared type).
- **Aliasing** analysis (two locals pointing at the same object).
- **Discriminated-union**-style narrowing on enum/`int` tags.

**The 1.0 soundness promise:** narrowing is *conservative* — when unsure, we widen to the declared/`Variant` type, so we **never narrow wrongly** (a wrong narrowing would *hide* a real `UNSAFE_*` or assert a member that isn't there). Precision improves post-1.0; **soundness is fixed at 1.0.** This is the honest line: *we beat the engine on the common guard idioms today, and we keep closing the gap, but "perfect narrowing" is explicitly not a 1.0 deliverable.*

---

## Workstream 3 — The formatter

A formatter that operates on the **lossless CST we already have** (Phase 1 — every token + trivia retained), targeting **`gdformat` compatibility** (gdtoolkit's `gdformat` is the de-facto GDScript formatting standard) or a **documented superset**. Shared by the CLI (`gdscript-cli format`, Phase 5) and the LSP `formatting`/`rangeFormatting` capability (which the engine LSP does **not** provide — [`research/04`](research/04-gdscript-semantics-and-features.md) §5.1, `formatting: false`).

### 3.1 Design — CST → token stream → layout

- **Input is the CST, not the AST.** Formatting must preserve comments, blank-line intent, and string-quote choices, all of which live in CST trivia. We **never** round-trip through the lossy AST. A parse with errors → format the well-formed regions, leave the broken span untouched (error-tolerant, like the rest of the stack).
- **Pretty-printer over a `Doc` IR** (Wadler/Prettier-style group/line/indent algebra), not ad-hoc string concatenation — this is what makes line-width reflow and idempotence tractable. The CST drives `Doc` construction; the `Doc` renderer applies the width budget.
- **GDScript-specific rules:** significant indentation (the formatter emits INDENT/DEDENT correctly — tabs by default), `:` block headers, annotation placement (`@export` on its own line vs inline), `##` doc-comment preservation, `#region`/`#endregion` retention, trailing-comma handling in arrays/dicts/typed collections, operator spacing, and the one-line-body convention (`func f(): return x`).

### 3.2 Config

GDScript uses **tabs by default** (the engine's own convention); the formatter must default to tabs to match. Minimal, documented config (no bikeshed sprawl — see Risks):

| Option | Default | Notes |
|---|---|---|
| `line_width` | `100` | gdtoolkit's `gdformat` default is 100; match it |
| `indent` | `tabs` | GDScript default; `spaces`+`indent_size` allowed |
| `indent_size` | `4` | only when `indent = spaces` |
| `safe_mode` | `true` | refuse to reformat if a parse error would change semantics |

Config source: a `gdscript-analyzer.toml` (or `[tool.gdformat]`-compatible section) discovered up the tree; LSP `formatting` reads it from the workspace root. **Compatibility statement** lives in docs: where we match `gdformat` exactly, and every documented deviation (the "superset" parts).

### 3.3 Idempotence + the golden corpus

- **Idempotence is the core invariant:** `format(format(x)) == format(x)` for all inputs. A property test asserts it over the whole fixture corpus + fuzzer output.
- **Golden corpus:** `fixtures/format/*.gd` (input) + `*.expected` (formatted). Seeded from (a) the Godot demo-projects corpus, (b) a `gdformat` **parity set** — files run through real `gdformat` whose output we must match (deviations are explicit golden exceptions with a documented reason), and (c) adversarial cases (deep nesting, long calls, comment-heavy, mixed tabs/spaces).
- **Round-trip safety:** formatting **must not change semantics** — a test re-parses before/after and asserts AST equivalence (modulo trivia).

### 3.4 Cross-reference: guitkx already needs this

The guitkx project (ReactiveUI-for-Godot) already has a **GDScript-aware formatting need** — it formats embedded GDScript inside `.guitkx` markup `{expr}`/hook blocks. Because our formatter takes **CST + byte ranges** and returns a `SourceChange`, guitkx can **range-format just the embedded GDScript** (via the same Volar-style source-map adapter it uses for completion/hover, Phase 5) instead of shelling out to `gdformat` (a Python runtime dependency it would otherwise carry). This is a concrete **second consumer** of the formatter (relevant to the ≥1-external-consumer criterion, Workstream 7) and validates the range-formatting path.

---

## Workstream 4 — Performance hardening

1.0 means *fast on real projects, and provably non-regressing.* The targets are the keystroke-latency promise from [`ROADMAP`](ROADMAP.md) Phase 3 (*"editing one file does not re-type-check the whole project; keystroke latency flat as project grows"*), now measured and guarded.

### 4.1 The benchmark fixture — a real Godot game

A committed large-project fixture (a real open-source Godot 4.x game — e.g. a Maaack template-based project or a demo-projects superset; license-compatible, vendored under `fixtures/perf/`). It must exercise: many `.gd` files, a deep `class_name`/`extends` graph, autoloads, and **scene-heavy** node-path typing (Phase 4). Tiered sizes (small ~50 files, medium ~300, large ~1000+) so regressions show where they bite.

### 4.2 Throughput + latency targets (criterion + a harness)

| Metric | Target | Rationale |
|---|---|---|
| Cold full-project analyze (large fixture) | budget set from first measurement, then **regression-guarded** (±10%) | CI/CLI use case; one-shot `analyze(files)` |
| **Warm keystroke** (edit one body, re-query diagnostics) | **< 10 ms**, flat as project grows | the core incremental promise; salsa durability must hold |
| Edit a **signature**/`class_name` (bounded invalidation) | re-checks only dependents, **not** the world | the §Phase-3 invariant, now measured |
| Member completion (`recv.`) warm | **< 5 ms** | carried from Phase 2 |
| Parse throughput | MB/s baseline, regression-guarded | parser is on the hot path |

### 4.3 Memory + salsa tuning

- **Memory profiling** (`dhat`/heaptrack on native) on the large fixture: cap resident analysis state; ensure the shared `Arc<EngineApi>` is loaded once (not per file); interning (`ClassId`/`BuiltinId`/`EcoString`) keeps `Ty` small + `Copy`.
- **Salsa cache tuning:** verify **durability** separates volatile inputs (the edited file) from durable ones (the engine API, unchanged for a session) so editing a body invalidates only `infer(body)`/`flow(body)` ([`01`](01-ARCHITECTURE.md) §3; [`research/09`](research/09-type-system-and-inference.md) §3.4). Add **LRU eviction** for cold-file derived data on huge projects (rust-analyzer's approach) to bound memory. Confirm the **"body edit never invalidates project-global data"** invariant with a test that counts query re-executions.

### 4.4 The wasm bundle-size budget

The browser playground (Phase 5) is a 1.0 deliverable; bundle size is a UX gate.

| Artifact | v1 budget | Strategy |
|---|---|---|
| wasm code module (brotli) | target ≤ ~1.5 MB | `wasm-opt -Oz`, `opt-level="z"`, strip, no panic infra in release; choose the smaller of napi-wasm vs wasm-bindgen per measurement ([`01`](01-ARCHITECTURE.md) §4) |
| Engine-API data asset (brotli) | target ≤ ~few-hundred KB | prune → rkyv/postcard → brotli → **separate content-hashed asset**, fetched (not `include_bytes!`) ([`01`](01-ARCHITECTURE.md) §5) |

### 4.5 Regression guards in CI

- A **`bench` CI job** (criterion + `--save-baseline` against the committed baseline, or `cargo-codspeed`/Bencher) that **fails the PR on a >10% regression** of any tracked metric.
- A **wasm-size guard:** build the playground artifact, brotli-compress, **fail if over budget** (a hard byte ceiling in CI).
- The existing wasm32 **portability check** stays green (no `std::fs`/`Instant`/threads leaked — [`01`](01-ARCHITECTURE.md) §7).

---

## Workstream 5 — Documentation completeness

At 1.0 the docs scaffold from Phase 0 ([`research/07`](research/07-ecosystem-and-release-tooling.md) §5) is **finished**. Three surfaces: the mdBook guide, docs.rs API docs, and the playground-as-live-docs.

### 5.1 The mdBook user guide (`docs/`, deployed via Pages)

`SUMMARY.md` skeleton (extends [`research/07`](research/07-ecosystem-and-release-tooling.md) §5.5), all sections written:

- **User Guide** — Install (crates.io / `npm i @gdscript-analyzer/{core,wasm}` / CLI binary), **Consume from Rust** (`AnalysisHost`/`Analysis` walkthrough), **Consume from Node** (the napi `AnalysisHandle`), **Consume from the browser** (the wasm package + the data-asset fetch), **Configuration** (warning settings, `project.godot` mapping, formatter config).
- **Warning Reference** — one page **per warning**: code, default level, engine message, an example that triggers it, how to suppress (`@warning_ignore` + the `project.godot` key), and — for `UNSAFE_PROPERTY_ACCESS`/`UNSAFE_METHOD_ACCESS` — **the documented divergence** (we suppress these via narrowing where the engine doesn't, #93510). Generated from the `WarningCode` table (single source of truth) so it can't drift from the implementation.
- **Editor / LSP Client Integration** — Overview, VS Code, Neovim, **Godot external editor**, and **"Adding a new client"** modeled on rust-analyzer's *Other Editors* page ([`research/07`](research/07-ecosystem-and-release-tooling.md) §5.4): finding the server binary on `PATH`, transport (stdio/TCP), all settings via LSP `initializationOptions` (documented JSON schema + default example), advertised capabilities, and tracing for client authors. Cross-link to docs.rs for embedding the crate directly (the guitkx path).
- **Reference** — CLI (`check`/`lint`/`format`/`symbols`, flags incl. `--engine-defaults`/`--strict`), config-file schema, advertised LSP capabilities, the **contract page** (below).
- **Contributing** — Architecture, crate layout, build (the `xtask` flows).

CI: `mdbook test` (validates Rust samples) + `mdbook-linkcheck` on every PR ([`research/07`](research/07-ecosystem-and-release-tooling.md) §5.2).

### 5.2 docs.rs API docs polished

`gdscript-ide` is the documented contract: every public type/method has rustdoc, with **module-level docs** explaining `AnalysisHost`/`Analysis`/`Cancellable`/the POD result types and a **top-level usage example** (doctested). `[package.metadata.docs.rs]` `all-features = true` + `--cfg docsrs`, offline-buildable ([`research/07`](research/07-ecosystem-and-release-tooling.md) §5.1). Internal crates get lighter docs; the **public-vs-internal boundary is explicit in the docs** (Workstream 6).

### 5.3 The playground as live docs + examples

- The Phase-5 WASM playground (Monaco/CodeMirror, à la Ruff/Biome) becomes **part of the docs**: each warning-reference page links a **prefilled playground** showing the diagnostic live; the narrowing pages show the `is`-guard suppression live (the #93510 win, visible).
- **Examples** (`examples/`): a minimal Rust embedder, a minimal Node embedder, a browser snippet, and a CLI-in-CI snippet — all built/tested in CI so they can't rot.

### 5.4 The contract page (semver + supported-Godot matrix)

A dedicated docs page (also summarized in Workstream 6) stating the 1.0 stability policy and the supported-Godot-version matrix verbatim (the policy statement + matrix sketches are in Workstream 6).

---

## Workstream 6 — API stabilization & the 1.0 commitment

This is the **irreversible** workstream — at 1.0 the `gdscript-ide` public surface (+ the FFI POD shape) becomes a contract a 2.0 is needed to break. So it gets an explicit **API-review pass** before the cut.

### 6.1 Freeze + document the public surface

- **Inventory** every `pub` item reachable from `gdscript-ide`'s root and from the `gdscript-ffi` POD JSON. This *is* the contract; everything else (`gdscript-hir`, `-db`, `-syntax`, `-api` internals) is **not** stable and is documented as such.
- **`#[non_exhaustive]` everywhere a variant/field will grow:** `WarningCode` (we add warnings as Godot does), `Diagnostic`, `CompletionItem`, `HoverResult`, `CodeAction`, and every result struct/enum a consumer matches on — so additive growth is a **minor**, not a major. Sketch:

```rust
#[non_exhaustive] pub struct Diagnostic { pub range: TextRange, pub code: WarningCode,
    pub severity: Severity, pub message: String, pub fixes: Vec<CodeAction> }
#[non_exhaustive] pub enum Severity { Error, Warning, Information, Hint }
```

- **API-review checklist:** naming consistency, no `lsp-types` leaked into the core ([`01`](01-ARCHITECTURE.md) §2), all positions are byte offsets (UTF-16 conversion is the client's job), `Cancellable<T>` on every read query, POD is `serde`-round-trippable and matches the documented JSON schema.

### 6.2 The semver / stability policy statement (verbatim for the contract page)

> **`gdscript-analyzer` 1.0 stability policy.** From 1.0.0 we follow **SemVer 2.0.0**. The **stable public API** is the `gdscript-ide` crate's public surface and the `@gdscript-analyzer/*` npm packages' POD JSON result shapes — one shared version across crates.io and npm. **MAJOR** = a breaking change to that surface (removing/renaming a method, changing a field's type, removing an enum variant, narrowing accepted input). **MINOR** = additive, backward-compatible change (new method, new `#[non_exhaustive]` variant/field, new warning code, new config option). **PATCH** = bug/behavior fix with no surface change. **Not covered by this guarantee:** the internal crates (`gdscript-hir`, `gdscript-db`, `gdscript-syntax`, `gdscript-api`, `gdscript-scene`); the *exact wording* of diagnostic messages (we track Godot's strings, which change between engine versions — the stable identifier is the **code**, not the message); inference **precision** (a value previously typed `Variant` becoming a concrete type, or a `UNSAFE_*` warning that previously fired no longer firing because narrowing improved, is a **quality** change shipped in MINOR/PATCH, not a break). **Deprecation policy:** a stable item is marked `#[deprecated]` with a pointer to its replacement for **≥1 minor release** before removal in the next major. MSRV bumps are **minor** (documented in the changelog). `cargo-semver-checks` runs in release CI and blocks an unmarked break.

### 6.3 The supported-Godot-version matrix (verbatim for the contract page)

The analyzer bundles several Godot minor versions' API ([`01`](01-ARCHITECTURE.md) §5; [`GODOT-SYNC.md`](GODOT-SYNC.md)) and selects per project ([`research/04`](research/04-gdscript-semantics-and-features.md) §3.6 version detection). The matrix states, per analyzer version, which Godot minors are **supported** (bundled API + tested), and the policy for new Godot releases:

| Analyzer | Bundled Godot APIs | Default | Notes |
|---|---|---|---|
| `1.0.x` | 4.3, 4.4, 4.5, (newest stable at cut) | newest | 4.3 = oldest supported; master-only warnings gated off for 4.3 |
| policy | a new Godot **minor** → added in a **minor** analyzer release (additive, via Godot-sync PR) | newest | dropping an old Godot minor → a **major** analyzer release |

Policy statement: **"We support the latest N Godot minors (N≥3). A new Godot stable minor is picked up automatically by the Godot-sync workflow and shipped in the next minor analyzer release. Dropping support for a Godot minor is a breaking change (major bump)."** Per-project selection is detected from `project.godot` `[application] config/features`, snapped to the nearest bundled minor, newest as default, overridable ([`01`](01-ARCHITECTURE.md) §5).

---

## Workstream 7 — Ecosystem maturity (governance for a 1.0 community project)

1.0 invites contributors and external consumers; governance must scale from "founder + ADRs" toward "community project" — but **only as far as volume warrants** ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2: *"RFCs are a tool, not a burden"*).

- **ADRs → RFCs, conditionally.** Stay on **lightweight in-repo ADRs** ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2) as the default. **Graduate to a separate `rfcs` repo with an FCP** *only if* external-contributor volume justifies it (the explicit trigger from research 07). At 1.0 we likely keep ADRs + a `proposal` issue form + an `S-needs-design` label (Bevy model); we document the *trigger* for graduating, not graduate prematurely.
- **Issue triage at scale.** Seed the rust-analyzer-style labels ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.3): `C-bug`/`C-enhancement`/`C-diagnostic`/`C-architecture`, `E-easy`/`medium`/`hard`, `S-actionable`/`needs-repro`/`needs-info`/`needs-design`, `good-first-issue`. A `diagnostic` issue form requires a GDScript snippet + expected-vs-actual diagnostic + Godot version + analyzer version + crate-vs-npm + OS (so warning-parity bugs are reproducible). Milestones per release.
- **A public roadmap.** Publish the post-1.0 roadmap (this doc's "Post-1.0 outlook" + a GitHub Projects board / pinned issue) so contributors and consumers see what's next and what's deliberately deferred.
- **First external-consumer outreach (the ≥1-external-consumer criterion).** [`00`](00-VISION-AND-SCOPE.md) §6 requires *"≥1 external consumer beyond guitkx and our own LSP/CLI."* Active steps: the **formatter is a natural second consumer** (Workstream 3 — guitkx's embedded-GDScript range-format *and* anyone wanting a Rust/WASM `gdformat`); announce the standalone LSP to the Godot community (the documented demand — proposal #11056, the externalize-the-LSP crowd); a "powered by gdscript-analyzer" badge + the "add a client" guide lowering the integration cost; reach out to existing GDScript tooling authors (an alternative editor extension, a CI-lint project) to adopt the CLI/crate. Track the criterion as a release blocker.

---

## Testing strategy

1. **The full warning corpus vs engine output.** For each of the 48 codes, a fixture (`fixtures/warnings/<code>.gd` + `.expected`) that triggers it, asserting **code + byte range + verbatim message + resolved severity**. A **differential harness** runs the same typed corpus through real Godot (`--check-only`/editor export) at each supported minor and diffs — **with two documented exceptions** treated as expected divergence, not failure: (a) the `UNSAFE_PROPERTY_ACCESS`/`UNSAFE_METHOD_ACCESS` cases we correctly suppress via narrowing (#93510), and (b) default-level normalization (Godot ships the opt-in group off; we normalize gating before comparing). Master-only codes are only diffed against master.
2. **Gating + suppression.** Fixtures pairing a `.gd` with synthetic `project.godot` settings: master `enable=false` ⇒ zero warnings; per-code override flips level; `treat_warnings_as_errors` escalates; `exclude_addons`/`directory_rules` suppress by path; `@warning_ignore`/`_start`/`_restore` suppress the right spans; unknown ignore name → meta-diagnostic. Assert editing a warning setting **does not** invalidate `infer`/`flow` (query-recount test).
3. **Narrowing correctness incl. the #93510 cases.** Golden fixtures for every row of the §2.2 table: `is`-guard suppresses `UNSAFE_*` in the `then` branch (the headline win), early-return narrows past the guard, `and`/`or` short-circuit, `match` arms, **reassignment invalidates** narrowing, **opaque call invalidates** `self`-member narrowing (soundness — assert we *don't* wrongly suppress), `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` fire. A **soundness property test:** narrowing never produces a member access we then can't justify (no "narrowed wrongly").
4. **Formatter idempotence + gdformat parity.** `format(format(x)) == format(x)` property test over the whole corpus + fuzzer output; semantic-preservation (re-parse, assert AST-equivalent modulo trivia); the **`gdformat` parity set** (output must match real `gdformat`, deviations are documented golden exceptions); range-format a sub-span (the guitkx path) leaves the rest byte-identical.
5. **Large-project perf regression.** The criterion `bench` job on the tiered real-game fixture, `--save-baseline` vs the committed baseline, **fail on >10% regression** of any tracked metric (§4.2); the warm-keystroke + bounded-invalidation metrics asserted via query-recount, not just wall-clock.
6. **Cross-target release tests.** The Phase-0 CI matrix green at 1.0: native 3-OS, the napi per-platform matrix, the wasm build; a **wasm-size guard** fails over budget (§4.4); a smoke test loads the wasm + pruned API asset and analyzes a snippet in-browser (headless); `npm i @gdscript-analyzer/core` + `cargo add gdscript-ide` both resolve and run a hello-analyze.
7. **API-stability checks.** `cargo-semver-checks` in release CI blocks an unmarked break to `gdscript-ide` (Workstream 6); a **doctest** of the documented public-API usage example; a test that the FFI POD JSON matches the documented schema (so npm and Rust contracts can't diverge); a `#[non_exhaustive]` audit (every consumer-matched type carries it).

---

## Exit criteria (the v1.0 bar from [`00`](00-VISION-AND-SCOPE.md) §6, as a testable checklist)

All must pass for the 1.0 tag.

- [ ] **Real multi-file projects:** cross-file goto/find-refs/rename, autoloads, `class_name` work on the large real-game fixture (carried green from Phase 3; re-asserted under 1.0 perf).
- [ ] **Scene-typed node paths:** `$Panel/VBox/StartButton` infers `Button` from `.tscn` with zero annotations; invalid `$DoesNotExist` warns (carried from Phase 4).
- [ ] **The full 48-warning set** emits with **engine-matching messages + codes + default levels**, gated by `debug/gdscript/warnings/*`, honoring `@warning_ignore[_start|_restore]`; the differential harness matches Godot modulo the two documented divergences.
- [ ] **Full flow narrowing (1.0 cut):** the §2.2 rules hold; the **#93510 win** is demonstrable — `UNSAFE_*` suppressed inside proven `is`/`as`/`!= null` guards where the engine still warns; narrowing is **sound** (never narrows wrongly).
- [ ] **Formatter:** idempotent, semantics-preserving, **`gdformat`-parity** (modulo documented deviations) on the golden corpus; available via CLI `format` and LSP `formatting`/`rangeFormatting`.
- [ ] **Performance:** warm keystroke < 10 ms flat as the project grows; bounded invalidation on signature/`class_name` edits; the `bench` CI guard is active (>10% regression fails).
- [ ] **Stable, documented, semver'd packages:** `cargo add gdscript-ide` + `npm i @gdscript-analyzer/core` install **1.0.0**; the contract page (semver policy + supported-Godot matrix) is published; `cargo-semver-checks` gates releases; `#[non_exhaustive]` audit passes.
- [ ] **A live web playground** analyzes pasted GDScript in-browser, within the wasm bundle budget, and is wired into the docs (warning + narrowing pages link live demos).
- [ ] **Docs complete:** mdBook guide (install / consume-from-Rust-Node-browser / configuration / per-warning reference / "add a client" guide) finished and link-checked; docs.rs API docs polished; examples build in CI.
- [ ] **≥1 external consumer** beyond guitkx and our own LSP/CLI is using the crate or a published package (tracked as a release blocker; the formatter-as-second-consumer + community outreach feed this).
- [ ] **wasm32 portability check** green; the full cross-target release matrix green.

---

## Post-1.0 outlook (what's deliberately deferred)

1.0 is a floor, not a ceiling. Explicitly deferred (and stated as such in the public roadmap, Workstream 7):

- **The multi-year narrowing tail** ([`research/09`](research/09-type-system-and-inference.md) §7) — narrowing through call results, loop-carried fixpoints, aliasing, discriminated-union/enum-tag narrowing. 1.0 fixed *soundness*; post-1.0 raises *precision*, shipped in MINOR/PATCH (it's a quality change, not an API break — §6.2).
- **GDScript 3.x / Godot 3** — out of scope unless demand appears ([`00`](00-VISION-AND-SCOPE.md) §5). Would be a separate parser/API track.
- **Deeper refactorings** — extract method/variable, inline, change-signature, organize-imports-style preload cleanup; and **call hierarchy** ([`research/04`](research/04-gdscript-semantics-and-features.md) §4 "Later"/Phase 3+).
- **More language bindings** — PyO3 → PyPI, the C ABI (cbindgen) reaching Go/C#/Swift/Java ([`01`](01-ARCHITECTURE.md) §4 "cheap optionality, post-v1"). No core change required.
- **Performance via parallelism** — feature-gated, native-only (`#[cfg(feature = "parallel")]`), never in the wasm hot path ([`01`](01-ARCHITECTURE.md) §7 rule 3).
- **Recursive / inherited scene edge cases** beyond Phase 4's common slice — deep instanced-sub-scene chains, multi-owner scripts typed to a common base, `@onready` timing subtleties.
- **An RFC repo + FCP governance** — graduate from ADRs only when contributor volume warrants (§Workstream 7).

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| **Warning-message drift vs the engine.** Godot changes message strings between minors; the integer enum values shift between 4.3 and master. | Key everything on the **symbolic code** (never the int — [`research/04`](research/04-gdscript-semantics-and-features.md) §2.2); make the **message** explicitly *not* part of the semver contract (§6.2 — stable id = code, not text); the **Godot-sync** workflow ([`GODOT-SYNC.md`](GODOT-SYNC.md)) surfaces message deltas per release; the differential harness (Testing #1) per supported minor catches drift early; version-gate master-only codes (`WarningCode::since`). |
| **Narrowing complexity / unsoundness.** A full CFG with mutation + aliasing is where subtle bugs (wrong narrowing → hidden real warning, or asserting a member that isn't there) live. | **Conservative-by-construction:** when unsure, widen to declared/`Variant` — never narrow wrongly (§2.3). Draw the **1.0 cut** at shallow, stable `Place`s + conservative invalidation; defer call-result/aliasing/loop-fixpoint to post-1.0. A **soundness property test** (Testing #3) guards the invariant. |
| **Formatter bikeshedding / `gdformat` parity.** Formatting style is endlessly debatable; "match gdformat exactly" is a moving, externally-owned target. | **Minimal config** (§3.2 — four options, tabs default); a **documented compatibility statement** (where we match `gdformat`, every deviation explicit); a **parity golden set** run through real `gdformat` so divergences are visible and intentional, not accidental; **idempotence + semantics-preservation** are the hard invariants, style is secondary. |
| **1.0 API lock-in regret.** Freezing `gdscript-ide` too early forces a premature 2.0; freezing wrong shapes hurts consumers. | An explicit **API-review pass** before the cut (§6.1); `#[non_exhaustive]` on every growable type so additive change stays minor; the contract page scopes the guarantee tightly (only `gdscript-ide` + FFI POD; precision/messages/internals excluded); `cargo-semver-checks` enforces it; the two real consumers (guitkx + the standalone LSP) exercise the surface *before* the freeze. |
| **Sustaining the community.** A 1.0 attracts users faster than a solo maintainer can triage; governance can ossify or burn out. | Scale governance **only as volume warrants** (§Workstream 7; research 07's "tool, not burden"); seed triage labels + issue forms so reports are actionable; a public roadmap sets expectations on what's deferred; the Godot-sync automation keeps the highest-churn maintenance task (engine API drift) hands-off. |
| **The ≥1-external-consumer criterion stalls** (adoption is not fully in our control). | Make adoption **cheap and obvious:** the "add a client" guide (rust-analyzer model), the formatter as a low-friction second consumer, the standalone LSP answering documented community demand (#11056), and direct outreach to existing GDScript-tooling authors — tracked as a release blocker, not an afterthought. |
| **Perf regressions slip in** as features land post-Phase-3. | The **`bench` CI guard** (>10% fails the PR), query-recount tests for the incremental invariants, and a real-game fixture so benchmarks reflect production, not microbenchmarks. |

---

## References (relative links)

- [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) — **the v1.0 success bar (§6)**; scope/non-goals (§5); consumers (§4); guiding principles (§7).
- [`01-ARCHITECTURE.md`](01-ARCHITECTURE.md) — crate stack (§1); **`gdscript-ide` = the semver contract (§2)**; salsa/durability (§3); FFI/WASM + bundle strategy (§4); data model + multi-version (§5); portability rules (§7); cross-cutting decisions incl. SemVer (§9).
- [`ROADMAP.md`](ROADMAP.md) — Phase 6 = v1.0, Tier 3 full; deliverable + exit criteria; the Tier→Phase table (Tier 3 full = "multi-year polish").
- [`GODOT-SYNC.md`](GODOT-SYNC.md) — the `gdscript-api` data pipeline + message-delta surfacing the warning set tracks.
- [`PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md`](PHASE-2-API-AND-SINGLE-FILE-SEMANTICS.md) — the curated warning subset + the **emit-then-gate seam** this phase completes; syntactic narrowing this phase replaces with a CFG.
- [`PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md`](PHASE-3-PROJECT-WIDE-AND-INCREMENTAL.md) — the `project.godot` parse (warning settings source), salsa adoption (perf-tuned here), the keystroke-latency invariant measured here.
- [`PHASE-4-SCENE-AWARENESS.md`](PHASE-4-SCENE-AWARENESS.md) — scene typing the node/onready warnings depend on; scene-heavy perf fixtures.
- [`PHASE-5-CLIENTS-AND-DISTRIBUTION.md`](PHASE-5-CLIENTS-AND-DISTRIBUTION.md) — the LSP `formatting` capability + CLI `format` (consume Workstream 3); the playground (live docs); 0.x GA → 1.0; the guitkx source-map adapter the formatter reuses.
- [`research/04-gdscript-semantics-and-features.md`](research/04-gdscript-semantics-and-features.md) — **PRIMARY**: the full 48-warning set + default levels (§2.2), `debug/gdscript/warnings/*` gating + `@warning_ignore` (§2.3), the 36 annotations (§1.10), the complete LSP feature list (§4), the engine-LSP gaps we fill (§5).
- [`research/09-type-system-and-inference.md`](research/09-type-system-and-inference.md) — **PRIMARY**: full flow narrowing / binder-checker CFG (§3.2), beating the engine on `is`/`as` guards (§1.6, #93510), the `UNSAFE_*` family + verbatim messages (§1.7), the Tier-3 multi-year tail (§7).
- [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) — **PRIMARY**: the 1.0 semver commitment + Cargo 0.x→1.0 reading (§3.3), docs completeness + the "add a client" page (§5), governance maturity ADR→RFC + triage (§6), `cargo-semver-checks` in release CI (§3).
