# ADR-0002: Hand-written parser, tree-sitter as oracle

- **Status:** Accepted
- **Date:** 2026-06-22

## Context

A "Roslyn for Godot" needs a parser it fully controls: lossless (every byte,
including comments and whitespace, recoverable from the tree), error-recovering
(an IDE parses broken code on every keystroke), and able to produce precise
diagnostics. GDScript adds a specific hazard — Python-like **significant
indentation** — that must be handled deliberately. See
[`plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §6 and the
parsing-strategy research it cites.

There is an existing grammar, **tree-sitter-gdscript** (MIT, mature, with Rust
and WASM bindings). It is tempting to adopt it as *the* parser. But tree-sitter
has properties that disqualify it as the grammar-of-record for this project: it
is effectively single-maintainer, is manually synced to Godot (so it lags the
engine), models comments lossily, and gives us limited control over error
recovery and diagnostic quality. At the same time, throwing it away entirely
would forfeit a valuable, independent reference implementation we could test
against.

The forces in tension: **ship something parsing quickly** vs. **own the grammar
long-term**, and **avoid reinventing a grammar** vs. **not depending on an
external grammar we can't steer**.

## Decision

**We will own a hand-written, lossless, error-recovering recursive-descent
parser, and use tree-sitter-gdscript only as a bootstrap and a permanent
differential test oracle — never as the grammar-of-record.**

- **End state:** a hand-written recursive-descent parser producing a **`cstree`**
  CST (chosen over `rowan` for `Send + Sync` + interning, which suits our
  concurrency), with a typed AST layer on top. The lexer is `logos`-based, with
  a **hand-written indentation pre-pass** that injects INDENT/DEDENT/NEWLINE
  (indent stack + bracket-depth counter to suppress significance inside
  `()[]{}`, backslash line-continuation, tab/space rules), isolating the
  significant-indentation risk into one tested module.
- **A `Parser` trait** sits in front of the implementation. For a week-1 MVP we
  *may* wrap tree-sitter-gdscript behind that trait to get something parsing
  immediately, then swap in our hand-written backend.
- **tree-sitter is demoted to a permanent oracle:** we run differential tests
  comparing our trees against tree-sitter's on a large corpus. It is the
  reference grammar and the regression net — *never* the grammar we ship.

## Consequences

**Easier / positive.**

- Full control over grammar, error recovery, losslessness, and diagnostic
  messages — the prerequisites for an IDE-grade analyzer and for matching
  Godot's own warning messages later.
- `cstree` gives us a concurrent-friendly, interned CST that fits the
  `AnalysisHost`/`Analysis` snapshot model.
- The `Parser` trait lets Phase 1 start producing real output (document symbols,
  folding) via the tree-sitter backend *before* the hand-written parser is
  finished — value-earliest, with a safe migration path.
- Keeping tree-sitter as a differential oracle gives us an independent,
  continuously-run correctness check that catches grammar regressions for the
  life of the project.

**Harder / negative — the constraints this creates.**

- Writing and maintaining a hand-written parser with good error recovery is
  substantial, ongoing work (this is essentially all of Phase 1).
- The indentation pre-pass is subtle and must be thoroughly fixture-tested; it is
  the highest-risk part of the lexer.
- We must keep the tree-sitter dependency (and its attribution — the verbatim
  `Copyright (c) 2016 Max Brunsfeld` line in `THIRD-PARTY-NOTICES.md`) for as
  long as it serves as the oracle, even though it is not shipped as the parser.
- Differential testing requires reconciling two trees with different shapes,
  which itself needs a normalization layer.
