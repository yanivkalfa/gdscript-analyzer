//! WS5 — the differential oracle.
//!
//! Cross-validates our hand-written parser against `tree-sitter-gdscript` (the
//! reference grammar / oracle, never the grammar-of-record — `plans/PHASE-1-…` §WS5).
//! For core GDScript, the two should agree on whether a file is well-formed; a
//! disagreement is a signal to fix our parser or to record a known tree-sitter
//! limitation in `tests/KNOWN_DIVERGENCES.md`.
//!
//! Native-only and feature-gated (tree-sitter compiles a C parser and floors at Rust
//! 1.90): run with
//!   cargo test -p gdscript-syntax --features tree-sitter-oracle --test differential
#![cfg(feature = "tree-sitter-oracle")]

use gdscript_syntax::parse;

/// Whether tree-sitter considers `src` to contain a syntax error.
fn ts_has_error(src: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_gdscript::LANGUAGE.into())
        .expect("load the tree-sitter-gdscript grammar");
    let tree = parser.parse(src, None).expect("tree-sitter parse");
    tree.root_node().has_error()
}

/// Core GDScript that both parsers fully support. Bleeding-edge 4.4/4.5 syntax that
/// tree-sitter 6.1 may lag on (typed dictionaries, varargs `...`, `@abstract`) is
/// intentionally excluded and documented in `tests/KNOWN_DIVERGENCES.md`.
const VALID: &[&str] = &[
    "func f():\n\tpass\n",
    "class_name Foo\nextends Node\n",
    "var x: int = 1\nconst K := 2\n",
    "func add(a: int, b: int) -> int:\n\treturn a + b\n",
    "signal died(reason: String)\n",
    "enum State { IDLE, RUNNING = 2 }\n",
    "@export var hp := 100\n@onready var s = $Sprite\n",
    "func g(z):\n\tif z and z > 0:\n\t\treturn 1\n\telif z < 0:\n\t\treturn 2\n\telse:\n\t\treturn 3\n",
    "func h():\n\tfor i in range(10):\n\t\tprint(i)\n\twhile true:\n\t\tbreak\n",
    "func m(v):\n\tmatch v:\n\t\t1:\n\t\t\tpass\n\t\t_:\n\t\t\tpass\n",
    "var arr = [1, 2, 3]\nvar d = {\"a\": 1, \"b\": 2}\n",
    "func e(a, b, c):\n\tvar r = 2 ** 3 + 1 * -4\n\tvar t = a if b else c\n\treturn r + t\n",
    "class Inner:\n\tvar y = 1\n\tfunc inner_fn():\n\t\treturn y\n",
    "func l():\n\tvar cb = func(x): return x + 1\n\treturn cb.call(1)\n",
    // Lambdas inside call arguments (single-line and multiline) — the stack-of-stacks
    // indentation case.
    "func s(arr):\n\treturn arr.map(func(x): return x * 2)\n",
    "func s(arr):\n\tarr.sort_custom(func(a, b):\n\t\treturn a < b\n\t)\n",
    // Broader coverage (burndown Stage 3 — a wider error-agreement set).
    "func f(a, b = 1, c := 2):\n\tpass\n",
    "static func make() -> Node:\n\treturn null\n",
    "func f():\n\tvar x = 1\n\tx += 1\n\tx -= 2\n\tx *= 3\n",
    "func f():\n\tassert(true, \"never\")\n",
    "func f():\n\tawait get_tree().process_frame\n",
    "@export_range(0, 100) var hp = 50\n",
    "var s = \"plain\"\nvar ml = \"\"\"multi\nline\"\"\"\n",
    "const D = {\n\t\"a\": 1,\n\t\"b\": 2,\n}\n",
    "func f():\n\tif true: pass\n\telse: return\n",
    "@tool\nextends Node\nfunc _ready():\n\tpass\n",
    "func f(v):\n\treturn v is Node and not v == null\n",
    "func f():\n\tvar n := Vector2(1, 2)\n\treturn n.x + n.y\n",
];

#[test]
fn both_parsers_accept_core_gdscript() {
    let mut divergences = Vec::new();
    for src in VALID {
        let ours_ok = parse(src).errors().is_empty();
        let ts_ok = !ts_has_error(src);
        if ours_ok != ts_ok {
            divergences.push(format!("  ours_ok={ours_ok} ts_ok={ts_ok} :: {src:?}"));
        }
    }
    assert!(
        divergences.is_empty(),
        "parser / tree-sitter divergences (fix our parser or document in \
         tests/KNOWN_DIVERGENCES.md):\n{}",
        divergences.join("\n"),
    );
}

/// Broken code: both parsers should flag an error (sanity check that the oracle is
/// actually exercising error detection, not vacuously agreeing).
#[test]
fn both_parsers_reject_broken_gdscript() {
    // Only cases BOTH parsers reject. tree-sitter-gdscript is lenient on a missing block `:`
    // (`func f()` / `if x` without a colon parse clean for it) — those are documented in
    // tests/KNOWN_DIVERGENCES.md, not asserted here.
    let broken = [
        "func f(:\n\tpass\n",
        "var = \n",
        "class\n",
        "func g() ->\n",
        "var x = = 1\n",           // doubled `=`
        "func f():\n\treturn )\n", // stray closing paren
    ];
    for src in broken {
        assert!(
            !parse(src).errors().is_empty(),
            "our parser should flag {src:?}"
        );
        assert!(ts_has_error(src), "tree-sitter should flag {src:?}");
    }
}

/// The count of top-level functions our parser sees (direct `FuncDecl` children of the file).
fn our_top_level_funcs(src: &str) -> usize {
    parse(src)
        .syntax_node()
        .children()
        .filter(|n| n.kind() == gdscript_syntax::SyntaxKind::FuncDecl)
        .count()
}

/// The count of top-level functions tree-sitter sees (`function_definition` named children).
fn ts_top_level_funcs(src: &str) -> usize {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_gdscript::LANGUAGE.into())
        .expect("load the tree-sitter-gdscript grammar");
    let tree = parser.parse(src, None).expect("tree-sitter parse");
    let root = tree.root_node();
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|n| n.kind() == "function_definition")
        .count()
}

/// A coarse **structural** cross-check (beyond pure error-agreement): both parsers must see the same
/// number of top-level function declarations — a high-signal skeleton property both grammars expose
/// unambiguously (catches a mis-nested / dropped / spuriously-split top-level `func`).
#[test]
fn top_level_function_count_agrees() {
    for src in VALID {
        assert_eq!(
            our_top_level_funcs(src),
            ts_top_level_funcs(src),
            "top-level function-count skeleton mismatch: {src:?}",
        );
    }
}
