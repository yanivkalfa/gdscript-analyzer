# Quickstart

> **Status: forthcoming.** The analysis API lands in **Phase 1+** (see
> [`plans/ROADMAP.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/ROADMAP.md)). The sketch below shows the
> *intended shape* of the smallest possible "analyze a `.gd` string" program so
> the surface is fixed early. It does not compile against the Phase-0 stubs yet.

The library is built around two types modeled on rust-analyzer:

- **`AnalysisHost`** — the single mutable owner of analysis state, one per
  project. Its only mutation entry point is `apply_change`.
- **`Analysis`** — a cheap, cloneable, immutable snapshot you run read queries
  against. Every query is *cancellable*: a newer change cancels in-flight reads.

All inputs are a `FileId` plus **byte offsets**; all results are POD structs
(serde-serializable), with **no `lsp-types`** in the core. The client converts
byte offsets to UTF-16 and codes to its protocol.

## Analyze a single `.gd` string (forthcoming)

```rust,ignore
use gdscript_ide::{AnalysisHost, Change, FileId};

fn main() {
    // 1. Create a host and push a file's contents through a Change.
    //    The library never reads the filesystem itself — text is injected.
    let mut host = AnalysisHost::new();
    let file = FileId::from_raw(0);

    let source = r#"
extends Node

func _ready() -> void:
    var n := 1 + 1
    print(n)
"#;

    let mut change = Change::new();
    change.set_file_text(file, source.into());
    host.apply_change(change);

    // 2. Take an immutable snapshot and run read queries.
    let analysis = host.analysis();

    // Parse + type diagnostics for the file (POD; byte offsets).
    let diagnostics = analysis.diagnostics(file).unwrap();
    for d in &diagnostics {
        println!("{:?} @ {:?}: {}", d.severity, d.range, d.message);
    }

    // Document symbols (outline) for the file.
    let symbols = analysis.document_symbols(file).unwrap();
    println!("{} top-level symbols", symbols.len());
}
```

The same `Analysis` snapshot exposes `completions`, `hover`, `goto_definition`,
`find_references`, `rename`, `signature_help`, `semantic_tokens`,
`inlay_hints`, and more — one method per IDE feature, each returning POD. See
[Consuming from Rust](../consume/rust.md) for the full surface, and
[`plans/01-ARCHITECTURE.md`](https://github.com/yanivkalfa/gdscript-analyzer/blob/master/plans/01-ARCHITECTURE.md) §2 for the
authoritative API sketch.

## What works today (Phase 0)

Right now the repository builds, tests, lints, and produces empty napi + wasm
binding stubs — proving the toolchain end to end. The first *real* feature set
(parse diagnostics, document symbols, folding, by-name completion) arrives in
**Phase 1**; inference-backed hover and completion in **Phase 2** (the MVP).
