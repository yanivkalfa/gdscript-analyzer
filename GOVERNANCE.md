# Governance

This document describes how decisions are made in `gdscript-analyzer`. It is
deliberately **lightweight**: the project is an early `0.x` community effort, and
the governance is sized to match. It will grow as the contributor base grows.

## Guiding principle

Govern as little as necessary. Process exists to keep the project healthy and
decisions transparent — not for its own sake. Heavier mechanisms (a formal RFC
process, a steering committee, voting) are **deferred until contributor volume
warrants them**, at which point they will be introduced via an ADR.

## Roles

### Maintainer (current model: single lead maintainer)

At this stage the project has a single lead maintainer (a "benevolent dictator"
arrangement, BDFL-ish), who has final say on direction, merges, and releases.
This is appropriate for a young project and is explicitly a *starting point*, not
a permanent structure.

The maintainer is responsible for:

- Reviewing and merging pull requests.
- Cutting releases (see [`CONTRIBUTING.md`](CONTRIBUTING.md) and the release
  tooling).
- Maintaining the architectural direction recorded in the ADRs and the
  [`plans/`](plans/) documents.
- Upholding the [Code of Conduct](CODE_OF_CONDUCT.md).

### Contributors

Anyone who opens an issue or PR is a contributor. Contributions of all kinds —
code, tests, docs, triage, reviews — are valued equally and recognized.

## How decisions are made

Most decisions are made through ordinary discussion on issues and pull requests
and need no ceremony. The bar scales with consequence:

- **Routine changes** (bug fixes, docs, tests, dependency bumps): decided in the
  PR. A maintainer review and a green `cargo xtask ci` are sufficient.
- **Architecturally consequential changes** (public API contracts, the crate
  dependency graph, a major new dependency, MSRV bumps, portability/WASM
  strategy, the engine-sync model): require a **numbered Architecture Decision
  Record (ADR)**.

### The ADR process

ADRs are the project's primary decision-recording mechanism. They live under
[`docs/src/adr/`](docs/src/adr/) in Nygard format (Title / Status / Context /
Decision / Consequences) and are numbered sequentially.

1. Propose the change — open a design proposal issue (or a draft PR) describing
   the context and options.
2. Add a numbered ADR (copy `template.md`) in the same PR as the change it
   governs, or ahead of it if the change is large.
3. Discussion happens on the PR; the maintainer records the outcome by setting
   the ADR's **Status** (`Proposed` → `Accepted` / `Rejected`) and merging.
4. A superseded decision is not deleted — a new ADR supersedes it and the old
   one's Status is updated to point at the replacement.

The three foundational decisions (Rust + library-not-server; hand-written parser
with tree-sitter as oracle; napi-rs v3 dual-target binding) are recorded as
ADR-0001 through ADR-0003.

## RFCs (deferred)

A separate, heavier RFC process — for large cross-cutting proposals that warrant
broad written review before any code — is intentionally **not** adopted yet. The
ADR mechanism above covers current needs. An RFC process will be introduced (and
described here, via its own ADR) once sustained contributor volume makes the
extra structure worthwhile.

## Becoming a maintainer

There is no bureaucratic application. Maintainership is earned through a
sustained track record of high-quality contributions and good judgment in
reviews and discussions. When someone has demonstrated that consistently, the
lead maintainer may invite them to become a maintainer, with commit and release
responsibilities. As the project grows beyond a single maintainer, this section
will be expanded (via an ADR) to describe shared decision-making, tie-breaking,
and the path to emeritus status.

## Changing this document

Changes to governance are themselves architecturally consequential and follow
the ADR process. Until the structures above are outgrown, simplicity wins.
