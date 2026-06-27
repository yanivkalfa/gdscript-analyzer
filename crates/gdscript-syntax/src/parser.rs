//! WS3 — the resilient recursive-descent parser.
//!
//! Architecture (matklad's "Resilient LL Parsing", adapted to build a [`cstree`]
//! tree — see `plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §WS3):
//!
//! - The parser walks the **non-trivia** tokens (real tokens + the synthetic
//!   `Newline`/`Indent`/`Dedent` markers) and emits a flat [`Event`] stream
//!   (`Open`/`Close`/`Advance`). It never returns `Result`: parsing *always* yields a
//!   tree plus a list of [`SyntaxError`]s.
//! - A [`Marker`]/[`MarkClosed`] API lets a node be opened, closed with its final
//!   kind, or wrapped retroactively (`open_before`) — e.g. promoting an expression to
//!   a `BinExpr` once an operator is seen.
//! - A **fuel** counter turns any accidental non-advancing loop into an immediate
//!   panic the robustness harness catches, instead of a hang.
//! - The `sink` replays the events over the *full* token stream (trivia included),
//!   building the lossless green tree and re-attaching trivia.
//!
//! The grammar productions live in [`grammar`]; this module owns the machinery.

use std::cell::Cell;
use std::sync::Arc;

use cstree::Syntax;
use cstree::build::GreenNodeBuilder;
use cstree::green::GreenNode;
use cstree::interning::TokenInterner;
use cstree::syntax::ResolvedNode;
use text_size::{TextRange, TextSize};

use crate::SyntaxKind;
use crate::lexer::{RawToken, tokenize};
use crate::prepass::run as run_prepass;

mod grammar;

/// The result of parsing a source file: a lossless green tree, the interner needed to
/// read token text back, and the diagnostics gathered while parsing.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    interner: Arc<TokenInterner>,
    errors: Vec<SyntaxError>,
}

impl Parse {
    /// The resolved (interner-carrying) red tree root. Cheap to produce; supports
    /// `Display`/`.text()` and the byte-exact round-trip.
    #[must_use]
    pub fn syntax_node(&self) -> ResolvedNode<SyntaxKind> {
        ResolvedNode::new_root_with_resolver(self.green.clone(), Arc::clone(&self.interner))
    }

    /// The parse diagnostics (lexer/parser recovery + indentation issues).
    #[must_use]
    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// The raw green tree (position-independent, shared).
    #[must_use]
    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    /// A stable, indented S-expression dump of the tree (kinds + byte ranges + token
    /// text) — the golden-fixture review surface.
    #[must_use]
    pub fn debug_tree(&self) -> String {
        cstree::syntax::SyntaxNode::<SyntaxKind>::new_root(self.green.clone())
            .debug(&self.interner, true)
    }
}

/// Equality compares the lossless green tree and the diagnostics; the **interner is excluded**
/// because it is a derived token-text cache (two parses with equal green trees reference equal
/// token text). This makes [`Parse`] a sound `salsa` tracked-fn return: an unchanged reparse
/// *backdates* instead of invalidating dependents — the Phase-3 incrementality precondition
/// (Playbook §4). `GreenNode` equality is structural, so this is `O(tree)` worst case but
/// short-circuits on the first difference.
impl PartialEq for Parse {
    fn eq(&self, other: &Self) -> bool {
        self.green == other.green && self.errors == other.errors
    }
}
impl Eq for Parse {}

/// A byte-ranged syntax diagnostic with an "expected X" style message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    /// The byte range the error applies to.
    pub range: TextRange,
    /// A human-readable message.
    pub message: String,
}

/// Parse GDScript source into a lossless [`Parse`]. Never fails.
#[must_use]
pub fn parse(text: &str) -> Parse {
    let raw = tokenize(text);
    let (tokens, indent_diags) = run_prepass(&raw, text);

    let mut p = Parser::new(text, &tokens);
    p.source_file();
    let Parser {
        events, mut errors, ..
    } = p;

    errors.extend(indent_diags.into_iter().map(|d| SyntaxError {
        range: d.range,
        message: d.message,
    }));

    let (green, interner) = build_tree(&events, &tokens, text);
    Parse {
        green,
        interner,
        errors,
    }
}

/// A parser event. `Open`'s kind is `Tombstone` until the matching [`Parser::close`]
/// overwrites it; an `Open` left as `Tombstone` is an abandoned marker the sink skips.
#[derive(Debug, Clone, Copy)]
enum Event {
    Open { kind: SyntaxKind },
    Close,
    Advance,
}

/// A handle to an opened-but-not-yet-closed node (an index into the event list).
struct Marker {
    pos: usize,
}

/// A handle to a closed node, usable to wrap it retroactively via
/// [`Parser::open_before`].
#[derive(Clone, Copy)]
struct MarkClosed {
    pos: usize,
}

/// How many times [`Parser::nth`] may be called without an intervening
/// [`Parser::advance`] before we declare the parser stuck. Generous; only a genuine
/// non-advancing loop trips it.
const FUEL: u32 = 256;

struct Parser<'s> {
    src: &'s str,
    tokens: &'s [RawToken],
    /// Indices into `tokens` of the non-trivia tokens the grammar walks.
    nontrivia: Vec<usize>,
    /// Cursor into `nontrivia`.
    pos: usize,
    fuel: Cell<u32>,
    events: Vec<Event>,
    errors: Vec<SyntaxError>,
}

impl<'s> Parser<'s> {
    fn new(src: &'s str, tokens: &'s [RawToken]) -> Self {
        let nontrivia = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.kind.is_trivia())
            .map(|(i, _)| i)
            .collect();
        Self {
            src,
            tokens,
            nontrivia,
            pos: 0,
            fuel: Cell::new(FUEL),
            events: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// The kind `n` non-trivia tokens ahead (`Eof` past the end). Burns a unit of fuel.
    fn nth(&self, n: usize) -> SyntaxKind {
        assert!(self.fuel.get() > 0, "parser stuck at position {}", self.pos);
        self.fuel.set(self.fuel.get() - 1);
        self.nontrivia
            .get(self.pos + n)
            .map_or(SyntaxKind::Eof, |&i| self.tokens[i].kind)
    }

    fn at(&self, kind: SyntaxKind) -> bool {
        self.nth(0) == kind
    }

    fn at_any(&self, kinds: &[SyntaxKind]) -> bool {
        kinds.contains(&self.nth(0))
    }

    fn eof(&self) -> bool {
        self.pos >= self.nontrivia.len()
    }

    /// The byte range of the current token (an empty range at EOF), for diagnostics.
    fn cur_range(&self) -> TextRange {
        self.nontrivia.get(self.pos).map_or_else(
            || TextRange::empty(TextSize::of(self.src)),
            |&i| self.tokens[i].range,
        )
    }

    /// The source text of the current token (`""` at EOF) — used for the few
    /// contextual keywords GDScript lexes as identifiers (`get`/`set`).
    fn cur_text(&self) -> &str {
        self.nontrivia
            .get(self.pos)
            .map_or("", |&i| &self.src[self.tokens[i].range])
    }

    fn advance(&mut self) {
        // A resilient parser treats `advance` at EOF as a no-op: recovery paths may
        // reach it, and every list/loop re-checks `eof()`, so this can't spin. (Fuel is
        // only reset on a real advance, so a stuck loop still trips the fuel guard.)
        if self.eof() {
            return;
        }
        self.fuel.set(FUEL);
        self.events.push(Event::Advance);
        self.pos += 1;
    }

    fn open(&mut self) -> Marker {
        let m = Marker {
            pos: self.events.len(),
        };
        self.events.push(Event::Open {
            kind: SyntaxKind::Tombstone,
        });
        m
    }

    // `Marker` is intentionally consumed by value: moving it enforces "close a node
    // exactly once" at the type level (a used Marker can't be reused or dropped).
    #[allow(clippy::needless_pass_by_value)]
    fn close(&mut self, m: Marker, kind: SyntaxKind) -> MarkClosed {
        self.events[m.pos] = Event::Open { kind };
        self.events.push(Event::Close);
        MarkClosed { pos: m.pos }
    }

    /// Wrap an already-closed node in a new (outer) node — the retroactive-wrap used
    /// by the Pratt parser to promote operands into `BinExpr`/`CallExpr`/etc.
    fn open_before(&mut self, m: MarkClosed) -> Marker {
        self.events.insert(
            m.pos,
            Event::Open {
                kind: SyntaxKind::Tombstone,
            },
        );
        Marker { pos: m.pos }
    }

    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or record an "expected" diagnostic (without consuming).
    fn expect(&mut self, kind: SyntaxKind) {
        if self.eat(kind) {
            return;
        }
        self.error(format!("expected {kind:?}"));
    }

    /// Record a diagnostic at the current token.
    fn error(&mut self, message: String) {
        self.errors.push(SyntaxError {
            range: self.cur_range(),
            message,
        });
    }

    /// Wrap the current (unexpected) token in an `ErrorNode` and report it — the
    /// skip-one-token recovery step. Makes progress so loops terminate. Returns the
    /// closed node so it can be used as an operand placeholder in expression recovery.
    fn advance_with_error(&mut self, message: &str) -> MarkClosed {
        let m = self.open();
        self.error(message.to_owned());
        if !self.eof() {
            self.advance();
        }
        self.close(m, SyntaxKind::ErrorNode)
    }
}

/// Replay the parser events over the full token stream (trivia included) to build the
/// lossless green tree. Trivia is flushed before each advanced token; trailing trivia
/// is flushed inside the root just before it closes.
fn build_tree(events: &[Event], tokens: &[RawToken], src: &str) -> (GreenNode, Arc<TokenInterner>) {
    let mut builder: GreenNodeBuilder<'static, 'static, SyntaxKind> = GreenNodeBuilder::new();
    let mut tok = 0usize;
    let mut depth: u32 = 0;

    for event in events {
        match *event {
            Event::Open { kind } => {
                if kind == SyntaxKind::Tombstone {
                    continue; // abandoned marker
                }
                depth += 1;
                builder.start_node(kind);
            }
            Event::Close => {
                depth -= 1;
                if depth == 0 {
                    // Root closing: flush any remaining tokens (trailing trivia) inside
                    // it so nothing escapes the single root.
                    while tok < tokens.len() {
                        emit(&mut builder, tokens[tok], src);
                        tok += 1;
                    }
                }
                builder.finish_node();
            }
            Event::Advance => {
                while tok < tokens.len() && tokens[tok].kind.is_trivia() {
                    emit(&mut builder, tokens[tok], src);
                    tok += 1;
                }
                if tok < tokens.len() {
                    emit(&mut builder, tokens[tok], src);
                    tok += 1;
                }
            }
        }
    }

    let (green, cache) = builder.finish();
    let interner = cache
        .expect("a builder created with `new()` owns its cache")
        .into_interner()
        .expect("the cache owns its interner");
    (green, Arc::new(interner))
}

/// Emit one token into the builder: fixed-lexeme kinds via `static_token`, everything
/// else (identifiers, literals, trivia, the zero-width synthetic markers) via the
/// interning `token`.
fn emit(builder: &mut GreenNodeBuilder<'static, 'static, SyntaxKind>, t: RawToken, src: &str) {
    if <SyntaxKind as Syntax>::static_text(t.kind).is_some() {
        builder.static_token(t.kind);
    } else {
        builder.token(t.kind, &src[t.range]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(src: &str) {
        let parse = parse(src);
        assert_eq!(
            parse.syntax_node().to_string(),
            src,
            "round-trip mismatch for {src:?}",
        );
    }

    #[test]
    fn round_trips_a_function() {
        round_trips("func f():\n\tpass\n");
    }

    #[test]
    fn round_trips_inline_function() {
        round_trips("func square(a): return a\n");
    }

    #[test]
    fn round_trips_with_trivia() {
        round_trips("## doc\nfunc _ready() -> void:\n\tpass\n\n# trailing comment\n");
    }

    #[test]
    fn round_trips_multiple_functions() {
        round_trips("func a():\n\tpass\nfunc b():\n\tpass\n");
    }

    #[test]
    fn round_trips_empty_and_blank() {
        round_trips("");
        round_trips("\n\n");
        round_trips("# only a comment\n");
    }

    #[test]
    fn produces_expected_top_level_shape() {
        let parse = parse("func f():\n\tpass\n");
        let root = parse.syntax_node();
        assert_eq!(root.kind(), SyntaxKind::SourceFile);
        let func = root
            .children()
            .find(|n| n.kind() == SyntaxKind::FuncDecl)
            .expect("a FuncDecl child");
        assert!(func.children().any(|n| n.kind() == SyntaxKind::Block));
    }

    /// A node-only S-expression (no tokens, no trivia) — the structural shape, used to
    /// assert operator precedence/associativity.
    fn node_sexpr(node: &ResolvedNode<SyntaxKind>) -> String {
        let mut s = format!("({:?}", node.kind());
        for child in node.children() {
            s.push(' ');
            s.push_str(&node_sexpr(child));
        }
        s.push(')');
        s
    }

    fn structure(src: &str) -> String {
        node_sexpr(&parse(src).syntax_node())
    }

    #[test]
    fn precedence_factor_binds_tighter_than_add() {
        // 1 + 2 * 3  →  1 + (2 * 3)
        assert_eq!(
            structure("var x = 1 + 2 * 3\n"),
            "(SourceFile (VarDecl (Name) (BinExpr (Literal) (BinExpr (Literal) (Literal)))))"
        );
        // 1 * 2 + 3  →  (1 * 2) + 3
        assert_eq!(
            structure("var x = 1 * 2 + 3\n"),
            "(SourceFile (VarDecl (Name) (BinExpr (BinExpr (Literal) (Literal)) (Literal))))"
        );
    }

    #[test]
    fn power_is_left_associative() {
        // GDScript: 2 ** 3 ** 4  →  (2 ** 3) ** 4  (unlike Python's right-assoc)
        assert_eq!(
            structure("var x = 2 ** 3 ** 4\n"),
            "(SourceFile (VarDecl (Name) (BinExpr (BinExpr (Literal) (Literal)) (Literal))))"
        );
    }

    #[test]
    fn unary_minus_then_power() {
        // -2 ** 2  →  -(2 ** 2)  (power binds tighter than the unary sign)
        assert_eq!(
            structure("var x = -2 ** 2\n"),
            "(SourceFile (VarDecl (Name) (UnaryExpr (BinExpr (Literal) (Literal)))))"
        );
    }

    #[test]
    fn ternary_is_right_associative() {
        assert_eq!(
            structure("var x = a if c else b\n"),
            "(SourceFile (VarDecl (Name) (TernaryExpr (NameRef) (NameRef) (NameRef))))"
        );
    }

    #[test]
    fn postfix_chain_call_field_index() {
        // a.b().c[0]
        assert_eq!(
            structure("var x = a.b().c[0]\n"),
            "(SourceFile (VarDecl (Name) (IndexExpr (FieldExpr (CallExpr (FieldExpr (NameRef) \
             (NameRef)) (ArgList)) (NameRef)) (Literal))))"
        );
    }

    #[test]
    fn leading_utf8_bom_is_trivia_not_an_error() {
        // A `.gd` saved with a UTF-8 BOM is valid GDScript (Godot strips it). The BOM must
        // be lexed as trivia, round-trip byte-for-byte, and NOT produce a parse error at 1:1.
        let src = "\u{feff}class_name Foo\nextends Node\n";
        let parse = parse(src);
        assert_eq!(
            parse.syntax_node().to_string(),
            src,
            "BOM file must round-trip byte-for-byte"
        );
        assert!(
            parse.errors().is_empty(),
            "BOM-prefixed file should parse clean: {:?}",
            parse.errors()
        );
        // The BOM does not shift the first declaration's indentation: `class_name` is at col 0.
        assert!(
            structure(src).starts_with("(SourceFile (ClassNameDecl"),
            "{}",
            structure(src)
        );
    }

    #[test]
    fn multiline_lambda_does_not_absorb_following_paren_line() {
        // A block-body lambda assigned to a var, followed by a statement that begins with
        // `(`. The dedent ends the lambda; the `(...)` line is its OWN statement — it must
        // NOT be parsed as a postfix call on the lambda. (Regression: the parser used to
        // absorb the `(` as `CallExpr(LambdaExpr, …)`.)
        let src = "func f():\n\tvar cb := func():\n\t\treturn 1\n\t(self).process()\n";
        let st = structure(src);
        assert!(
            st.contains("(VarDecl (Name) (LambdaExpr"),
            "lambda should be the var initializer, standalone: {st}"
        );
        assert!(
            !st.contains("CallExpr (LambdaExpr"),
            "the following `(` line must not be absorbed as a call on the lambda: {st}"
        );
        // The `(self).process()` line is a separate ExprStmt with its own call chain.
        assert!(
            st.contains("(ExprStmt (CallExpr (FieldExpr (ParenExpr"),
            "the `(self).process()` line should be its own statement: {st}"
        );
        round_trips(src);
    }

    #[test]
    fn inline_lambda_still_chains_postfix() {
        // An *inline* (single-line) lambda has no dedent, so a postfix `.call()` on the same
        // logical line must still chain — the fix only suppresses postfix after a block body.
        let src = "var x = (func(): return 1).call()\n";
        let st = structure(src);
        assert!(
            st.contains("CallExpr (FieldExpr (ParenExpr (LambdaExpr"),
            "inline lambda should still accept a postfix chain: {st}"
        );
        round_trips(src);
    }

    #[test]
    fn statement_level_annotation_in_a_body_parses_clean() {
        // `@warning_ignore("…")` (and friends) can decorate a STATEMENT inside a function body, not
        // just a declaration. It must parse as a sibling Annotation, not fall into expr-stmt and
        // error. (Found on the godot-demo-projects corpus.)
        let src = "func f():\n\t@warning_ignore(\"integer_division\")\n\tvar x := 1 / 2\n";
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        round_trips(src);
    }

    #[test]
    fn multiline_lambda_arg_with_dedented_closer_parses_clean() {
        // A multi-line lambda passed as a call argument, with the closing `)` on its own line at a
        // column BETWEEN the lambda header and its body (real Godot style — the tween demo). The `)`
        // ends the body via the bracket close — no spurious INDENT, no syntax error.
        let src = "func f():\n\tobj.call(func():\n\t\t\tbody()\n\t\t)\n";
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        round_trips(src);
    }

    #[test]
    fn multiline_lambda_body_ending_at_a_comma_parses_clean() {
        // A multi-line lambda whose single-statement body is followed by `, more_args` on the same
        // line (`call(func(v): body, 0.0, 1.0)`). A bare `,` at the lambda's enclosing bracket depth
        // is the call's argument separator, so it ends the body. (Found on the corpus.)
        let src = "func f():\n\tobj.call(\n\t\tfunc(v):\n\t\t\tuse(v), 0.0, 1.0)\n";
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        round_trips(src);
    }

    /// A broad, realistic GDScript file exercising most of the grammar. The key
    /// invariant is that it round-trips byte-for-byte and parses without panicking.
    const CORPUS: &str = r#"@tool
class_name Player extends CharacterBody2D
## A documented player controller.

const SPEED := 300.0
@export var health: int = 100
@export_range(0, 100) var armor := 0
static var instances: Array[Player] = []

enum State { IDLE, RUNNING, JUMPING = 10 }

signal died(reason: String)

var _vel: Vector2 = Vector2.ZERO:
	get:
		return _vel
	set(value):
		_vel = value

class Inner extends RefCounted:
	var x = 1
	func helper() -> int:
		return x * 2

func _ready() -> void:
	var node := $Sprite2D
	var unique = %HealthBar
	add_child(preload("res://thing.tscn").instantiate())
	for i in range(0, 10):
		if i % 2 == 0 and i > 0:
			print(i, " even")
		elif i == 5:
			continue
		else:
			pass
	while health > 0:
		health -= 1
	match State.IDLE:
		State.IDLE, State.RUNNING:
			pass
		[var first, ..]:
			print(first)
		{"key": var v} when v > 0:
			print(v)
		_:
			breakpoint
	var cb := func(a: int, b := 2) -> int: return a + b
	var ok = node is Node2D
	var cast = node as Sprite2D
	assert(health >= 0, "negative health")
	died.emit("test")
"#;

    #[test]
    fn corpus_round_trips_byte_for_byte() {
        round_trips(CORPUS);
    }

    #[test]
    fn corpus_parses_without_unexpected_errors() {
        // The corpus is valid GDScript; it should parse with no syntax errors.
        let parse = parse(CORPUS);
        assert!(
            parse.errors().is_empty(),
            "unexpected parse errors:\n{:#?}",
            parse.errors()
        );
    }

    #[test]
    fn inline_if_elif_else_clauses_attach() {
        // Real-corpus regression (ReactiveUI-Godot reconciler.gd / router matcher.gd):
        // an inline branch body (`if c: stmt`) followed by `elif`/`else` on the next
        // line. The inline body ends at a logical newline that must not orphan the
        // clause as a stray statement.
        let src = "func f():\n\tif a: x = 1\n\telif b: x = 2\n\telse: x = 3\n";
        let parse = parse(src);
        assert_eq!(parse.syntax_node().to_string(), src, "lossless");
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let if_stmt = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::IfStmt)
            .expect("an IfStmt node");
        assert!(
            if_stmt
                .descendants()
                .any(|n| n.kind() == SyntaxKind::ElifClause),
            "elif clause attached to the if"
        );
        assert!(
            if_stmt
                .descendants()
                .any(|n| n.kind() == SyntaxKind::ElseClause),
            "else clause attached to the if"
        );
    }

    #[test]
    fn soft_keyword_names_parse() {
        // Real-corpus regression (ReactiveUI-Godot router): Godot's `is_identifier()` /
        // `is_node_name()` soft keywords used as identifiers — `match` as a function
        // name and a member name, `when` as a parameter and an identifier expression.
        let src = "static func match(when: bool) -> int:\n\tvar r = RUIRouteMatcher.match(when)\n\treturn when\n";
        let parse = parse(src);
        assert_eq!(parse.syntax_node().to_string(), src, "lossless");
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
    }

    #[test]
    fn multiline_lambda_with_trailing_call_paren() {
        // Real-corpus regression (ReactiveUI-Godot media.gd): a multiline lambda whose
        // enclosing call paren closes on the body's last line (`call(func(): … last())`).
        let src = "func f():\n\tt.connect(func():\n\t\tif ok:\n\t\t\tp.free())\n";
        let parse = parse(src);
        assert_eq!(parse.syntax_node().to_string(), src, "lossless");
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
    }

    #[test]
    fn multiline_lambda_in_call_argument_parses() {
        // The fixed lambda-in-brackets case: a multiline lambda body inside a call.
        let src = "func f():\n\tcb(func(a, b):\n\t\treturn a + b\n\t)\n";
        let parse = parse(src);
        assert_eq!(parse.syntax_node().to_string(), src, "lossless");
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        let root = parse.syntax_node();
        let lambda = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::LambdaExpr)
            .expect("a LambdaExpr node");
        let block = lambda
            .children()
            .find(|n| n.kind() == SyntaxKind::Block)
            .expect("the lambda body Block");
        assert!(
            block
                .descendants()
                .any(|n| n.kind() == SyntaxKind::ReturnStmt),
            "the lambda body contains the return statement"
        );
    }

    #[test]
    fn single_line_lambda_in_call_argument_parses() {
        // The body stops at the call's `)` (parser inline-block fix).
        let src = "var m = arr.map(func(x): x * 2)\n";
        let parse = parse(src);
        assert_eq!(parse.syntax_node().to_string(), src, "lossless");
        assert!(parse.errors().is_empty(), "no errors: {:?}", parse.errors());
        assert!(
            parse
                .syntax_node()
                .descendants()
                .any(|n| n.kind() == SyntaxKind::LambdaExpr),
            "a LambdaExpr node"
        );
    }

    #[test]
    fn broken_code_recovers_and_round_trips() {
        // A malformed parameter list: a tree is still produced, errors are reported,
        // siblings still parse, and the source round-trips.
        let src = "func ok():\n\tpass\nfunc bad(:\n\tpass\nfunc also_ok():\n\tpass\n";
        let parse = parse(src);
        assert_eq!(
            parse.syntax_node().to_string(),
            src,
            "recovery must stay lossless"
        );
        assert!(!parse.errors().is_empty(), "expected a syntax error");
        // The two well-formed functions are still recognized.
        let funcs = parse
            .syntax_node()
            .children()
            .filter(|n| n.kind() == SyntaxKind::FuncDecl)
            .count();
        assert!(
            funcs >= 2,
            "siblings should survive a broken declaration, got {funcs}"
        );
    }

    #[test]
    fn golden_small_class() {
        let parse = parse("class_name Foo\nvar x := 1\n");
        expect_test::expect_file!["../test_data/golden/small_class.cst"]
            .assert_eq(&parse.debug_tree());
    }
}
