# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.1.0] - 2026-06-26

### Added

- **phase-0:** Scaffold workspace, tooling, CI, and Godot-sync
- **phase-3:** Cross-file resolution, navigation & bug-hunt hardening (M0–M5) ([#28](https://github.com/yanivkalfa/gdscript-analyzer/pull/28))
- **wasm,db,ide:** Wasm binding over gdscript-session + runtime engine-API injection

### Fixed

- **db:** Post-wasm hunt — salsa-invalidate the runtime engine model (critical)


