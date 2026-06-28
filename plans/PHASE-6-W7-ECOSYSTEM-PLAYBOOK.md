# Phase 6 · Workstream 7 — Ecosystem Maturity Playbook

> The lightest, mostly-non-code workstream of v1.0: graduate governance from
> "founder + ADRs" toward a 1.0 community project, **only as far as volume
> warrants** — and clear the one criterion adoption (not engineering) gates:
> **≥1 external consumer**.
>
> **Parents:** [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) §Workstream 7,
> [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) §6 (the v1.0 exit bar — the
> *"≥1 external consumer beyond guitkx and our own LSP/CLI"* line).
> **Primary evidence:** [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md)
> §6 (ADR-vs-RFC, triage labels, issue forms), §6.3 (the rust-analyzer label set).
>
> **Verified against the tree 2026-06-27.** This is a governance workstream, so
> "the code" is `.github/`, the root community-health files, and `docs/src/adr/`.
> Several plan claims **already ship** — this playbook corrects the plan where the
> repo has diverged (notably: ADRs live at `docs/src/adr/`, not the `docs/adr/`
> research/07 §6.1 still cites; the diagnostic-grade bug form already exists; and
> there is a **stale duplicate issue-template set** to delete).

---

## 0. Thesis

Six of the seven v1.0 workstreams are engineering. This one is **process +
adoption**, and the honest reading of the plan is: *most of the governance
scaffolding already exists from Phase 0* — the job at 1.0 is to (a) **finish and
de-duplicate** what's there, (b) **provision the triage taxonomy as code** so it
survives a label rename, (c) **write down the RFC-graduation trigger as an
observable threshold** (not a vibe), and (d) treat **≥1 external consumer** as a
tracked **release blocker**, because it is the *only* exit criterion that
engineering alone cannot satisfy. Keep governance sized to a solo-maintainer
project: *"RFCs are a tool, not a burden"* ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2).

---

## 1. Goal — the 1.0 cut vs the deferred tail

### What ships at 1.0 (this workstream)

1. **The RFC-graduation policy, with a *documented trigger*.** Stay on
   lightweight in-repo ADRs as the default; document the *observable* condition
   under which we stand up a separate `rfcs` repo + FCP — not "when volume
   warrants" in the abstract, but a concrete, checkable trigger (§3.1).
2. **The full rust-analyzer-style triage taxonomy, provisioned as code.** The
   `C-*` / `E-*` / `S-*` / `good-first-issue` label set ([`research/07`](research/07-ecosystem-and-release-tooling.md)
   §6.3) defined in a committed `.github/labels.yml` and synced by an Action — so
   the labels referenced by the issue forms actually exist in the repo and can't
   drift (§3.2).
3. **A dedicated `diagnostic` issue form.** A warning-parity report distinct from
   a generic bug: snippet + expected-vs-actual diagnostic + Godot version +
   analyzer version + crate-vs-npm + OS, labeled `C-diagnostic` (§3.3). *(The
   generic bug form already collects most of this — §2 — so this is a
   specialization, not a from-scratch build.)*
4. **A public post-1.0 roadmap.** The plan's "Post-1.0 outlook" surfaced as a
   GitHub Projects board (or a pinned `meta: roadmap` issue) + a `ROADMAP`-linked
   docs page, so consumers see what's next and what's *deliberately deferred*
   (§3.4).
5. **The ≥1-external-consumer plan, tracked as a release blocker.** The
   formatter-as-second-consumer, the standalone-LSP-answers-#11056 outreach, the
   "add a client" guide, and direct outreach — each a checklist item on a
   release-blocker tracking issue (§3.5).
6. **Status de-staling.** `CONTRIBUTING.md`/`SUPPORT.md` still say *"Phase 0…
   no analyzer features yet"*; the 1.0 cut flips these to 1.0 reality (§3.6).

### Deferred past 1.0 (do NOT build now)

| Deferred | Why |
|---|---|
| A separate `rfcs` repo + FCP machinery | Only when the §3.1 trigger fires — premature process is the documented failure mode ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2). |
| A steering committee / voting / shared-maintainer model | `GOVERNANCE.md` already defers this to "an ADR when outgrown"; 1.0 stays BDFL-ish. |
| Automated triage bots, stale-bot, auto-labelers beyond the basic label-sync | Solo-maintainer overhead; revisit if issue volume forces it. |
| A bespoke community site / forum beyond GitHub Discussions | Discussions already wired in `config.yml`; sufficient at 1.0. |

---

## 2. Current state — what EXISTS today vs the gap

Phase 0 over-delivered here. The accurate inventory (paths verified):

| Asset | Status | Path | Gap for 1.0 |
|---|---|---|---|
| Code of Conduct | **Done** | `CODE_OF_CONDUCT.md` (Covenant 3.0, contact filled) | none |
| Contributing guide | **Done, stale status** | `CONTRIBUTING.md` | header says *"Phase 0… no analyzer features yet"* (line 12-14) — de-stale (§3.6) |
| Governance doc | **Done** | `GOVERNANCE.md` | ADR-default + RFC-deferred policy present; **trigger is vague** ("volume warrants") — make it observable (§3.1) |
| Security policy | **Done** | `SECURITY.md` | supported-versions table is `0.x`-only; revise the window at 1.0 (cross-ref Workstream 6 contract page) |
| Support doc | **Done, stale status** | `SUPPORT.md` | same "Phase 0" staleness (line 5-7) — de-stale (§3.6) |
| ADR mechanism | **Done** | `docs/src/adr/` — `README.md`, `template.md`, ADR-0001…0003 (Accepted), indexed in `SUMMARY.md` | path is `docs/src/adr/`, **not** `docs/adr/` (research/07 §6.1 + Phase-6 plan §5 both say `docs/adr/` — stale); ADRs work as-is |
| Issue forms — **canonical set** | **Done** | `.github/ISSUE_TEMPLATE/{01-bug-report,02-feature-or-diagnostic,03-proposal}.yml` + `config.yml` | `01-bug-report.yml` **already** collects snippet/expected/actual/Godot-version/analyzer-version/consumed-via/OS and labels `C-bug,S-needs-triage` — the plan's "diagnostic form" requirement is ~80% met by this |
| Issue forms — **stale duplicate set** | **DELETE** | `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml` | added by PR #21; flat `bug`/`enhancement` labels, weaker fields; **two bug forms confuse the chooser and split labels** (§3.3 step 0) |
| PR template | **Done** | `.github/PULL_REQUEST_TEMPLATE.md` | spot-check the changeset/ADR checklist still matches 1.0 |
| Dependabot | **Done** | `.github/dependabot.yml` (cargo + npm + actions) | none |
| Discussions wired | **Done** | `config.yml` `contact_links` → Discussions + docs site | none |
| **Labels as code** | **MISSING** | — | labels exist only as inline strings inside the issue forms (`C-bug`, `S-needs-triage`, `C-enhancement`, `C-architecture`, `S-needs-design`); **never provisioned in the repo**, so applying a form to a repo without those labels silently fails to label. No `E-*`, no `good-first-issue`, no `C-diagnostic`, no `S-actionable/needs-repro/needs-info` (§3.2) |
| **Public roadmap board** | **MISSING** | — | only prose: `plans/ROADMAP.md` + Phase-6 §"Post-1.0 outlook". No Projects board / pinned roadmap issue / docs roadmap page (§3.4) |
| **External-consumer tracking** | **MISSING** | — | no release-blocker issue/board; the criterion is in the plan's exit checklist but nothing *operationalizes* it (§3.5) |
| **RFC-trigger threshold** | **MISSING (as a number)** | `GOVERNANCE.md` §"RFCs (deferred)" | the *policy* is written; the *trigger* is not observable (§3.1) |

**Net:** the community-health files and the canonical issue forms are done and
good. The four genuine gaps are **labels-as-code**, the **roadmap board**, the
**external-consumer release-blocker**, and turning two vague prose statements
(the RFC trigger; the stale "Phase 0" status) into concrete artifacts — plus a
**one-line cleanup** (delete the duplicate template set). This is hours of work,
not weeks; the long pole is **adoption** (§3.5), which is calendar-bound, not
effort-bound — hence "track it early."

---

## 3. Design — concrete artifacts to add or correct

Idioms to match: the existing forms are GitHub **YAML issue forms** with
`labels: [...]` arrays; `config.yml` sets `blank_issues_enabled: false`; ADRs are
Nygard 5-section markdown numbered `NNNN-kebab.md` and indexed in both
`docs/src/adr/README.md` and `docs/src/SUMMARY.md`. Stay inside those idioms.

### 3.1 ADR-default / graduate-to-RFC, with an *observable* trigger

`GOVERNANCE.md` already states the policy (ADRs now; RFC repo deferred). The gap
is that the trigger — *"once sustained contributor volume makes the extra
structure worthwhile"* — is unfalsifiable. Replace it with a checkable threshold
and record the *decision to defer* as an ADR so the bar is explicit.

**Trigger (edit into `GOVERNANCE.md` §"RFCs (deferred)"):**

> We graduate from ADRs to a separate `rfcs` repo with a Final Comment Period
> when **all** of the following hold for **two consecutive release cycles**:
> (1) ≥5 *distinct external* contributors have merged non-trivial PRs;
> (2) ≥3 user-facing design proposals (the `proposal` issue form) were opened
> that an ADR-in-the-governing-PR could not comfortably absorb — i.e. they needed
> broad written review *before* any code; and (3) a maintainer team of ≥2 exists
> to run the FCP. Until then ADRs + the `proposal` form + the `S-needs-design`
> label are the process. The graduation itself lands as an ADR.

These numbers are deliberately low-but-nonzero: they make "are we there yet?" a
*lookup* (contributor count, proposal count, maintainer count), not a debate.
They are illustrative — the load-bearing change is that the trigger is
**countable**, not the exact integers. Mirror the trigger into the post-1.0
roadmap (§3.4) under "deferred."

**Add `ADR-0004: ADR-default governance; RFC process deferred (with trigger).`**
Status `Accepted`. Context = the plan's "RFCs are a tool, not a burden"
([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2) + solo-maintainer reality; Decision = the trigger
above; Consequences = cheap now, with a written escape hatch. Copy
`docs/src/adr/template.md`, add to `docs/src/adr/README.md` index **and**
`docs/src/SUMMARY.md` (both, per the ADR README process step 2). This ADR also
records the v1.0 **public-API freeze** cross-reference to Workstream 6 if not
already captured there.

### 3.2 Triage labels as code (the real gap)

The forms reference labels that don't exist as repo objects. Provision the full
[`research/07`](research/07-ecosystem-and-release-tooling.md) §6.3 taxonomy in a committed file and sync it with an Action so
it's reproducible and reviewable.

**Add `.github/labels.yml`** (consumed by `EndBug/label-sync` or
`micnncim/action-label-syncer`):

```yaml
# Category (C-*) — what kind of issue
- { name: "C-bug",            color: "d73a4a", description: "Wrong diagnostic, hover, completion, crash, or other incorrect behavior" }
- { name: "C-diagnostic",     color: "e11d48", description: "Warning/lint parity with Godot: a warning we mis-emit, miss, or word differently" }
- { name: "C-enhancement",    color: "a2eeef", description: "New feature or improvement" }
- { name: "C-architecture",   color: "5319e7", description: "Crate graph, public API, data model, FFI/WASM strategy — routes to an ADR" }
- { name: "C-perf",           color: "fbca04", description: "Performance / incrementality regression or opportunity" }
- { name: "C-docs",           color: "0075ca", description: "Documentation, the guide, or examples" }
# Effort (E-*) — onboarding signal
- { name: "E-easy",           color: "c2e0c6", description: "Small, well-scoped change" }
- { name: "E-medium",         color: "bfd4f2", description: "Moderate change" }
- { name: "E-hard",           color: "d4c5f9", description: "Deep change touching core invariants" }
- { name: "good-first-issue", color: "7057ff", description: "A good entry point for a new contributor" }
# State (S-*) — triage status (exactly one at a time)
- { name: "S-needs-triage",   color: "ededed", description: "Awaiting maintainer triage" }
- { name: "S-needs-repro",    color: "fef2c0", description: "Needs a minimal reproduction" }
- { name: "S-needs-info",     color: "fef2c0", description: "Needs more information from the reporter" }
- { name: "S-needs-design",   color: "fef2c0", description: "Needs a design decision / ADR before implementation" }
- { name: "S-actionable",     color: "0e8a16", description: "Triaged, reproducible, ready to be worked" }
- { name: "S-blocked",        color: "b60205", description: "Blocked on another change or upstream Godot" }
# Cross-cutting
- { name: "godot-sync",       color: "1d76db", description: "Driven by the automated Godot extension_api.json sync" }
- { name: "meta",             color: "ededed", description: "Project process, governance, roadmap" }
- { name: "release-blocker",  color: "b60205", description: "Must be resolved before the next tagged release" }
```

**Add `.github/workflows/labels.yml`** — on push to `master` touching
`.github/labels.yml` (+ `workflow_dispatch`), run the label-syncer with
`permissions: issues: write`. This is the same "config-as-code, Action-applies-it"
discipline the repo already uses for Godot-sync. Keep `prune: false` initially so
it never deletes a label a human added.

**Reconcile the forms:** the canonical forms (`01/02/03-*.yml`) already emit
`C-bug,S-needs-triage` / `C-enhancement,S-needs-triage` / `C-architecture,S-needs-design`
— all now backed by real labels. No form edit needed beyond §3.3.

### 3.3 The `diagnostic` issue form

**Step 0 (do first — cleanup):** delete the **stale duplicate set**
`.github/ISSUE_TEMPLATE/bug_report.yml` and `.github/ISSUE_TEMPLATE/feature_request.yml`.
They use flat `bug`/`enhancement` labels (outside the taxonomy), collect weaker
fields, and produce a confusing *two-bug-reports* chooser alongside the canonical
`01-bug-report.yml`. The numbered set + `config.yml` is canonical (added by the
Phase-0 scaffold; the duplicates came later in PR #21 and were never reconciled).

**Then add `.github/ISSUE_TEMPLATE/04-diagnostic.yml`** — a *warning-parity*
report, distinct from a generic bug, so Godot-vs-analyzer discrepancies are
first-class and land on the `C-diagnostic` queue. It is a near-clone of
`01-bug-report.yml` (reuse its field set verbatim where it fits) with two
parity-specific additions:

```yaml
name: Diagnostic / warning parity
description: A warning we emit (or miss, or word differently) vs. what Godot reports.
title: "diagnostic: "
labels: ["C-diagnostic", "S-needs-triage"]
body:
  - type: markdown
    attributes:
      value: |
        Use this when our **diagnostic output** disagrees with Godot's — a
        warning we shouldn't emit, one we miss, a wrong severity, or a message
        that doesn't match the engine. (For crashes / wrong hover / completion,
        use the Bug report form instead.)
  - type: input
    id: warning-code
    attributes:
      label: Warning code (if known)
      description: e.g. UNSAFE_METHOD_ACCESS / unsafe_method_access. Leave blank if unsure.
      placeholder: "UNSAFE_METHOD_ACCESS"
  - type: textarea
    id: snippet
    attributes: { label: GDScript snippet, description: "Minimal `.gd` that shows the discrepancy.", render: gdscript }
    validations: { required: true }
  - type: textarea
    id: godot-says
    attributes: { label: What Godot reports, description: "The engine's diagnostic (message + line), or 'nothing'." }
    validations: { required: true }
  - type: textarea
    id: analyzer-says
    attributes: { label: What gdscript-analyzer reports, description: "Our diagnostic (message + span), or 'nothing'." }
    validations: { required: true }
  - type: input
    id: godot-version
    attributes: { label: Godot version, placeholder: "4.4.1-stable" }
    validations: { required: true }
  - type: input
    id: analyzer-version
    attributes: { label: Analyzer version, placeholder: "1.0.0" }
    validations: { required: true }
  - type: dropdown
    id: consumed-via
    attributes:
      label: Consumed via
      options: ["Rust crate (crates.io)", "npm package (@gdscript-analyzer/*)", "LSP server", "wasm / browser", "CLI", "Other / unsure"]
    validations: { required: true }
  - type: dropdown
    id: os
    attributes: { label: Operating system, options: ["Linux", "macOS", "Windows", "Other"] }
    validations: { required: true }
  - type: checkboxes
    id: known-divergence
    attributes:
      label: Is this an expected divergence?
      description: We *intentionally* suppress UNSAFE_PROPERTY_ACCESS / UNSAFE_METHOD_ACCESS inside proven `is`/`as`/`!= null` guards (Godot #93510). Check if you've confirmed it isn't one of those.
      options:
        - label: "I checked the Warning Reference and this is not a documented intentional divergence."
```

That last checkbox is the W7-specific cross-link to **Workstream 1/5**: the
documented `#93510` divergence is the most likely false-positive parity report,
so the form pre-empts it. (`02-feature-or-diagnostic.yml` stays — it's for
*proposing new* diagnostics; this `04` form is for *parity bugs in existing* ones.)

### 3.4 The public post-1.0 roadmap

Two surfaces, single source of truth = the Phase-6 plan's "Post-1.0 outlook":

1. **A docs page** `docs/src/guide/roadmap.md` (add to `SUMMARY.md` under User
   Guide) that restates: the multi-year **narrowing tail**, GDScript 3.x as
   demand-gated, deeper refactorings + call hierarchy, more language bindings
   (PyO3/C-ABI), parallelism (native-only), and the **RFC-repo graduation
   trigger** (§3.1). Frame it as *"1.0 is a floor, not a ceiling; here's what's
   deliberately deferred and why"* — matching the plan's honest tone.
2. **A GitHub Projects board** (or, lighter, a pinned `meta: roadmap` issue) with
   columns *Now (1.0) / Next / Later / Deferred*, seeded from that list, linked
   from the docs page and the README. Prefer the pinned issue if board upkeep
   would outpace a solo maintainer — the plan says "Projects board *or* pinned
   issue."

Cross-link the README "Roadmap" line (currently → `plans/ROADMAP.md`) to also
point at the published roadmap page so consumers (not just contributors) see it.

### 3.5 The ≥1-external-consumer plan (the release blocker)

This is the criterion that **engineering cannot close** — it requires an actual
outside adopter — so it is tracked as a **release blocker**, not an afterthought
([`PHASE-6`](PHASE-6-V1-RELEASE.md) §Exit criteria; [`00`](00-VISION-AND-SCOPE.md) §6).

**Add a tracking issue** `meta: v1.0 release blocker — ≥1 external consumer`,
labeled `meta,release-blocker`, pinned, with this checklist (each item is a
concrete avenue, ordered by likelihood):

- [ ] **Formatter as the natural 2nd consumer.** guitkx range-formats embedded
  GDScript inside `.guitkx` `{expr}`/hook blocks via the Workstream-3 formatter
  (CST + byte-range → `SourceChange`) through its existing Volar-style source-map
  adapter — *instead of* shelling out to Python `gdformat`. This is the
  lowest-friction win because guitkx is already in-house and already a client of
  the crate. **Caveat for honest accounting:** guitkx is named in the exit
  criterion as something the external consumer must be *"beyond"* — so the
  formatter satisfying guitkx is necessary plumbing but does **not by itself**
  clear the bar. It does, however, make the *crate* a proven formatter host that
  a third party can adopt.
- [ ] **Standalone LSP answers documented demand (#11056).** Announce
  `gdscript-lsp` to the Godot community as the externalized LSP that engine
  proposal [#11056](https://github.com/godotengine/godot-proposals/issues/11056)
  asked for (no running editor; semantic tokens, inlay hints, rename — features
  the engine LSP lacks, per [`00`](00-VISION-AND-SCOPE.md) §4). Target: a Neovim/Helix/Zed
  user wiring it as their GDScript server = an external consumer of the
  *published binary/crate*.
- [ ] **"Add a client" guide lowers integration cost.** Ship the rust-analyzer
  *Other Editors*-modeled page (Workstream 5 / [`research/07`](research/07-ecosystem-and-release-tooling.md) §5.4): finding
  the binary on `PATH`, transport, `initializationOptions` schema, advertised
  capabilities, tracing — plus a docs.rs cross-link for embedding the crate
  directly. Add a **"powered by gdscript-analyzer"** badge snippet to the README
  for adopters to display.
- [ ] **Direct outreach.** Open issues / PRs against ≥3 existing GDScript-tooling
  projects (an alternate editor extension, a CI-lint project, a playground)
  proposing they adopt the CLI/crate/wasm package; log responses here.
- [ ] **Confirm + cite the consumer.** When an external project ships using a
  published package/crate, link it here and check the exit-criteria box in
  [`PHASE-6`](PHASE-6-V1-RELEASE.md).

**Definition of "external consumer" (write it in the issue, avoid goalpost
drift):** a project **not maintained by us** that depends on a *published*
artifact (`cargo add gdscript-ide`, `npm i @gdscript-analyzer/{core,wasm}`, or the
released `gdscript-lsp` binary) in its committed manifest/config — guitkx and our
own `gdscript-lsp`/`gdscript-cli` explicitly excluded.

### 3.6 De-stale the status banners

At the 1.0 cut, three docs still describe "Phase 0":

- `CONTRIBUTING.md` lines 12-14 (the `> Project status: Phase 0…` blockquote).
- `SUPPORT.md` lines 5-7 (same).
- `docs/src/adr/README.md` note that ADRs "distill decisions already settled" is
  fine; leave it.

Replace the Phase-0 banners with a 1.0 status line ("v1.0 — stable, semver'd; see
the contract page") and re-point any `plans/ROADMAP.md`-only links that consumers
hit toward the published docs. Trivial edits; easy to forget — hence the explicit
list. (Workstream 6 owns the contract page these now link to.)

---

## 4. Step-by-step implementation plan

Ordered cheapest-first; every step is independently mergeable and none blocks the
engineering workstreams.

1. **Cleanup (1 PR).** Delete `.github/ISSUE_TEMPLATE/bug_report.yml` and
   `feature_request.yml`. Confirm the chooser now shows exactly Bug /
   Feature-or-diagnostic / Design-proposal + the two `config.yml` contact links.
2. **Labels as code (1 PR).** Add `.github/labels.yml` (§3.2) +
   `.github/workflows/labels.yml` (sync Action, `prune: false`,
   `issues: write`). Run it once via `workflow_dispatch`; verify every label the
   forms reference now exists in the repo.
3. **Diagnostic form (1 PR).** Add `04-diagnostic.yml` (§3.3). Re-test the
   chooser; confirm a filed diagnostic lands with `C-diagnostic,S-needs-triage`.
4. **RFC trigger + ADR-0004 (1 PR).** Edit `GOVERNANCE.md` §"RFCs (deferred)"
   with the observable trigger (§3.1); add `docs/src/adr/0004-*.md`; index it in
   `docs/src/adr/README.md` **and** `docs/src/SUMMARY.md`. *(Architecturally
   consequential per the ADR README → it correctly lands as its own ADR.)*
5. **Public roadmap (1 PR).** Add `docs/src/guide/roadmap.md` (§3.4) → `SUMMARY.md`;
   create the Projects board *or* pinned `meta: roadmap` issue; link both from the
   README "Roadmap" line.
6. **External-consumer blocker (issue, not a PR).** Open the pinned
   `release-blocker` tracking issue (§3.5) with the checklist + the
   external-consumer definition. Begin the outreach items in parallel with the
   rest of Phase 6 (long lead time).
7. **De-stale banners (1 PR, late).** Flip the Phase-0 status lines in
   `CONTRIBUTING.md`/`SUPPORT.md` to 1.0 (§3.6) — do this **at the cut**, once
   Workstream 6's contract page exists to link to.

Steps 1-5 + 7 are doc/config PRs (hours total). Step 6 is the calendar risk and
must start **first** despite producing no code.

---

## 5. Test / verification plan

Governance has no golden corpus; "tests" are mechanical checks + CI guards.

1. **Issue-form lint.** Add a CI job (or reuse the docs job) running a YAML issue
   forms validator (e.g. GitHub's schema via `actions/github-script` or a
   `js-yaml` parse + required-keys assertion) over `.github/ISSUE_TEMPLATE/*.yml`,
   so a malformed form fails the PR rather than silently breaking the chooser.
2. **Label-reference integrity.** A tiny CI check (script or `xtask`) that every
   `labels:` value across all issue forms **and** every `release-blocker` /
   `good-first-issue` string the project relies on **exists in
   `.github/labels.yml`** — the canonical guard against the "form references a
   nonexistent label" failure that motivated §3.2. This is the one genuinely
   automatable correctness test in the workstream.
3. **Link-check.** The existing `mdbook-linkcheck` (docs CI, [`research/07`](research/07-ecosystem-and-release-tooling.md)
   §5.2) covers the new `roadmap.md` and ADR-0004 links once they're in
   `SUMMARY.md` — assert it stays green.
4. **ADR index consistency.** Extend any existing ADR check (or add one) that the
   set of `docs/src/adr/NNNN-*.md` files equals the rows in
   `docs/src/adr/README.md` *and* the entries in `SUMMARY.md` — so ADR-0004 can't
   be half-added.
5. **Manual chooser smoke test.** After steps 1+3, open *New issue* on the repo
   and eyeball: 4 forms (bug / feature-or-diagnostic / diagnostic / proposal) +
   2 contact links, no duplicate bug form, no "blank issue" escape hatch
   (`blank_issues_enabled: false`).
6. **External-consumer audit (the exit gate).** Before tagging 1.0, the
   release-blocker issue's "Confirm + cite the consumer" box must be checked with
   a real, linked, *published-artifact* dependency from a project we don't own —
   this is the human verification of [`00`](00-VISION-AND-SCOPE.md) §6.

There is no property/differential/fuzz testing here — those belong to the
engineering workstreams (1-4). Tests 1, 2, and 4 are the only ones worth wiring
into CI; the rest are pre-release manual checklist items.

---

## 6. Risks & mitigations

| Risk | Sev | Mitigation |
|---|---|---|
| **The ≥1-external-consumer criterion stalls — adoption isn't in our control.** | **Critical** (it can block the *tag*) | Track as a pinned `release-blocker` from day one (§3.5); make adoption cheap (the "add a client" guide, the badge, the formatter as a turnkey 2nd consumer, the LSP answering #11056); run *direct outreach* to ≥3 tooling authors rather than waiting. Define "external consumer" precisely up front to avoid goalpost drift. |
| **Duplicate / drifting issue templates** (already present). | Med | §3.3 step 0 deletes the stale set; the §5.2 label-integrity check + §5.1 form lint stop the next drift; one canonical numbered set. |
| **Labels referenced but not provisioned** (current state — forms label with strings the repo may lack). | Med | `.github/labels.yml` + sync Action (§3.2) make labels reproducible; the §5.2 CI check fails a PR that references a missing label. |
| **Premature governance ossifies / burns out a solo maintainer.** | Med | Hard ceiling: ADRs stay the default; the RFC repo is gated behind an *observable, multi-cycle* trigger (§3.1, ADR-0004); no bots/committee at 1.0; *"a tool, not a burden"* ([`research/07`](research/07-ecosystem-and-release-tooling.md) §6.2). |
| **Stale "Phase 0" status misleads new users at 1.0.** | Low | §3.6 explicit de-stale step, done at the cut against Workstream 6's contract page. |
| **Roadmap board rots** (boards need upkeep). | Low | Allow the lighter pinned-issue form; single source of truth = the plan's Post-1.0 outlook, so the board/issue is a *view*, not a second list to maintain. |
| **A flood of false "parity" diagnostic reports** (the #93510 intentional divergence). | Low | The `04-diagnostic.yml` "Is this an expected divergence?" checkbox + the Warning-Reference cross-link (§3.3) pre-empt the common false positive; `C-diagnostic` queue keeps them sortable. |

---

## 7. Dependencies on other workstreams

This workstream **consumes** outputs of others and **gates** the tag; it produces
nothing they depend on.

- **Workstream 3 (Formatter) → us.** The formatter is the designated low-friction
  *second consumer* (§3.5). Its CST + byte-range → `SourceChange` shape is what
  lets guitkx range-format embedded GDScript and what a third party can adopt.
  The external-consumer checklist's first item can't fully resolve until the
  formatter lands.
- **Workstream 5 (Docs) → us.** The **"add a client" guide**, the
  **Warning Reference** (which the `04-diagnostic.yml` divergence checkbox links
  to), and the **playground-as-live-docs** are the adoption-enablers behind §3.5.
  The roadmap docs page (§3.4) lives in the same mdBook and rides its
  `mdbook test` / linkcheck CI.
- **Workstream 6 (API stabilization) → us.** The **contract page** (semver policy
  + supported-Godot matrix) is what the de-staled `CONTRIBUTING`/`SUPPORT`/
  `SECURITY` status lines point at (§3.6); ADR-0004 (§3.1) sits beside the
  API-freeze ADR work. `SECURITY.md`'s supported-versions table is revised in
  lockstep with the 1.0 cut.
- **Workstream 1 (Warning set) → the diagnostic form.** The `C-diagnostic` queue
  and the #93510 expected-divergence checkbox presuppose the full 48-warning set
  + the documented divergences exist (Workstream 1 §1.5).
- **All workstreams → the exit checklist.** This workstream owns operationalizing
  the **≥1-external-consumer** and governance lines of [`PHASE-6`](PHASE-6-V1-RELEASE.md)
  §Exit criteria; it does not gate any engineering work, but the
  external-consumer item gates the **1.0 tag itself**.

---

## References

- [`PHASE-6-V1-RELEASE.md`](PHASE-6-V1-RELEASE.md) — §Workstream 7 (this doc grounds it), §Exit criteria (≥1-external-consumer line), §Post-1.0 outlook (the roadmap source).
- [`00-VISION-AND-SCOPE.md`](00-VISION-AND-SCOPE.md) — §6 v1.0 bar (the external-consumer criterion), §4 consumers (guitkx / standalone LSP / CLI / playground / community).
- [`research/07-ecosystem-and-release-tooling.md`](research/07-ecosystem-and-release-tooling.md) — §6 governance (ADR-vs-RFC, file list), §6.2 "RFCs are a tool, not a burden", §6.3 the rust-analyzer label taxonomy, §5.4 the "add a client" page.
- **In-repo (verified):** `GOVERNANCE.md`, `CONTRIBUTING.md`, `SUPPORT.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`; `.github/ISSUE_TEMPLATE/{01-bug-report,02-feature-or-diagnostic,03-proposal,config}.yml` (canonical) + `{bug_report,feature_request}.yml` (stale, to delete); `docs/src/adr/{README,template,0001..0003}.md` + `docs/src/SUMMARY.md`.
