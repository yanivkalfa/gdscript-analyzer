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
    let broken = ["func f(:\n\tpass\n", "var = \n", "class\n", "func g() ->\n"];
    for src in broken {
        assert!(
            !parse(src).errors().is_empty(),
            "our parser should flag {src:?}"
        );
        assert!(ts_has_error(src), "tree-sitter should flag {src:?}");
    }
}
