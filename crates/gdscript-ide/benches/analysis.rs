//! Phase-2 single-file analysis benchmarks (Playbook §5.2): full diagnostics, hover, and
//! member completion on a ~300-line typed `.gd` file. Targets: cold < 50 ms, warm < 5 ms,
//! member-completion < 5 ms warm.
//!
//! The bundled engine model is `Arc`-shared and deserialized once on first use, so it is
//! warmed before the timed loops and excluded from per-file timing (Playbook §1.2). There is no
//! salsa cache in Phase 2, so each query re-parses and re-infers — "warm" here means the engine
//! model and allocator caches are hot, which is the realistic steady-state cost.

use criterion::{Criterion, criterion_group, criterion_main};
use std::fmt::Write as _;
use std::hint::black_box;

use gdscript_base::{FileId, FilePosition};
use gdscript_ide::{Analysis, AnalysisHost, Change};

/// ~300 lines of representative, mostly-typed GDScript.
fn sample_script() -> String {
    let mut s = String::from(
        "extends Node\nclass_name Sample\n\nvar count := 0\nvar label_text: String = \"hi\"\nvar items: Array[int] = []\n\n",
    );
    for i in 0..36 {
        let _ = write!(
            s,
            "func method_{i}(a: int, b: Vector2, node: Node) -> int:\n\
             \tvar v := a + count\n\
             \tvar p := b.x\n\
             \tvar parent := node.get_parent()\n\
             \tif v > 0 and node is Node2D:\n\
             \t\tcount += 1\n\
             \t\tnode.position = b\n\
             \tfor j in v:\n\
             \t\tcount += j\n\
             \tvar c := v if p > 0.0 else -v\n\
             \treturn c\n\n"
        );
    }
    s
}

fn analysis_for(src: &str) -> (Analysis, FileId) {
    let mut host = AnalysisHost::new();
    let file = FileId(0);
    let mut change = Change::new();
    change.change_file(file, src);
    host.apply_change(change);
    (host.analysis(), file)
}

fn bench(c: &mut Criterion) {
    let src = sample_script();
    let (analysis, file) = analysis_for(&src);

    // Warm the bundled engine model (excluded from per-file timing).
    let _ = analysis.diagnostics(file).unwrap();

    c.bench_function("diagnostics_~300loc", |b| {
        b.iter(|| black_box(analysis.diagnostics(black_box(file)).unwrap()));
    });

    // Hover on the `b.x` member access in the first method.
    let hover_off = u32::try_from(src.find("b.x").unwrap() + 2).unwrap();
    c.bench_function("hover", |b| {
        b.iter(|| {
            black_box(
                analysis
                    .hover(black_box(FilePosition {
                        file,
                        offset: hover_off,
                    }))
                    .unwrap(),
            )
        });
    });

    // Member completion right after `node.` — find a `node.get_parent()` site.
    let dot = src.find("node.get_parent").unwrap() + "node.".len();
    let comp_off = u32::try_from(dot).unwrap();
    c.bench_function("member_completion", |b| {
        b.iter(|| {
            black_box(
                analysis
                    .completions(black_box(FilePosition {
                        file,
                        offset: comp_off,
                    }))
                    .unwrap(),
            )
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
