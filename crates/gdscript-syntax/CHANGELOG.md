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
- Phase 2 — single-file semantic analysis (types, diagnostics, IDE features- phase 2 implementation playbook) dev to prod ([#27](https://github.com/yanivkalfa/gdscript-analyzer/pull/27))
- **phase-3:** Cross-file resolution, navigation & bug-hunt hardening (M0–M5) ([#29](https://github.com/yanivkalfa/gdscript-analyzer/pull/29))

### Fixed

- **syntax:** Parser hardening v2 to master (keyword identifiers + lambda) ([#24](https://github.com/yanivkalfa/gdscript-analyzer/pull/24))


