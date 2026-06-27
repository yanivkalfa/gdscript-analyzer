# Phase 6 · Execution Overview — sequencing the v1.0 release across the seven workstreams

> The synthesis layer over the seven Phase-6 workstream playbooks. It answers the questions the
> individual playbooks can't: **what across the whole phase is already built vs net-new, what order to
> build in given the real cross-workstream dependencies, where the 1.0 cut line falls per workstream,
> which work is risky/large, and what to build first.** It also cross-checks the seven playbooks for
> consistency and flags the conflicts/gaps it found.
>
> **Reads:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) (canonical plan) and the seven playbooks:
> [W1 warnings](PHASE-6-W1-WARNINGS-PLAYBOOK.md), [W2 narrowing](PHASE-6-W2-NARROWING-PLAYBOOK.md),
> [W3 formatter](PHASE-6-W3-FORMATTER-PLAYBOOK.md), [W4 perf](PHASE-6-W4-PERF-PLAYBOOK.md),
> [W5 docs](PHASE-6-W5-DOCS-PLAYBOOK.md), [W6 API stabilization](PHASE-6-W6-API-STABILIZATION-PLAYBOOK.md),
> [W7 ecosystem](PHASE-6-W7-ECOSYSTEM-PLAYBOOK.md).
>
> **Verified against the tree** (`cleanup_and_upgrades`, 2026-06-27): `non_exhaustive` appears only in
> an unrelated `finish_non_exhaustive()` Debug impl; **no `WarningCode`, no `flow.rs`, no `warnings.rs`,
> no `gdscript-fmt` crate, no `fixtures/perf`, no `examples/`**; `ide::semantic::type_diagnostics` is
> literally `queries::analyze_file(db, file).diagnostics.clone()`. The "what's already done" claims below
> are the union of the seven playbooks' grounding, spot-checked and consistent with the code.

---

## 1. State of Phase 6 — already built vs net-new (one paragraph)

**Everything *functional* exists; almost nothing *Phase-6-specific* does.** By the start of Phase 6 the
analyzer already ships, at `0.2.1`, the entire load-bearing substrate the seven workstreams build on: a
**lossless `cstree` CST** that round-trips byte-for-byte (W3's hard precondition — fully met), a **mature
salsa graph** with a two-tier durability model and *already-firewall-tested* bounded invalidation
(W4's machinery — "measure + guard," not "build"), **working lexical `is`/`as`/`!=null` narrowing** with
the load-bearing `is_uninformative` + widen-only soundness guard and cross-file ScriptRef narrowing
(W2 *extends and formalizes* this, it does **not** greenfield it), the **full `AnalysisHost`/`Analysis`
public surface** with `Cancellable<T>` on all 15 read queries and a complete serde POD result set in
`gdscript-base` (W6's surface is "already the right shape"), a **deployed mdBook + playground + docs.rs
metadata on all 8 crates** (W5 is "wiring, not writing"), and an **over-delivered governance scaffold**
— Code of Conduct, Governance, the canonical numbered issue forms, a working ADR mechanism (W7 is mostly
de-duplication + adoption). What is genuinely **net-new** is the Phase-6 delta: there is **no
`WarningCode` enum, no `RawWarning`, no `gate()` filter layer** (W1's whole premise that a "Phase-2
emit-then-gate seam" exists is false — severity is baked at every emit site today); **no CFG / `Place` /
`FlowFacts` / reachability / `UNREACHABLE_*` emitter** (W2); **no `gdscript-fmt` crate, no `Doc` IR, no
`Analysis::format`** (W3); **no `fixtures/perf`, no project-scale bench, no dhat/LRU/brotli-asset
pipeline, no bench-or-wasm-size CI** (W4); **no Configuration/Warning-Reference/client/contract pages,
no docgen xtask, no `examples/`** (W5); **zero `#[non_exhaustive]`, no PR-time semver gate, no public
Godot-version accessor** (W6); and **no labels-as-code, no roadmap board, no external-consumer
blocker** (W7). The honest framing: **Phase 6 is a quality + commitment phase that adds two real new
subsystems (the warning-gating seam and the CFG) plus a new analysis-free crate (the formatter), and
then freezes a surface that already exists.**

---

## 2. The critical path + recommended sequencing

### 2.1 The dependency graph (only the load-bearing edges)

```
                 ┌──────────────────────────────────────────────────────────┐
   W2 (CFG) ─────┤ produces reachability + match-wildcard data ──► W1 emits  │
      │          │ correct narrowing suppresses UNSAFE_* (#93510) ─► W1 gates│
      │          └──────────────────────────────────────────────────────────┘
      │                                  │
      │  (UNREACHABLE_* + UNSAFE_* suppression)
      ▼                                  ▼
   W1 (warning set + gate + suppression) ──► WarningCode enum ──► W5 (docgen 48 pages)
      │                                                              ▲
      │  Diagnostic.code type freeze (joint decision) ──► W6 ────────┘ (contract prose)
      ▼                                                     │
   (all warnings additive under #[non_exhaustive])          │
                                                            ▼
   W3 (formatter) ──► Analysis::format on the frozen surface ──► W6 reviews it
      │                                                          │
      │  (formatter = guitkx's 2nd consumer)                     │
      ▼                                                          ▼
   W7 (external-consumer blocker) ◄── W5 "add a client" guide ◄──┘ W6 contract page
                                                                      ▲
   W4 (perf) ── independent; re-baseline AFTER W1/W2 land ────────────┘ (drives the frozen API)
```

**The real cross-workstream edges (the ones that dictate order):**

| Edge | Type | Why it's load-bearing |
|---|---|---|
| **W2 → W1** | hard, code | `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` need W2's CFG reachability; the **correct** `UNSAFE_*` suppression (#93510) needs W2's narrowing. W1 *declares + gates* these; W2 *makes them fire correctly*. Shared seam: `emit_unsafe` (`infer.rs:~1459-1476`). |
| **W1 → W5** | hard, code | The 48-page warning reference is **generated** from `WarningCode` + its `default_level`/`setting_name`/`message`/`since` methods. No enum ⇒ no real pages. (Mitigated: W5 builds docgen against a 7-entry stub, snaps in `WarningCode::ALL` later.) |
| **W1 ↔ W6** | hard, judgment | `Diagnostic.code`'s **type** is a frozen contract field. W6 must decide *with* W1: keep `code: String` (W1's recommendation, non-breaking) vs flip to `code: WarningCode` (the plan's sketch, a POD break). **Freeze once, at the cut.** |
| **W3 → W6** | hard, code | `Analysis::format`/`format_range` + `FmtConfig` join the frozen surface; `FmtConfig` needs `#[non_exhaustive]`. The formatter API must be **settled before** the W6 freeze. |
| **W6 → W5** | content | W5's contract page embeds W6's verbatim policy + Godot matrix; W5 carries the correction that the contract = `gdscript-ide` **+ re-exported `gdscript-base` POD**. |
| **W3 → W7** | adoption | The formatter is guitkx's turnkey second consumer and a standalone "Rust/WASM gdformat" adoption hook — but **does not by itself clear** the ">beyond guitkx" bar. |
| **W5 → W7** | adoption | The "add a client" guide lowers integration cost feeding the ≥1-external-consumer criterion. |
| **W1/W2 → W4** | timing | W1's gating + W2's `flow(body)` add per-edit compute; the warm-keystroke + bounded-invalidation **benches must run after them, or be re-baselined when they land.** `flow(body)` is also a prime LRU candidate to coordinate with W2. |

### 2.2 The critical path (longest hard-dependency chain)

```
W2 M0–M2 (CFG + narrowing) → W1 M0 (gating seam) + M1 (checks) + M3 (W2-coordinated codes)
   → W6 M1 (#[non_exhaustive], incl. the Diagnostic.code type freeze, joint with W1)
   → W5 M5 (snap in WarningCode::ALL → 48 pages) + M6 (contract page from W6)
   → W7 (external-consumer blocker resolves)  ── the 1.0 TAG.
```

**This is the chain that determines the ship date.** The two genuinely *new subsystems* (W2's CFG, W1's
gate) sit at its head; the irreversible freeze (W6) sits in the middle; the doc fill and the
adoption gate sit at the tail. W3 and W4 hang **off** this chain, not on it.

### 2.3 Recommended sequencing (what to start when)

**Tier A — start immediately, in parallel (no inbound hard deps):**

- **W2 (CFG + narrowing).** It is the **head of the critical path** (W1's two hardest codes and its
  headline #93510 win depend on it) and it is a *real new subsystem* with the longest unknown
  (soundness). Start it first. Build + golden-test the **pure CFG (M0)** before touching the checker.
- **W1 M0 (the gating skeleton).** The `WarningCode` enum + `gate()` + settings-parse + suppression map
  is the **prerequisite for the entire rest of W1** *and* unblocks W5's docgen and W6's `code`-type
  decision. It does **not** need W2 — only the two CFG-codes + the #93510 suppression do. Start W1 M0 in
  parallel with W2 M0. (See §5 — this is the recommended *first concrete milestone* for the phase.)
- **W3 (formatter).** **Fully independent** — analysis-free, only touchpoint is the cached `parse`
  query. No dep on W1/W2/W4. Build it in parallel start-to-finish; its only downstream coupling is that
  its API must land **before** the W6 freeze.
- **W4 M0 (vendor `fixtures/perf` + cold bench).** Independent and **unblocks all later perf work**;
  the warm/keystroke benches are re-baselined after W1/W2, but the fixture + cold bench can land now.
- **W7 step 6 (external-consumer outreach).** **Start first despite producing no code** — it is the only
  exit criterion engineering can't satisfy and it is **calendar-bound, not effort-bound**. The other W7
  steps (labels, forms, roadmap) are hours of doc/config PRs droppable anywhere.

**Tier B — once Tier A heads land:**

- **W1 M1–M2** (the 35+ self-contained checks + full gating/suppression) — needs only W1 M0, runs in
  parallel per-check.
- **W2 M3 → W1 M3** — the W2/W1 handoff for `UNREACHABLE_*` and `UNSAFE_*` suppression.
- **W5 M0–M4** (structural SUMMARY, docgen-against-stub, playground deep-links, examples, docs.rs polish)
  — all proceed against the stub; only the 48-page fill (M5) waits on W1.
- **W4 M1–M5** (recount harness, keystroke/parse/completion benches, dhat/LRU, brotli pipeline, CI
  guards) — re-baseline keystroke after W1/W2.

**Tier C — the freeze and the tail (sequenced, near the end):**

- **W6** is the **last engineering workstream**, by construction. `#[non_exhaustive]` is *itself* a
  breaking change, so it **must ride the final pre-1.0 break** — after W3's formatter API exists and
  W1's `Diagnostic.code` type is decided, before the tag. Everything that reshapes the public surface
  (W1 codes, W3 format methods, W6's accessors) must be **in** before W6 M1 fires.
- **W5 M5/M6** (snap in WarningCode + the contract prose) and **W7's de-stale + final external-consumer
  confirmation** are the very last steps, gated on W1 and W6 respectively.

**The one ordering rule that matters most:** **do not freeze (W6 M1) until every surface-reshaping edit
from W1, W3, and W6 itself is landed.** A missed `#[non_exhaustive]` or a late `Diagnostic.code` type
flip forces a premature 2.0.

---

## 3. The explicit 1.0 cut-line decisions (per workstream)

| WS | IN 1.0 | DEFERRED post-1.0 (on the public roadmap) |
|---|---|---|
| **W1 warnings** | The full emit-then-gate machinery (`WarningCode` + `RawWarning` + pure `gate()` + `WarningSettings` + `@warning_ignore[_start\|_restore]` suppression map); **all self-contained checks** (~35+: unused/unassigned/shadowing/numeric/assert/standalone/confusable/deprecated families); complete project-setting gating (master switch, per-code override, treat-as-errors, scope rules) + version-gating master-only codes. `Diagnostic.code` stays **`String` on the wire** (`WarningCode` internal, bridged by `as_str()`). | `GET_NODE_DEFAULT_WITHOUT_ONREADY` (scene-dep, wiring is W1-adjacent). `UNREACHABLE_*` + correct `UNSAFE_*` suppression are **owned jointly with W2** (W1 declares+gates, W2 makes them fire). `TYPE_MISMATCH`/`INVALID_NODE_PATH`/`CYCLIC_INHERITANCE` stay analyzer-native + **ungated** (no engine key). The Godot-differential harness is a **separate opt-in CI job** (needs a Godot binary), not default `xtask ci`. |
| **W2 narrowing** | Flow-sensitive narrowing of locals + shallow `self`/field `Place`s through `if`/`else`/`elif`, `!=null`/`if x:` → `NotNull`, early-return flow-past-guard, `and`/`or` short-circuit, `match` arms; conservative invalidation on reassignment + opaque calls; `UNREACHABLE_CODE`/`UNREACHABLE_PATTERN` **data** (emission owned by W1). **Soundness FROZEN at 1.0** (widen when unsure, never narrow wrongly). | Narrowing through arbitrary **call results** (`if get_thing() is T:`); **loop-carried back-edge fixpoints** (loops entered widened, no iteration); **aliasing**; enum/int **discriminant** narrowing. Precision is a MINOR/PATCH quality change, not an API break. |
| **W3 formatter** | A `gdscript-fmt` crate: whole-file `format` + `format_range`, **Wadler/Prettier Doc IR** (vendored ~150 lines, no external pretty-printer dep), four config options (`line_width=100`, `indent=tabs`, `indent_size=4`, `safe_mode=true`); **idempotence + semantics-preservation** as hard invariants; `gdformat` parity as a **documented superset** (golden corpus + `DEVIATIONS.md`, not byte-identity); error-tolerant `fallback_verbatim`. `FmtConfig` passed as an **argument, not a salsa input**; no salsa-cached format query. Wired into CLI `format` + LSP `formatting`/`rangeFormatting`. | Magic-trailing-comma semantics; comment prose reflow; style profiles beyond the four options; member/preload sorting; formatting inside string literals. |
| **W4 perf** | Tiered `fixtures/perf/{small,medium,large}`; criterion harness for cold-analyze / warm-keystroke (<10ms, **flat**) / member-completion (<5ms) / parse-MB/s; **dhat** memory profiling + documented resident ceiling; **salsa LRU** for cold-file derived data; `wasm-opt -Oz` + **brotli + content-hashed fetched** engine asset; **>10% bench guard** (CodSpeed/Bencher) + **byte-ceiling wasm-size guard**. | Per-query parallelism (rayon stays CLI file-fan-out only, native-only, feature-gated); the zero-copy `rkyv::access` rewrite of `EngineApi::from_bytes` (measure first); sub-ms absolute cold-start; semantic-token full/delta streaming. |
| **W5 docs** | Completed `SUMMARY.md` (Configuration, generated Warning Reference, per-editor client pages + "add a client" guide, Reference branch incl. contract page); the **docgen xtask + `--check` CI gate** (anti-drift); playground-as-live-docs deep links; CI-built `examples/`; docs.rs polish (doctest, POD docs, `deny(missing_docs)`, internal-crate "not stable" banners). | VitePress/Astro front-site; clap-generated CLI reference (1.0 ships hand-written-but-tested); per-editor screencasts; translations; deep docs on internal crates. |
| **W6 API** | `#[non_exhaustive]` on all consumer-matched enums/result-structs + `Change` (+ `::new` constructors); the small value types (`FileId`, `TextRange`, `FilePosition`, `FileRange`, `LineCol`, `TextEdit`, `FileEdit`) stay **literal-constructible on purpose**; contract scoped to **`gdscript-ide` + `gdscript-base` + the FFI JSON**; **PR-time + release-time** `cargo-semver-checks`; public `bundled_godot_version()`/`project_engine_version()` accessors. | The façade refactor privatizing `gdscript-base` (a 2.0 move); `cargo-public-api` snapshot tests; TS `.d.ts` codegen. |
| **W7 ecosystem** | Labels-as-code (`.github/labels.yml` + sync Action) with the full rust-analyzer taxonomy; `04-diagnostic.yml` parity form (+ #93510 expected-divergence checkbox); **observable** RFC-graduation trigger recorded as ADR-0004; public roadmap page/board; pinned **external-consumer release-blocker**; de-stale the "Phase 0" banners; delete the stale duplicate template set. | A separate `rfcs` repo + FCP; steering committee/voting; triage bots; a bespoke community site. |

---

## 4. Effort / risk ranking

Ordered by **(effort × risk)**, largest first. Effort and the dominant risk are each from the playbooks'
own estimates, reconciled here.

| Rank | WS | Effort | Dominant risk | Why it ranks here |
|---|---|---|---|---|
| 1 | **W1 warnings** | **Large** (~6–8 wks, partly parallel) | **Critical:** gating in the wrong place in the salsa graph silently breaks the keystroke-latency/incremental-cache invariant; secondary: the assumed Phase-2 seam doesn't exist, so M0 must build it before anything. | Biggest surface (48 codes), and it builds a *new subsystem* (the gate seam) on a false premise the plan asserts. The single most code + the most check-by-check breadth. |
| 2 | **W2 narrowing** | **Medium-large** (~200–300 LOC CFG + the dataflow + rewire) | **Critical:** unsound narrowing hides a real `UNSAFE_*` or asserts an absent member — worse than the engine's over-warning. | A new subsystem (real CFG) at the *head of the critical path*; soundness is the hardest correctness bar in the phase. Mitigated by preserving the existing `is_uninformative` guard verbatim + a soundness property test. |
| 3 | **W4 perf** | **Medium** (~3–4 wks, 6 milestones) | **High:** CI bench noise (±10–30% on shared runners) flaps a naive gate; secondary: unverified salsa-0.27 LRU API + the browser-brotli decode story. | Lots of net-new infra (fixtures, benches, dhat, LRU, brotli pipeline, CI), but the incremental *machinery* + recount idiom already exist to reuse. Risk is operational, not correctness. |
| 4 | **W3 formatter** | **Medium** (Doc IR is the bulk; rule set is breadth; wiring is mechanical) | **High (correctness):** semantics-preservation under significant indentation; **High (fuzzy):** gdformat parity is an externally-owned moving target. | Greenfield crate but **fully parallelizable** and analysis-free — the cheapest large piece to land correctly once the Doc IR is in. Both risks are well-fenced (AST-equality + `safe_mode`; golden corpus + `DEVIATIONS.md`). |
| 5 | **W5 docs** | **Medium** (mostly content + one real engineering piece: the docgen xtask) | **Critical (narrow):** the warning reference drifting from the impl; **High (dep):** `WarningCode` doesn't exist yet. | Machinery (mdBook/Pages/docs.rs/playground/linkcheck) already works; de-risked by the docgen `--check` gate + stub-table-then-snap-in. |
| 6 | **W6 API** | **Small-to-medium**, mostly mechanical (~22 attributes + constructors + 1 CI job + 2 accessors + prose) | **High (timing):** `#[non_exhaustive]` is *itself* a break — must land in the **last** pre-1.0 release; missing one type forces a premature 2.0. | Low volume, but **irreversible** and **judgment/coordination-heavy** (the `Diagnostic.code` freeze with W1, the publish-status decision, completeness of the attribute set). |
| 7 | **W7 ecosystem** | **Small** (~1–2 eng-days of doc/config PRs) | **Critical (schedule, not labor):** the ≥1-external-consumer criterion can stall the *tag* — adoption isn't in our control. | Lowest engineering cost; the only one whose risk is calendar, which is exactly why its outreach item starts *first*. |

**The two things that can actually slip the date:** (1) **W2 soundness** debugging (a subtle CFG/flow
bug is the hardest to find), and (2) **W7's external-consumer** adoption lead time. Everything else is
effort that parallelizes.

---

## 5. The suggested first concrete milestone

**Build W1 M0 — the gating skeleton — first, in parallel with W2 M0 (the pure CFG).**

W1 M0 is the highest-leverage opening move in the whole phase:

1. It is the **prerequisite for the entire rest of W1** (you can't "emit all 48" until there's a `gate()`
   to route them through — the plan's premise that this seam exists is false).
2. It **unblocks W5's docgen** (the `WarningCode` enum + its `default_level`/`setting_name`/`message`/
   `since` methods are the docgen source of truth) and **W6's `Diagnostic.code` type decision** (freeze
   `String` once `WarningCode::as_str()` exists to bridge it).
3. It validates the **single load-bearing architectural decision of the phase** end-to-end before any
   breadth work: **`gate()` must run DOWNSTREAM of the `#[salsa::tracked]` `analyze_file` query** — in
   `ide::semantic::type_diagnostics` (`semantic.rs:47`, today literally
   `queries::analyze_file(db, file).diagnostics.clone()`), keyed on the MEDIUM-durability
   `warning_settings` query — **not inside `analyze_file`**, or editing a warning level invalidates
   inference (the salsa-cacheability violation W1, W4, and the plan all hard-require).

**Concrete W1 M0 exit criteria (the proof the seam works):**

- `crates/gdscript-hir/src/warnings.rs` exists: `WarningCode` (all 48 + the `as_str`/`setting_name`/
  `default_level`/`since`/`from_setting_name` tables), `WarnLevel`, `Since`, `RawWarning`,
  `WarningSettings`, `WarnScope`, `SuppressionMap`, and the pure `gate()`.
- The **5 already-emitted codes** (`INTEGER_DIVISION`, `NARROWING_CONVERSION`, `UNSAFE_CALL_ARGUMENT`,
  `INFERENCE_ON_VARIANT`, the two `UNSAFE_*` accesses) are **re-routed** from baked-severity `emit(...)`
  to severity-free `warn(range, WarningCode::_, msg)`, gated in `type_diagnostics`.
- `parse_warning_settings` reads `[debug] gdscript/warnings/*` (never scanned today) + a tracked
  `warning_settings` query keyed on `ProjectConfig`; `build_suppression_map` + a `suppression_map` query
  keyed on the parse.
- **The mandatory query-recount test** (mirroring `queries.rs:1410`/`1515`): set a `project.godot`, run
  `type_diagnostics`, **edit a warning level**, assert `analyze_file`/`item_tree`/`infer` did **not**
  recompute (only `warning_settings` + the gate wrapper did). *This test is the proof the seam is in the
  right place; it is the gate on M0.*

Run **W2 M0 (the pure CFG + facts + reachability, golden-tested, checker untouched)** in lockstep so the
two new subsystems are validated independently before they couple at the W2→W1 handoff. Together these
two M0s de-risk the entire critical path: after them, W1's 35+ checks and W2's new constructs are
parallel breadth work, and W5/W6/W7 can proceed against known shapes.

---

## 6. Cross-check — conflicts, gaps, and consistency notes

The seven playbooks are **internally consistent on every load-bearing claim**; they were each grounded
against the same tree and they corroborate each other's corrections. The notable points:

### 6.1 Consistencies worth recording (the playbooks agree, and the agreement matters)

- **`Diagnostic.code` stays `String`.** W1 (keep `String`, `WarningCode` internal, `as_str()` bridge),
  W5 (`features.rs:37` emits `code: String` today), and W6 (`code: String` at `gdscript-base:79`, freeze
  the type *once* at the cut) all agree. W6 explicitly defers to W1 and recommends keeping `String` if
  W1 slips. **No conflict** — but note this **diverges from the canonical plan's §6.1 sketch**, which
  writes `pub code: WarningCode` on the POD. All three playbooks consciously override the plan; the
  override is correct (the plan's sketch would be a breaking POD change to the frozen `gdscript-base`
  contract). The synthesis endorses: **keep `code: String`.**
- **The contract is `gdscript-ide` + `gdscript-base`,** not `gdscript-ide` alone. W6 and W5 both carry
  this correction (the POD types live in `gdscript-base` and consumers `use` them directly). The
  canonical plan says only `gdscript-ide`. Endorse the playbooks.
- **The W2→W1 handoff is cleanly split:** W2 *produces* reachability + match-wildcard data and exposes
  `unreachable_stmts`/`unreachable_patterns`; W1 *owns emission + gating* of `UNREACHABLE_*`. Both
  playbooks describe the same seam (`emit_unsafe`, `infer.rs:~1459-1476`) and the same contract. Consistent.
- **The salsa-cacheability invariant** (warning-setting / format-config / flow edits must not invalidate
  `infer`) is stated identically and reinforced across W1 (gate downstream of `analyze_file`), W3
  (`FmtConfig` is an argument, not a salsa input), W2 (cache flow *inside* `analyze_file`, no new salsa
  entity), and W4 (a recount test confirming a warning-setting edit doesn't touch `infer`/`flow`). This
  is the phase's spine and all four respect it.

### 6.2 String-type naming: a real divergence the playbooks already caught

- **`Severity::Info` vs `Severity::Information`:** the canonical plan §6.1 writes `Information`; the
  **code says `Info`** and W6 flags this explicitly ("do not rename"). W2 and W1 emit through the POD and
  use the code's actual `Severity`. **No inter-playbook conflict** — but a synthesis-level reminder: any
  doc prose (W5) and policy text (W6) must say **`Info`**.
- **`EcoString` vs `SmolStr`:** the canonical plan's W2 sketch (and W3's Doc-IR sketch) write
  `EcoString`; **the workspace uses `smol_str::SmolStr`**. W2 corrects this prominently. **W3's playbook
  still shows `EcoString` in its `Doc` IR sketch** (`Doc::Text(EcoString)`, `Doc::Trivia(EcoString)`) —
  this is a **minor inconsistency to fix at implementation time**: W3 should use `SmolStr` (or a plain
  `String`/`Box<str>` for Doc text) to match the workspace convention, exactly as W2 did. *Flagged as a
  gap; not blocking.*

### 6.3 One latent bug in W1's `gate()` sketch (flag for implementation, not a plan conflict)

W1's `gate()` sketch version-gates master-only codes with
`if raw.code.since() == Since::Master && s.engine < (4,5) { return None; }`. The `(4,5)` "nearest
master-ish cut" is a **magic number that will rot** as bundled Godot versions advance (W6's matrix lists
"4.3, 4.4, 4.5, …newest at cut" and says *don't hardcode 4.5 — read it from the API*). **The two
playbooks are in tension on this exact constant:** W6 mandates reading the bundled version from
`gdscript_api::godot_version()`; W1's `since()` gate hardcodes the threshold. **Resolution for
implementation:** W1's version gate should compare against the **bundled-version-derived** "is this a
master-only-and-we're-on-a-stable-that-predates-it" predicate sourced the same way W6 surfaces it, not a
literal `(4,5)`. Low severity (it only mis-gates 2 codes), but it's a concrete correctness seam where W1
and W6 must agree on the source of truth.

### 6.4 Gaps (things no playbook fully owns)

- **`GET_NODE_DEFAULT_WITHOUT_ONREADY`** is "W1-adjacent, scene-dep" in W1 and isn't claimed by any
  other workstream. It needs the scene model (`gdscript-scene`, landed in Phase 4) + `@onready`
  reasoning. **No playbook gives it a concrete milestone.** It's correctly out of the W1 *machinery*
  cut, but someone must own the wiring or it silently won't ship. **Recommendation: fold it into W1 M3
  (the scene/W2-coordinated codes) as an explicit deliverable**, or explicitly defer it to post-1.0 on
  the roadmap. Right now it's in a seam between workstreams.
- **The Godot differential harness** appears in W1 (warning parity), W2 (the #93510 cases), and W3
  (gdformat parity is a *different* external tool, gdtoolkit, not Godot). All three correctly make it an
  **opt-in CI job requiring an external binary**, not part of default `xtask ci`. **No conflict, but no
  single owner** of the harness *infrastructure* — three workstreams each assume "a differential job
  exists." **Recommendation: W1 builds the Godot-differential harness scaffold** (it has the most codes
  to diff); W2 contributes its cases into it; W3's gdformat-differential is a separate, smaller harness
  it owns. Worth an explicit hand-off note so the harness isn't built three times.
- **`Analysis::format` in W6's review scope.** W6 notes `Analysis::format` is in the plan §2 sketch but
  *absent in code today* and says "if W3 slips past 1.0, `format` is added in a later minor (additive,
  fine)." W3 treats `Analysis::format` as a **1.0 deliverable that must land before the W6 freeze**.
  These aren't contradictory but they imply different schedules. **Resolution: W3's formatter API is a
  1.0 cut item (it's a named exit criterion); it must land before W6 M1.** The W6 "if it slips" clause is
  a fallback, not the plan. Sequence W3's M2 (the consumer wiring + `Analysis::format`) **before** W6 M1.

### 6.5 No blocking conflicts

There are **no contradictions that block starting work.** The divergences above are either (a) the
playbooks *jointly* overriding the canonical plan (and being right to), or (b) implementation-time
naming/constant fixes (`EcoString`→`SmolStr`, the `(4,5)` magic number, `Info` not `Information`), or
(c) ownership seams (`GET_NODE_DEFAULT_WITHOUT_ONREADY`, the differential harness, the `Analysis::format`
schedule) that this overview resolves with explicit recommendations. The phase is internally coherent and
ready to sequence per §2.

---

## 7. The one-screen summary

- **State:** all *functional* substrate ships at `0.2.1`; the Phase-6 delta (gate seam, CFG, formatter
  crate, perf infra, doc fill, `#[non_exhaustive]` freeze, governance polish) is almost entirely net-new.
- **Critical path:** `W2 CFG → W1 gate+codes → W6 freeze → W5 fill + contract → W7 consumer gate → 1.0`.
- **Start now, parallel:** W2 M0, **W1 M0 (the first milestone — §5)**, W3 (independent), W4 M0,
  W7 outreach.
- **W6 is last engineering** and **irreversible** — freeze only after every surface-reshaping edit lands.
- **Biggest risks:** W2 soundness (correctness) and W7 external-consumer (schedule).
- **Fix at implementation:** keep `Diagnostic.code: String`; `SmolStr` not `EcoString` (incl. W3's Doc
  IR); `Severity::Info` not `Information`; W1's `since()` gate reads the bundled version (W6's source),
  not a literal `(4,5)`; give `GET_NODE_DEFAULT_WITHOUT_ONREADY`, the Godot-differential harness, and the
  `Analysis::format` schedule explicit owners.
