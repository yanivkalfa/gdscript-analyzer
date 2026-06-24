//! Ad-hoc corpus runner: load every `.gd` file under a directory through the public
//! [`AnalysisHost`] / [`gdscript_ide::Analysis`] API and report its diagnostics (parse +
//! the Phase-2 §5 type diagnostics). It exercises the full salsa pipeline end to end
//! (`apply_change` → tracked `parse`/`analyze_file` → features), so it doubles as the
//! zero-behaviour-change regression check across the Phase-3 migrations: 0 panics and a
//! stable diagnostic count on a real project.
//!
//! Usage: `cargo run -p gdscript-ide --example corpus -- <dir> [--show]`

use std::path::{Path, PathBuf};

use gdscript_base::{FileId, LineIndex};
use gdscript_ide::{AnalysisHost, Change};

fn collect_gd(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, ".godot" | ".git" | "node_modules" | "out" | "target") {
                continue;
            }
            collect_gd(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("gd") {
            out.push(path);
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: corpus <dir> [--show]");
    let show = args.any(|a| a == "--show");

    let mut files = Vec::new();
    collect_gd(Path::new(&dir), &mut files);
    files.sort();

    let (mut total, mut clean, mut with_diags, mut total_diags) = (0usize, 0usize, 0usize, 0usize);
    let mut panics = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        total += 1;
        let src_for_parse = src.clone();
        let result = std::panic::catch_unwind(move || {
            let mut host = AnalysisHost::new();
            let mut change = Change::new();
            change.change_file(FileId(0), src_for_parse.as_str());
            host.apply_change(change);
            host.analysis().diagnostics(FileId(0)).unwrap()
        });
        match result {
            Ok(diags) if diags.is_empty() => clean += 1,
            Ok(diags) => {
                with_diags += 1;
                total_diags += diags.len();
                if show {
                    let idx = LineIndex::new(&src);
                    println!("\n{}  ({} diag)", path.display(), diags.len());
                    for d in &diags {
                        let lc = idx.line_col(d.range.start);
                        let line_text = src.lines().nth(lc.line as usize).unwrap_or("");
                        println!(
                            "  {}:{}  [{}] {}",
                            lc.line + 1,
                            lc.col + 1,
                            d.code,
                            d.message
                        );
                        println!("      | {}", line_text.trim_end());
                    }
                }
            }
            Err(_) => panics.push(path.clone()),
        }
    }

    println!(
        "\n=== corpus: {dir} ===\n  files:       {total}\n  clean:       {clean}\n  with diags:  {with_diags} ({total_diags} diagnostics)\n  panics:      {}",
        panics.len()
    );
    for p in &panics {
        println!("  PANIC: {}", p.display());
    }
}
