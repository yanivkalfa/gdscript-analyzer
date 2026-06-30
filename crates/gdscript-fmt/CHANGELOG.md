# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(under Cargo's 0.x reading: while `0.x`, a breaking change bumps the minor and a
new feature is a patch).
## [0.5.1] - 2026-06-30



## [0.5.0] - 2026-06-30

### Added

- **fmt:** Intra-line spacing normalization (Phase 4 increment A)
- **fmt:** Blank-line policy (Phase 4 increment B)
- **fmt:** Blank-line insertion + boundary-comment indentation (Phase 4C)
- **fmt:** Preserve the source's line-ending style (Phase 4C)
- **fmt:** Length-driven line reflow (Phase 4C)
- **fmt:** Meaning-equivalence safety net + string-quote normalization (Phase 4)
- **fmt:** Magic trailing comma — exploded-with-comma reflow (Phase 4)
- **fmt:** Operator-chain wrapping (Phase 4)
- **fmt:** Enum-brace spacing (Phase 4)
- **fmt,ide,lsp:** Format_range — LSP range/selection formatting (Phase 4)
- **fmt:** Two-space inline-comment offset (Phase 4)
- **fmt:** Layout ownership — re-flow already-multi-line statements (Phase 4)
- **fmt:** Port gdformat's expression wrapper to a CST-driven engine (Phase 4)
- **fmt:** Dict-entry kv-pair wrapping + magic-comma chains explode leading-dot
- **fmt:** Split inline suite bodies (`if c: x` -> two lines), keep lambdas inline
- **fmt:** Split inline match-arm bodies + property bodies
- **fmt:** Split semicolon-separated statements; drop trailing `;`
- **fmt:** Strip redundant grouping parens (gdformat remove_outer_parentheses)
- **fmt:** Collapse backslash line continuations and re-wrap
- **fmt:** Remove a signal's empty parameter list (`signal s()` -> `signal s`)
- **fmt:** Collapse single-statement lambda bodies + fix the meaning net
- **fmt:** Split a long annotated declaration's annotation onto its own line
- **fmt:** Parse dict value-on-next-line; render lambdas in the CST wrapper
- **fmt:** Re-lay-out multi-line lambda-bearing statements
- **fmt:** Split an inner class's `extends` onto its own body line
- **fmt:** Thread block-level comments through a reshaped statement
- **fmt:** Thread comments through comma-lists (arrays / dicts / arg lists)
- **fmt:** Paren-wrap a multi-line-string operator chain, string content verbatim
- **fmt:** Place a lambda argument's separator before its trailing comment
- **fmt:** Thread standalone comments through an operator-chain wrap
- **fmt:** Thread a standalone comment that hangs off a lambda argument's body

### Changed

- **fmt:** Make the safety net a structural parse-tree comparison (Phase 4)

### Documentation

- **fmt:** DEVIATIONS.md (gdformat parity status) + TECH_DEBT W3 update
- **fmt:** Update DEVIATIONS parity numbers (exact ~91%/~60%, fixpoint ~99%/~98%) + new normalizations
- **fmt:** Update DEVIATIONS parity to ~95% (godot) / ~65% (ReactiveUI) byte-exact + new normalizations
- **fmt:** Update DEVIATIONS parity numbers to 96.7% godot / 93.3% ReactiveUI
- **fmt:** Update DEVIATIONS to 96.9%/94.4% + characterize the remaining tail (comments, multi-line strings, BOM)
- **fmt:** Update DEVIATIONS to 98.5%/97.8% — comment-threading + multi-line strings done
- **fmt:** Update DEVIATIONS to 99.6% godot / 100% ReactiveUI
- Enforce missing_docs on 6 crates + internal-API banners (Stage 9b/9c)

### Fixed

- **fmt:** Keep multi-line lambda-in-brackets bodies verbatim (Phase 4C-1)
- **fmt:** Match gdformat's column-0 / trailing-comment blank-line rules
- **fmt:** Unique-node `$%` spacing + single-line triple-quote normalization
- **fmt:** Wrap annotated declarations (`@onready var x = …`) via the CST path
- **fmt:** Preserve leading BOM + tight `.match(`/`.when(` member calls
- **fmt:** Annotation-prefixed defs force blanks only after a previous def
- **fmt:** Collapse every blank run to one before re-inserting def blanks
- **fmt:** Bottom-up a dot-chain that contains a lambda
- **fmt:** Expand inline blocks inside inner-class methods at the right depth
- **fmt:** Expand `;`-separated statements inside lambda bodies at the right depth
- **syntax:** Parse a lambda body that closes mid-line at an argument-separator comma
- **fmt:** Handle a nested multi-line bracket inside a lambda body, and an earlier-segment-lambda dot-chain
- **fmt:** Wrap a subscript-on-a-call chain leading-dot (`a.m(…)["k"]`)
- **fmt:** Strip a block-start blank + place a block-trailing comment at its block depth
- **fmt:** A trailing comment must not force a fitting statement to wrap
- **fmt:** A column-0 comment amid a function body forces no def blanks
- **fmt:** Explode an indexed collection literal, index on close line (Stage 6.29)
- **fmt:** Hoist a leading comment out of a paren operand → 456/456 gdformat (Stage 6.28)

### Style

- **fmt:** Satisfy clippy in expand_inline_blocks (sort_by_key, collapse if, drop borrows)



## [0.4.0] - 2026-06-28



## [0.3.0] - 2026-06-28

### Added

- **fmt:** W3 — gdscript-fmt formatter crate + Analysis::format

### Fixed

- **hir,fmt:** §1 bug-hunt — 5 confirmed Phase-6 defects


