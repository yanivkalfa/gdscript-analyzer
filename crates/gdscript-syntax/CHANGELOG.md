# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.5.0] - 2026-06-30

### Added

- **fmt:** Parse dict value-on-next-line; render lambdas in the CST wrapper
- **parser:** Disambiguate a statement-initial bare match identifier
- CONFUSABLE_IDENTIFIER + Unicode (UAX #31) identifier lexing
- **parser:** Tighten property get/set accessor parsing

### Documentation

- Enforce missing_docs on 6 crates + internal-API banners (Stage 9b/9c)

### Fixed

- **syntax:** Parse `$%Unique` / `$A/%Unique` node paths as get-node expressions
- **syntax:** Parse a lambda body that closes mid-line at an argument-separator comma
- **fmt:** Handle a nested multi-line bracket inside a lambda body, and an earlier-segment-lambda dot-chain
- **hir:** Eliminate 3 TYPE_MISMATCH false-positive classes (corpus 55->1)



## [0.4.0] - 2026-06-28



## [0.3.0] - 2026-06-28



## [0.2.1] - 2026-06-27

### Added

- **hir:** Recover soft-keyword-named symbols (match/when) at the AST layer

### Fixed

- **parser:** Handle 3 real GDScript forms — 307 -> 0 syntax errors on godot-demo-projects

### Style

- Apply rustfmt to the Phase-5 hardening changes



## [0.2.0] - 2026-06-27

### Documentation

- Client-facing READMEs + accurate v0.1 package metadata


