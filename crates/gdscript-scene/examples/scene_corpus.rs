//! Ad-hoc corpus runner for the M0 `.tscn`/`.tres` parser — parses every scene/resource under a
//! directory and reports panics + a problem summary (the robustness gate, Playbook §8.2).
//!
//! Usage: `cargo run -p gdscript-scene --example scene_corpus -- <dir> [--show]`

use std::path::{Path, PathBuf};

use gdscript_scene::parse_scene;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, ".godot" | ".git" | "target" | "node_modules") {
                continue;
            }
            collect(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("tscn" | "tres")
        ) {
            out.push(path);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args
        .first()
        .cloned()
        .expect("usage: scene_corpus <dir> [--show]");
    let show = args.iter().any(|a| a == "--show");

    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    files.sort();

    let (mut clean, mut with_problems, mut total_problems, mut nodes_total) = (0usize, 0, 0, 0);
    let mut panics = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue; // non-UTF-8 / unreadable — not a text scene
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_scene(&src))) {
            Ok(m) => {
                nodes_total += m.nodes.len();
                if m.problems.is_empty() {
                    clean += 1;
                } else {
                    with_problems += 1;
                    total_problems += m.problems.len();
                    if show {
                        println!("\n{}  ({} problems)", path.display(), m.problems.len());
                        for p in &m.problems {
                            println!("  {p:?}");
                        }
                    }
                }
            }
            Err(_) => panics.push(path.clone()),
        }
    }

    println!(
        "\n=== scene corpus: {dir} ===\n  files:        {}\n  clean:        {clean}\n  with problems: {with_problems} ({total_problems} problems)\n  nodes parsed:  {nodes_total}\n  panics:        {}",
        files.len(),
        panics.len()
    );
    for p in &panics {
        println!("  PANIC: {}", p.display());
    }
}
