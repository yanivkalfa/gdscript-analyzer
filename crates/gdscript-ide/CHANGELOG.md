# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.5.1] - 2026-06-30



## [0.5.0] - 2026-06-30

### Added

- **fmt,ide,lsp:** Format_range — LSP range/selection formatting (Phase 4)
- **ide:** Rename a connected method/signal rewrites its scene [connection]s
- **ide:** Classify a scene node as a renameable symbol (GodotDef::SceneNode)
- **ide:** Classify the full scene-node reference cascade (parent=, connection, get_node)
- **ide:** Rename a scene node, rewriting every reference (Half B)
- **ide:** Rename an @export var rewrites its scene property key (A3)
- **hir:** Resolve a non-* autoload via get_node("/root/Name")
- **corpus:** Faithful --per-project mode + wire into the corpus gate
- **hir:** Union-type a node path across multi-scene attachments
- **api,ide:** Hover docs — vendor Godot doc XML, BBCode→Markdown, native-only blob

### Changed

- **hir:** Single-source + CI-lock the classify/infer name-lookup order (Stage 4.20)
- **ide:** Measure project-wide find-refs — index not warranted (Stage 4.21)
- **ide:** Add project-scale cold bench; salsa-LRU not warranted (Stage 7)

### Documentation

- Enforce missing_docs on 6 crates + internal-API banners (Stage 9b/9c)

### Fixed

- **hir:** Keep inner-class navigation correct-or-refuse (Stage 4.24 inc.2 guard)



## [0.4.0] - 2026-06-28

### Added

- **cli,ide,db,hir:** W1 — --strict override + Phase-1 bug-hunt fixes



## [0.3.0] - 2026-06-28

### Added

- **hir:** W1 M0 — the warning emit-then-gate seam
- **hir:** W1 M1 — a self-contained warning-check subset
- **hir:** W1 M2 — @warning_ignore[_start|_restore] suppression
- **fmt:** W3 — gdscript-fmt formatter crate + Analysis::format

### Changed

- **ide:** W4 — warm-keystroke incremental re-analysis benchmark

### Documentation

- **w5,tech-debt:** Public-API example + record the §1 hardening pass



## [0.2.1] - 2026-06-27

### Added

- **hir:** Recover soft-keyword-named symbols (match/when) at the AST layer
- **ide:** Scope-aware by-name completion (indentation-based enclosing scope)
- **ide:** %Unique node-name completion (token-context disambiguated from modulo)
- **ide:** Derive ReferenceKind::Write in find-references + a 2nd classify/infer guard

### Fixed

- **ide:** Scope-aware completion hid params of lambdas/accessors/inline funcs (bug-hunt)

### Style

- Apply rustfmt to the Phase-5 hardening changes



## [0.2.0] - 2026-06-27

### Added

- **hir:** Anchor relative preload/extends paths to the importing file's dir

### Documentation

- Client-facing READMEs + accurate v0.1 package metadata


