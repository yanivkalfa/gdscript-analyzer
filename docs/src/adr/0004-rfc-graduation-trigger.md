# ADR-0004: A lightweight RFC process, graduating from issues only when needed

- **Status:** Accepted
- **Date:** 2026-06-28

## Context

As the analyzer approaches 1.0 (Phase 6), it acquires a **frozen public contract**
(`gdscript-ide` + the re-exported `gdscript-base` PODs + the FFI JSON — see the
Workstream-6 API-stabilization playbook) and an outside audience. Two pressures appear:

1. **Some changes are now consequential enough to deserve a written, reviewable
   decision before code** — a new public type, a breaking change to a result struct, a
   new warning that diverges from the engine, a change to the warning-gating semantics, or
   anything that touches the 1.0 contract.
2. **But the project is small**, and a heavyweight RFC repo + formal comment-period
   machinery (à la rust-lang/rfcs) would be pure overhead at this scale — most changes are
   still fine to land straight from a well-described issue + PR.

We already have: numbered issue forms (`01-bug-report`, `02-feature-or-diagnostic`,
`03-proposal`), an ADR mechanism (this directory), and labels-as-code (`.github/labels.yml`,
including `meta-rfc`). The question is *when* a change must escalate from "an issue + a PR"
to "a written proposal + an ADR", and how that escalation is triggered **observably** rather
than by gut feel.

## Decision

We will run a **two-tier, issue-based RFC process** — no separate `rfcs` repo, no formal
FCP — with an explicit, observable graduation trigger:

- **Tier 1 (default): an issue + PR.** Most changes. The `03-proposal` form captures intent;
  review happens on the PR. No ADR required.
- **Tier 2 (RFC): a proposal issue labeled `meta-rfc`, resolved by an ADR in this directory.**
  A change **must** graduate to Tier 2 when *any* of these objective triggers holds:
  - it **adds to or changes the frozen 1.0 public surface** (a new public type/enum/field, or
    a breaking change to one) — i.e. anything `cargo-semver-checks` would flag as
    minor/major post-1.0;
  - it **adds or changes a warning's default level / identity**, or introduces an
    **intentional divergence from the engine checker** (label `godot-divergence`);
  - it **changes a cross-cutting policy**: the semver policy, the warning-gating model, the
    supported-Godot-version matrix, or the MSRV.

  The graduation is **recorded**: the proposal issue gets `meta-rfc`, and the decision lands
  as a numbered ADR (`Accepted`/`Rejected`) that the PR links. The ADR — not a comment thread
  — is the durable record.

Rejected alternatives: a dedicated `rfcs` repository with a formal final-comment-period and a
steering committee (too heavy for the project's size — revisit if contributor volume grows);
and "no process, ADRs at author discretion" (the status quo — rejected because, post-1.0, the
contract changes are exactly the ones that must not be decided implicitly in a PR diff).

## Consequences

- **Easier:** contributors get a bright line for *when* a written proposal is required (the
  three triggers), and reviewers can point to it. The 1.0 contract can't drift via an
  un-discussed PR — a surface change without a linked ADR is a review stop.
- **Easier:** the audit trail is uniform — every consequential decision is a numbered ADR,
  discoverable in one directory, not scattered across issue comments.
- **Harder:** a little more ceremony for the subset of changes that hit a trigger (write the
  proposal, get the ADR merged). Mitigated by keeping Tier 1 the default — the overhead
  applies only to genuinely contract-affecting work.
- **Follow-on:** the `meta-rfc` label and the `03-proposal` issue form are the entry points;
  Workstream 6's PR-time `cargo-semver-checks` gate is the *mechanical* backstop that catches
  a surface change whose author forgot to graduate it (the gate fails → the PR must add an ADR
  + a deliberate version bump).
