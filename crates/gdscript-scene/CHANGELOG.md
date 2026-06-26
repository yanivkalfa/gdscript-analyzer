# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.1.0] - 2026-06-26

### Added

- **phase-0:** Scaffold workspace, tooling, CI, and Godot-sync
- **scene:** Phase-4 M0 — the .tscn/.tres parser (gdscript-scene)
- **hir,scene:** Phase-4 M1 — scene-aware node-path typing (the killer feature)
- **hir,ide,scene:** Phase-4 M2 — scene-aware diagnostics & navigation

### Fixed

- **scene:** M0 bug-hunt fixes — escape vs dangling, first-wins, inherited-root gating
- **hir,ide,scene:** Phase-4 M1-M3 hunt fixes — %-segment node paths + completion guard


