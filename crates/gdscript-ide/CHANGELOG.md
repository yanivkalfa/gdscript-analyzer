# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.1.0] - 2026-06-26

### Added

- **phase-0:** Scaffold workspace, tooling, CI, and Godot-sync
- Phase 1 - parser & syntax MVP - dev to master ([#18](https://github.com/yanivkalfa/gdscript-analyzer/pull/18))
- Prun and templates - pp - dev to master ([#21](https://github.com/yanivkalfa/gdscript-analyzer/pull/21))
- Phase 2 — single-file semantic analysis (types, diagnostics, IDE features) ([#26](https://github.com/yanivkalfa/gdscript-analyzer/pull/26))
- **phase-3:** Cross-file resolution, navigation & bug-hunt hardening (M0–M5) ([#28](https://github.com/yanivkalfa/gdscript-analyzer/pull/28))
- **hir,scene:** Phase-4 M1 — scene-aware node-path typing (the killer feature)
- **hir,ide,scene:** Phase-4 M2 — scene-aware diagnostics & navigation
- **wasm,db,ide:** Wasm binding over gdscript-session + runtime engine-API injection

### Fixed

- **hir,ide,scene:** Phase-4 M1-M3 hunt fixes — %-segment node paths + completion guard
- **db:** Post-wasm hunt — salsa-invalidate the runtime engine model (critical)


