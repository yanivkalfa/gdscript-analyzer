# Consuming from Rust

> **Status:** the API surface is fixed here; the implementation lands in
> **Phase 1+** ([`plans/ROADMAP.md`](../../../plans/ROADMAP.md)).

Native Rust consumers depend on a single crate, **`gdscript-ide`**:

```bash
cargo add gdscript-ide
```

This is the crate we semver most carefully — it is the contract every other
consumer (the napi addon, the wasm package, the LSP server, the CLI) is built
on top of.

## The `AnalysisHost` / `Analysis` model

```rust,ignore
/// The single mutable owner of analysis state. One per project/workspace.
pub struct AnalysisHost { /* ... */ }

impl AnalysisHost {
    pub fn new() -> Self;
    /// The ONLY mutation entry point: apply a batch of input changes
    /// (file added/edited/removed, project config, Godot version, scenes).
    pub fn apply_change(&mut self, change: Change);
    /// A cheap, cloneable, immutable, `Send` snapshot for read queries.
    pub fn analysis(&self) -> Analysis;
}

/// An immutable snapshot. Every query is cancellable.
pub struct Analysis { /* ... */ }
```

`Analysis` exposes one method per IDE feature, each taking a `FileId` +
**byte offsets** and returning POD wrapped in `Cancellable<T>`:

```rust,ignore
analysis.diagnostics(file)?;        // Vec<Diagnostic>
analysis.completions(pos)?;         // Vec<CompletionItem>
analysis.hover(pos)?;               // Option<HoverResult>
analysis.signature_help(pos)?;      // Option<SignatureHelp>
analysis.goto_definition(pos)?;     // Vec<NavTarget>
analysis.find_references(pos)?;     // Vec<Reference>
analysis.rename(pos, "new_name")?;  // Result<SourceChange, RenameError>
analysis.document_symbols(file)?;   // Vec<DocumentSymbol>
analysis.workspace_symbols("q")?;   // Vec<NavTarget>
analysis.semantic_tokens(file)?;    // SemanticTokens
analysis.inlay_hints(file)?;        // Vec<InlayHint>
analysis.format(file)?;             // Option<SourceChange>
```

## The rules of the surface

- **Inputs are injected.** The host owns a virtual file system; you push text
  via `apply_change`. The library **never** touches `std::fs` — this is what
  keeps it portable to WASM (see [the portability rules in
  `plans/01-ARCHITECTURE.md`](../../../plans/01-ARCHITECTURE.md) §7).
- **Outputs are POD.** A `Diagnostic` carries a byte `TextRange`, a code
  (e.g. `GDSCRIPT_UNSAFE_CALL`), a severity, a message, and optional fixes —
  never an `lsp_types::Diagnostic`. You convert at your boundary.
- **Cancellation.** Reads return `Cancellable<T>`; a concurrent `apply_change`
  cancels in-flight reads (at Tier 2 this is salsa's cancellation). Re-issue.

## Why a separate crate stack

The crates are layered so each depends only *downward*
(`base → syntax → api/db → hir → ide`). Most consumers only ever name
`gdscript-ide`; the lower crates are an implementation detail you can reach for
if you are building specialized tooling. The full layering is in
[Crate layout](../contributing/crates.md) and
[ADR-0001](../adr/0001-rust-library-not-server.md).
