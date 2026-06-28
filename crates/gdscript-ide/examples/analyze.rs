//! A minimal end-to-end tour of the `gdscript-ide` public API — the same surface an editor /
//! the `gdscript` CLI drives. Run it with:
//!
//! ```text
//! cargo run -p gdscript-ide --example analyze
//! ```
//!
//! The model is rust-analyzer's: one mutable [`AnalysisHost`] owns the project state; you push
//! file text through [`Change`] + [`AnalysisHost::apply_change`], then take cheap, `Send`,
//! read-only [`Analysis`] snapshots to answer queries. A concurrent edit cancels in-flight reads
//! (here there are none, so every `Cancellable` is `Ok`).

use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{AnalysisHost, Change};

fn main() {
    // A small script with: an integer-division warning, a typed binding to hover, and
    // four-space indentation the formatter will normalize to tabs.
    let src = "\
extends Node

func ratio() -> int:
    var halves := 10 / 4
    return halves
";

    // 1. Build the host and push the file (FileId is the caller's stable handle; the CLI/LSP keep
    //    their own path<->FileId map). `set_file_path` gives it a `res://` path so cross-file
    //    resolution (class_name / preload / autoloads) would work in a multi-file project.
    let file = FileId(0);
    let mut host = AnalysisHost::new();
    let mut change = Change::new();
    change.change_file(file, src);
    change.set_file_path(file, "res://ratio.gd");
    host.apply_change(change);

    // 2. Take a read-only snapshot and answer queries against it.
    let analysis = host.analysis();

    // Diagnostics: POD `Diagnostic { range, severity, code, message }` with byte offsets.
    println!("diagnostics:");
    for d in analysis.diagnostics(file).expect("not cancelled") {
        println!(
            "  [{:?}] {} ({}..{}) — {}",
            d.severity, d.code, d.range.start, d.range.end, d.message
        );
    }

    // Hover: the inferred type of the symbol under a byte offset.
    let offset = u32::try_from(src.find("halves :=").expect("present")).expect("fits u32");
    if let Some(h) = analysis
        .hover(FilePosition { file, offset })
        .expect("not cancelled")
    {
        println!(
            "\nhover @ `halves`: {}",
            h.ty_label.as_deref().unwrap_or("<none>")
        );
    }

    // Document symbols: the file's outline (funcs, vars, classes …).
    println!("\nsymbols:");
    for sym in analysis.document_symbols(file).expect("not cancelled") {
        println!("  {:?} {}", sym.kind, sym.name);
    }

    // Format: `None` when already tidy, else the reformatted text (here: tabs for the 4 spaces).
    if let Some(formatted) = analysis.format(file).expect("not cancelled") {
        println!("\nformatted:\n{formatted}");
    }
}
