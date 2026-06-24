//! Ad-hoc corpus runner: load every `.gd` file under a directory through the public
//! [`AnalysisHost`] / [`gdscript_ide::Analysis`] API and report its diagnostics (parse +
//! the Phase-2 §5 type diagnostics). It exercises the full salsa pipeline end to end
//! (`apply_change` → tracked `parse`/`analyze_file` → features), so it doubles as the
//! zero-behaviour-change regression check across the Phase-3 migrations: 0 panics and a
//! stable diagnostic count on a real project.
//!
//! `--project` loads every file into ONE host so the global `class_name` registry is populated
//! and cross-file references resolve — validating project-scale fidelity (M1+).
//!
//! Usage: `cargo run -p gdscript-ide --example corpus -- <dir> [--show] [--project]`

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

/// Project mode: load EVERY file into one host so the global `class_name` registry is populated
/// and cross-file references actually resolve. Validates that lighting up cross-file resolution
/// (M1+) introduces no project-scale false positives.
fn run_project(dir: &str, files: &[PathBuf], show: bool) {
    let root = Path::new(dir);
    let mut host = AnalysisHost::new();
    let mut change = Change::new();
    let mut loaded = Vec::new();
    for (i, path) in files.iter().enumerate() {
        if let Ok(src) = std::fs::read_to_string(path) {
            let id = FileId(u32::try_from(i).expect("< 4B files"));
            change.change_file(id, src.as_str());
            // The `res://` path = the file's path relative to the project root (the arg `dir`),
            // forward-slashed — so `preload("res://…")`/`extends "res://…"` resolve cross-file.
            let rel = path.strip_prefix(root).unwrap_or(path);
            let res_path = format!("res://{}", rel.to_string_lossy().replace('\\', "/"));
            change.set_file_path(id, res_path);
            loaded.push((id, path.clone(), src));
        }
    }
    host.apply_change(change);
    let analysis = host.analysis();

    let (mut clean, mut with_diags, mut total_diags) = (0usize, 0usize, 0usize);
    let mut panics = Vec::new();
    for (id, path, src) in &loaded {
        let id = *id;
        let snap = analysis.clone();
        let run = std::panic::AssertUnwindSafe(move || snap.diagnostics(id).unwrap());
        match std::panic::catch_unwind(run) {
            Ok(d) if d.is_empty() => clean += 1,
            Ok(d) => {
                with_diags += 1;
                total_diags += d.len();
                if show {
                    let idx = LineIndex::new(src);
                    println!("\n{}  ({} diag)", path.display(), d.len());
                    for diag in &d {
                        let lc = idx.line_col(diag.range.start);
                        let line_text = src.lines().nth(lc.line as usize).unwrap_or("");
                        println!(
                            "  {}:{}  [{}] {}",
                            lc.line + 1,
                            lc.col + 1,
                            diag.code,
                            diag.message
                        );
                        println!("      | {}", line_text.trim_end());
                    }
                }
            }
            Err(_) => panics.push(path.clone()),
        }
    }
    println!(
        "\n=== corpus (PROJECT mode — cross-file resolution active): {dir} ===\n  files:       {}\n  clean:       {clean}\n  with diags:  {with_diags} ({total_diags} diagnostics)\n  panics:      {}",
        loaded.len(),
        panics.len()
    );
    for p in &panics {
        println!("  PANIC: {}", p.display());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args
        .first()
        .cloned()
        .expect("usage: corpus <dir> [--show] [--project]");
    let show = args.iter().any(|a| a == "--show");
    let project = args.iter().any(|a| a == "--project");

    let mut files = Vec::new();
    collect_gd(Path::new(&dir), &mut files);
    files.sort();

    if project {
        run_project(&dir, &files, show);
        return;
    }

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
