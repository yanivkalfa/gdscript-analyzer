# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.5.0] - 2026-06-30

### Added

- **ide:** Rename a connected method/signal rewrites its scene [connection]s
- **ide:** Classify a scene node as a renameable symbol (GodotDef::SceneNode)
- **ide:** Classify the full scene-node reference cascade (parent=, connection, get_node)
- **ide:** Rename an @export var rewrites its scene property key (A3)
- **hir:** SHADOWED_GLOBAL_IDENTIFIER for locals, params, and members
- **hir:** ASSERT_ALWAYS_TRUE / ASSERT_ALWAYS_FALSE
- **hir:** UNTYPED_DECLARATION / INFERRED_DECLARATION (opt-in)
- **hir:** Resolve a non-* autoload via get_node("/root/Name")
- CONFUSABLE_IDENTIFIER + Unicode (UAX #31) identifier lexing
- **hir:** First-class item-tree annotations + annotation lifecycle checks
- **hir:** Precise read-vs-write UNUSED + UNUSED_PRIVATE_CLASS_VARIABLE
- **hir:** Union-type a node path across multi-scene attachments
- **hir:** Type override-children under an instanced sub-scene (Stage 4.23)
- **hir:** Type inner-class values + member access (Stage 4.24 inc.1)
- **hir:** Infer inner-class method bodies (Stage 4.24 inc.2)

### Changed

- **hir:** Single-source + CI-lock the classify/infer name-lookup order (Stage 4.20)

### Documentation

- Enforce missing_docs on 6 crates + internal-API banners (Stage 9b/9c)

### Fixed

- **hir:** Eliminate 3 TYPE_MISMATCH false-positive classes (corpus 55->1)
- **hir:** Type same-file enums (INFERENCE_ON_VARIANT corpus 33->11)
- **hir:** Keep inner-class navigation correct-or-refuse (Stage 4.24 inc.2 guard)



## [0.4.0] - 2026-06-28

### Added

- **hir:** W1 — member-kind misuse, base-class shadow, enum-without-default
- **hir:** W1 — STATIC_CALLED_ON_INSTANCE (conservative: typed local instance only)
- **hir:** W1 — enum-member-without-default + UNUSED_SIGNAL (file-level)
- **hir:** W1 — NATIVE_METHOD_OVERRIDE (conservative type-clash, engine-base)
- **cli,ide,db,hir:** W1 — --strict override + Phase-1 bug-hunt fixes
- **hir:** W2 — UNASSIGNED_VARIABLE via definite-assignment
- **hir:** W2 — UNREACHABLE_PATTERN (a match arm after a catch-all)



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


