# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.3.0] - 2026-06-28

### Added

- **hir:** W1 M0 — the warning emit-then-gate seam
- **hir:** W2 M0 — the per-body control-flow narrowing dataflow
- **hir:** W2 M1 — drive the checker's narrowing from the flow facts
- **hir:** W2 M2 — short-circuit narrowing + the cross-construct wins
- **hir:** W1 M1 — a self-contained warning-check subset
- **hir:** W1 M2 — @warning_ignore[_start|_restore] suppression
- **hir:** W1 — SHADOWED_VARIABLE for a local shadowing a param/member

### Documentation

- W5 — generated Warning Reference (docgen + anti-drift) + Configuration

### Fixed

- **hir:** W2 soundness — invalidate self-members on a call in a guard condition
- **hir,fmt:** §1 bug-hunt — 5 confirmed Phase-6 defects



## [0.2.1] - 2026-06-27

### Added

- **hir:** Recover soft-keyword-named symbols (match/when) at the AST layer
- **hir:** Parse project.godot engine version (config/features) + engine_version query
- **hir:** Type node paths that descend INTO an instanced sub-scene ($Enemy/Sprite)

### Style

- Apply rustfmt to the Phase-5 hardening changes



## [0.2.0] - 2026-06-27

### Added

- **hir:** Emit UNSAFE_CALL_ARGUMENT for unsafe-cast call arguments (Phase-2 MVP)
- **hir:** Anchor relative preload/extends paths to the importing file's dir
- **hir:** Type *-autoload scene roots via the recorded script_class=/resource_type shortcut
- **hir:** Warn on class_name that shadows a global identifier (W2)
- **hir:** Bounded fixpoint for member field type seeding (W2-MEMBER-FIXPOINT)
- **hir:** Emit CYCLIC_INHERITANCE on genuine extends cycles (D7)
- **hir:** Await recovers the coroutine call's return type (identity on a non-signal operand)
- **hir:** Resolve cross-file preload-const member access (the offset-free firewall path)

### Documentation

- Client-facing READMEs + accurate v0.1 package metadata

### Fixed

- **hir:** Satisfy clippy on the Wave-2 commits (clone_from, needless borrows, collapsible if)
- **hir:** Route extends "res://x.gd".Inner to the seam, not the outer script


