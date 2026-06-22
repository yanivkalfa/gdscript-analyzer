# Architecture Decision Records

An **Architecture Decision Record (ADR)** captures a single architecturally
consequential decision — the context that forced it, the decision itself, and
the consequences that follow — as a short, immutable, numbered document. The
format here is [Michael Nygard's](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions):
**Title / Status / Context / Decision / Consequences**. See the
[template](./template.md).

ADRs are how we make the *why* behind the architecture durable. Code shows what
we did; an ADR explains why we did it, what we considered, and what we gave up —
so future contributors don't relitigate settled questions or accidentally
violate an invariant without knowing it was deliberate.

## The index

| ADR | Title | Status |
|---|---|---|
| [0001](./0001-rust-library-not-server.md) | Rust + library-not-server | Accepted |
| [0002](./0002-handwritten-parser-treesitter-oracle.md) | Hand-written parser, tree-sitter as oracle | Accepted |
| [0003](./0003-napi-rs-v3-dual-target.md) | napi-rs v3 dual-target binding | Accepted |

## The process

1. **When.** Any decision that constrains the architecture — a crate boundary, a
   dependency with reach (parser, binding, incremental engine), a portability
   rule, the public API contract, the versioning model — lands as an ADR.
   Reversible, local choices do not need one.
2. **How.** Copy [`template.md`](./template.md) to the next number
   (`NNNN-short-kebab-title.md`), fill in Context / Decision / Consequences, add
   it to the index above and to `SUMMARY.md`, and submit it **in the same PR**
   as the change it justifies.
3. **Status lifecycle.** `Proposed` → `Accepted` (merged) → later possibly
   `Deprecated` or `Superseded by ADR-NNNN`. ADRs are **append-only**: you don't
   rewrite history, you supersede it with a new record that links back.
4. **Source.** The three seeded ADRs distill decisions already settled in
   [`plans/00-VISION-AND-SCOPE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/00-VISION-AND-SCOPE.md) and
   [`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md).
