//! Real-file robustness gate (Playbook §8.2): every vendored corpus fixture — real Godot
//! `.tscn`/`.tres` files extracted from godot-demo-projects + the target project — parses with **no
//! panic**, **no problems**, and a sane model. Broader robustness (the full demo-projects corpus)
//! is exercised ad hoc by `cargo run --example scene_corpus -- <dir>`.

use std::path::Path;

use gdscript_scene::{SceneKind, parse_scene};

#[test]
fn vendored_corpus_parses_cleanly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("the corpus fixture dir exists") {
        let path = entry.expect("a readable dir entry").path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "tscn" | "tres") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a readable fixture");
        let Ok(model) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_scene(&src)))
        else {
            panic!("parser PANICKED on {}", path.display())
        };
        count += 1;
        // Every vendored fixture is a real, well-formed file → zero problems.
        assert!(
            model.problems.is_empty(),
            "{}: {:?}",
            path.display(),
            model.problems
        );
        if model.kind == SceneKind::Scene {
            assert!(
                model.root.is_some(),
                "{}: a scene fixture must have a root",
                path.display()
            );
        }
        // (A node parented *into* an instanced/inherited sub-scene legitimately has
        // `parent_idx == None` — it's an override, not a dangling parent; the `problems.is_empty()`
        // check above already rules out genuine dangling parents.)
    }
    assert!(
        count >= 4,
        "expected the vendored corpus fixtures to be present, found {count}"
    );
}
