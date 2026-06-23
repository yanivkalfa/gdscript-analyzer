//! Robustness harness — the parser must **never panic** on any input, and must always
//! round-trip byte-for-byte (`plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §5).
//!
//! These run on stable + the local Windows-gnu toolchain (in-process `proptest`, no
//! `cargo-fuzz`/libFuzzer). Any discovered counterexample is persisted under
//! `proptest-regressions/` and replayed forever after.

use gdscript_syntax::parse;
use proptest::prelude::*;

/// Parse must succeed (no panic) and reproduce the input exactly.
fn check(src: &str) {
    let parsed = parse(src);
    assert_eq!(
        parsed.syntax_node().to_string(),
        src,
        "round-trip mismatch on {src:?}",
    );
}

proptest! {
    // Printable ASCII plus tab/newline — the shape most likely to form near-valid (and
    // adversarially malformed) GDScript, exercising the lexer + indentation + recovery.
    #[test]
    fn never_panics_on_ascii_textish(s in r"[ -~\t\r\n]{0,400}") {
        check(&s);
    }

    // Arbitrary bytes (often invalid UTF-8) via lossy decode — truncation, control
    // bytes, stray delimiters.
    #[test]
    fn never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let s = String::from_utf8_lossy(&bytes);
        check(s.as_ref());
    }

    // Full Unicode strings — multibyte boundaries, combining marks, etc.
    #[test]
    fn never_panics_on_unicode(s in r"\PC{0,200}") {
        check(&s);
    }
}

/// A realistic file with many constructs, used for truncation fuzzing below.
const SAMPLE: &str = r#"@tool
class_name Player extends Node2D
const SPEED := 300.0
@export var health: int = 100
signal died(reason: String)
enum State { IDLE, RUNNING = 2 }
var v: Vector2 = Vector2.ZERO:
	get: return v
	set(value): v = value
func _ready() -> void:
	for i in range(10):
		if i % 2 == 0 and i > 0:
			print($Sprite, %Bar, i ** 2)
		elif i == 5:
			continue
		else:
			pass
	match State.IDLE:
		State.IDLE, State.RUNNING:
			pass
		[var a, ..]:
			print(a)
		_:
			breakpoint
	var cb := func(x: int) -> int: return x + 1
	var ok = (v as Node) is Node and not v in [1, 2]
	assert(health >= 0, "neg")
"#;

/// Every prefix of a realistic file must parse without panicking and round-trip.
/// Truncating at every byte boundary is a deterministic, high-yield way to catch
/// EOF/recovery edge cases (incomplete `func`, dangling `:`, half-open brackets, …).
#[test]
fn every_prefix_round_trips() {
    for i in 0..=SAMPLE.len() {
        if !SAMPLE.is_char_boundary(i) {
            continue;
        }
        let prefix = &SAMPLE[..i];
        let parsed = parse(prefix);
        assert_eq!(
            parsed.syntax_node().to_string(),
            prefix,
            "prefix of length {i} did not round-trip",
        );
    }
}

/// Adversarial broken snippets: each must yield a tree (no panic), report at least one
/// error, and round-trip losslessly.
#[test]
fn broken_snippets_recover() {
    let cases = [
        "func (",
        "func f(:\n\tpass",
        "var x = \nfunc g(): pass",
        "if :\n\tpass",
        "class\nclass\nclass",
        "match\n\t:\n",
        "[[[[[",
        "func f(a, b, , c):\n\tpass",
        ")))",
        "@@@@",
        "var x = 1 +",
        "for in :\n",
    ];
    for src in cases {
        let parsed = parse(src);
        assert_eq!(parsed.syntax_node().to_string(), src, "lossless on {src:?}");
        assert!(!parsed.errors().is_empty(), "expected errors on {src:?}");
    }
}
