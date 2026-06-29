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

/// Warm-keystroke incremental re-analysis (Phase-6 W4): after a cold analysis, a one-token edit to
/// a single body should re-run only that file's `analyze_file` (flow + gate included), not the
/// cross-file firewalled queries. This is the load-bearing incrementality the salsa graph + the W1
/// gating seam (downstream of `analyze_file`) + the W2 flow pass (inside `analyze_file`) must keep
/// fast. Target: flat, well under the cold cost.
fn bench_keystroke(c: &mut Criterion) {
    let file = FileId(0);
    // Two bodies differing by one token — toggling them is a realistic single-keystroke edit to a
    // field initializer (a body change, not a signature change → the cross-file firewall holds).
    let a = sample_script();
    let b_src = a.replacen("var count := 0", "var count := 1", 1);

    let mut host = AnalysisHost::new();
    let mut change = Change::new();
    change.change_file(file, a.as_str());
    host.apply_change(change);
    let _ = host.analysis().diagnostics(file).unwrap(); // warm the engine model + first analysis

    let mut toggle = false;
    c.bench_function("warm_keystroke_diagnostics_~300loc", |bencher| {
        bencher.iter(|| {
            let mut ch = Change::new();
            ch.change_file(file, if toggle { b_src.as_str() } else { a.as_str() });
            toggle = !toggle;
            host.apply_change(ch);
            black_box(host.analysis().diagnostics(black_box(file)).unwrap());
        });
    });
}

/// Project-wide find-references cost (`TECH_DEBT` Stage 4.21 measurement): a `class_name` + a method
/// referenced in EVERY file of a 150-file project — the worst case a precise referrer reverse-index
/// would optimize. find-refs word-boundary pre-filters then re-classifies each hit; this measures
/// whether that re-classify cost is a real regression (→ build the index) or negligible (→ the
/// pre-filter already suffices). Native-only (the engine model is required for cross-file resolve).
fn bench_find_references(c: &mut Criterion) {
    const N: u32 = 150;
    const BODY: &str =
        "extends Node\nfunc use_it() -> int:\n\tvar s := Shared.new()\n\treturn s.ping()\n";
    let base = "class_name Shared\nfunc ping() -> int:\n\treturn 1\n";

    let mut host = AnalysisHost::new();
    let mut change = Change::new();
    change.change_file(FileId(0), base);
    change.set_file_path(FileId(0), "res://shared.gd");
    for i in 1..=N {
        change.change_file(FileId(i), BODY);
        change.set_file_path(FileId(i), format!("res://f{i}.gd"));
    }
    host.apply_change(change);
    let analysis = host.analysis();
    let _ = analysis.diagnostics(FileId(0)).unwrap(); // warm the engine model + first analyses

    // Member (`ping`) referenced in all 150 files via `s.ping()` — the Member find-refs path.
    let ping = FilePosition {
        file: FileId(0),
        offset: u32::try_from(base.find("ping").unwrap()).unwrap(),
    };
    c.bench_function("find_references_member_150files", |b| {
        b.iter(|| black_box(analysis.find_references(black_box(ping)).unwrap()));
    });

    // Global (`class_name Shared`) referenced in all 150 files — the Global find-refs path.
    let shared = FilePosition {
        file: FileId(0),
        offset: u32::try_from(base.find("Shared").unwrap()).unwrap(),
    };
    c.bench_function("find_references_global_150files", |b| {
        b.iter(|| black_box(analysis.find_references(black_box(shared)).unwrap()));
    });
}

criterion_group!(benches, bench, bench_keystroke, bench_find_references);
criterion_main!(benches);
