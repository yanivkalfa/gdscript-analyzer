//! The GDScript grammar productions.
//!
//! Hand-written resilient recursive descent over the non-trivia token stream, with a
//! Pratt expression parser. Precedence mirrors Godot's `gdscript_parser.cpp` ladder
//! (`plans/PHASE-1-IMPLEMENTATION-PLAYBOOK.md` §WS3) — notably `**` is **left**
//! associative and `as` sits at a **low** precedence (just above assignment).
//!
//! Recovery: every list/block loop is bounded by a terminator or the statement-level
//! recovery set, and `advance_with_error` guarantees forward progress so the parser
//! never hangs (the fuel counter is the backstop).

// A grammar file references dozens of `SyntaxKind` variants per function; the glob is
// the conventional readability choice here (rust-analyzer/Biome do the same).
#![allow(clippy::enum_glob_use)]

use super::{MarkClosed, Parser};
use crate::SyntaxKind::{self, *};

/// Operator associativity.
#[derive(Clone, Copy)]
enum Assoc {
    Left,
    Right,
}

// Precedence levels, low (loosest) → high (tightest), mirroring Godot's enum.
const PREC_ASSIGN: u8 = 1;
const PREC_CAST: u8 = 2;
const PREC_TERNARY: u8 = 3;
const PREC_OR: u8 = 4;
const PREC_AND: u8 = 5;
const PREC_NOT: u8 = 6; // unary
const PREC_IN: u8 = 7;
const PREC_CMP: u8 = 8;
const PREC_BIT_OR: u8 = 9;
const PREC_BIT_XOR: u8 = 10;
const PREC_BIT_AND: u8 = 11;
const PREC_SHIFT: u8 = 12;
const PREC_ADD: u8 = 13;
const PREC_FACTOR: u8 = 14;
const PREC_SIGN: u8 = 15; // unary - +
const PREC_BIT_NOT: u8 = 16; // unary ~
const PREC_POWER: u8 = 17; // ** (left assoc)
const PREC_TYPE_TEST: u8 = 18; // is
const PREC_AWAIT: u8 = 19; // unary

/// `(left_bp, right_bp)` for an infix operator. Left-assoc keeps `left < right`,
/// right-assoc flips it so equal-precedence operators nest the other way.
const fn bp(prec: u8, assoc: Assoc) -> (u8, u8) {
    match assoc {
        Assoc::Left => (2 * prec, 2 * prec + 1),
        Assoc::Right => (2 * prec + 1, 2 * prec),
    }
}

/// Kinds that begin a declaration / statement — the recovery resync set.
const RECOVERY: &[SyntaxKind] = &[
    FuncKw,
    VarKw,
    ConstKw,
    ClassKw,
    ClassNameKw,
    ExtendsKw,
    EnumKw,
    SignalKw,
    StaticKw,
    At,
    IfKw,
    ForKw,
    WhileKw,
    MatchKw,
    ReturnKw,
    BreakKw,
    ContinueKw,
    PassKw,
    AssertKw,
    BreakpointKw,
];

impl Parser<'_> {
    // ---- file & members -------------------------------------------------------

    /// `source_file := (item | NEWLINE)*`
    pub(super) fn source_file(&mut self) {
        let m = self.open();
        self.members(&[]);
        self.close(m, SourceFile);
    }

    /// Parse members until EOF or any kind in `until` (used for class bodies, where
    /// `until = [Dedent]`). Skips blank lines / stray layout markers.
    fn members(&mut self, until: &[SyntaxKind]) {
        while !self.eof() && !self.at_any(until) {
            if self.at_any(&[Newline, Indent, Dedent, Semicolon]) {
                self.advance();
                continue;
            }
            self.item();
        }
    }

    /// One top-level / class member.
    fn item(&mut self) {
        match self.nth(0) {
            At => self.annotation(),
            ClassNameKw => self.class_name_decl(),
            ExtendsKw => self.extends_clause(),
            FuncKw => self.func_decl(),
            StaticKw if self.nth(1) == FuncKw => self.func_decl(),
            VarKw | StaticKw => self.var_decl(),
            ConstKw => self.const_decl(),
            EnumKw => self.enum_decl(),
            SignalKw => self.signal_decl(),
            ClassKw => self.inner_class(),
            _ => {
                self.advance_with_error("expected a declaration");
            }
        }
    }

    /// `@name (args)?`
    fn annotation(&mut self) {
        let m = self.open();
        self.expect(At);
        if self.at(Ident) {
            self.advance(); // the annotation name (e.g. `export`, `onready`, `abstract`)
        } else {
            self.error("expected an annotation name after `@`".to_owned());
        }
        if self.at(LParen) {
            self.arg_list();
        }
        self.close(m, Annotation);
    }

    /// `class_name Name (extends Target)?`
    fn class_name_decl(&mut self) {
        let m = self.open();
        self.expect(ClassNameKw);
        self.opt_name();
        if self.eat(ExtendsKw) {
            self.extends_target();
        }
        self.close(m, ClassNameDecl);
    }

    /// `extends Target`
    fn extends_clause(&mut self) {
        let m = self.open();
        self.expect(ExtendsKw);
        self.extends_target();
        self.close(m, ExtendsClause);
    }

    /// `extends` target: `Base`, `Base.Inner`, `"res://x.gd"`, or `"x.gd".Inner`.
    fn extends_target(&mut self) {
        if self.eat(String) {
            // path form
        } else if self.at(Ident) {
            self.advance();
        }
        while self.eat(Dot) {
            self.eat(Ident);
        }
    }

    /// `(static)? func Name '(' params ')' ('->' type)? ':' block`
    fn func_decl(&mut self) {
        let m = self.open();
        self.eat(StaticKw);
        self.expect(FuncKw);
        self.opt_name();
        self.param_list();
        if self.eat(Arrow) {
            self.type_ref();
        }
        self.expect(Colon);
        self.block();
        self.close(m, FuncDecl);
    }

    /// `(static)? var Name (':' type)? ((':=' | '=') expr)? property_body?`
    fn var_decl(&mut self) {
        let m = self.open();
        self.eat(StaticKw);
        self.expect(VarKw);
        self.opt_name();
        if self.eat(Colon) && !self.at_any(&[Eq, Newline, Dedent]) {
            self.type_ref();
        }
        if self.eat(ColonEq) || self.eat(Eq) {
            self.expr();
        }
        // A trailing `:` introduces an inline/indented property body.
        if self.at(Colon) {
            self.property_body();
        }
        self.close(m, VarDecl);
    }

    /// `const Name (':' type)? ((':=' | '=') expr)?`
    fn const_decl(&mut self) {
        let m = self.open();
        self.expect(ConstKw);
        self.opt_name();
        if self.eat(Colon) && !self.at_any(&[Eq, Newline, Dedent]) {
            self.type_ref();
        }
        if self.eat(ColonEq) || self.eat(Eq) {
            self.expr();
        }
        self.close(m, ConstDecl);
    }

    /// `enum Name? '{' (variant (',' variant)*)? '}'`
    fn enum_decl(&mut self) {
        let m = self.open();
        self.expect(EnumKw);
        self.opt_name();
        self.expect(LBrace);
        while !self.at(RBrace) && !self.eof() {
            let v = self.open();
            self.eat(Ident);
            if self.eat(Eq) {
                self.expr();
            }
            self.close(v, EnumVariant);
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RBrace);
        self.close(m, EnumDecl);
    }

    /// `signal Name ('(' params ')')?`
    fn signal_decl(&mut self) {
        let m = self.open();
        self.expect(SignalKw);
        self.opt_name();
        if self.at(LParen) {
            self.param_list();
        }
        self.close(m, SignalDecl);
    }

    /// `class Name (extends Target)? ':' class_body`
    fn inner_class(&mut self) {
        let m = self.open();
        self.expect(ClassKw);
        self.opt_name();
        if self.eat(ExtendsKw) {
            self.extends_target();
        }
        self.expect(Colon);
        if self.eat(Newline) && self.at(Indent) {
            let b = self.open();
            self.advance(); // INDENT
            self.members(&[Dedent]);
            self.eat(Dedent);
            self.close(b, ClassBody);
        }
        self.close(m, InnerClassDecl);
    }

    /// A declaration name: a single identifier wrapped in a `Name` node.
    fn opt_name(&mut self) {
        if self.at(Ident) {
            let m = self.open();
            self.advance();
            self.close(m, Name);
        }
    }

    /// `'(' (param (',' param)*)? ')'` where a param is `name (: type)? (= expr)?` or
    /// a `...rest` vararg.
    fn param_list(&mut self) {
        let m = self.open();
        self.expect(LParen);
        while !self.at(RParen) && !self.eof() {
            if self.at(Ellipsis) {
                let p = self.open();
                self.advance();
                self.opt_name();
                self.close(p, VarargParam);
            } else if self.at(Ident) {
                let p = self.open();
                self.opt_name();
                if self.eat(Colon) && !self.at_any(&[Eq, Comma, RParen]) {
                    self.type_ref();
                }
                if self.eat(ColonEq) || self.eat(Eq) {
                    self.expr();
                }
                self.close(p, Param);
            } else {
                // A stray token where a parameter was expected (e.g. a double comma):
                // report it and skip a separator to recover.
                self.error("expected a parameter".to_owned());
                if !self.eat(Comma) {
                    break;
                }
                continue;
            }
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RParen);
        self.close(m, ParamList);
    }

    /// A type reference: `T`, `A.B`, `Array[T]`, `Dictionary[K, V]`.
    fn type_ref(&mut self) {
        let m = self.open();
        if self.at(Ident) || self.at(VoidKw) {
            self.advance();
            while self.eat(Dot) {
                self.eat(Ident);
            }
            if self.eat(LBrack) {
                self.type_ref();
                while self.eat(Comma) {
                    self.type_ref();
                }
                self.expect(RBrack);
            }
        } else {
            self.error("expected a type".to_owned());
        }
        self.close(m, TypeRef);
    }

    /// A property accessor body: `: get/set` (inline `= func`, or indented blocks).
    fn property_body(&mut self) {
        let m = self.open();
        self.expect(Colon);
        if self.eat(Newline) && self.at(Indent) {
            self.advance(); // INDENT
            while !self.at(Dedent) && !self.eof() {
                if self.at_any(&[Newline, Semicolon]) {
                    self.advance();
                    continue;
                }
                self.accessor();
            }
            self.eat(Dedent);
        } else {
            // inline: `get = f, set = f`
            self.accessor();
            while self.eat(Comma) {
                self.accessor();
            }
        }
        self.close(m, PropertyBody);
    }

    /// One `get`/`set` accessor (block or `= func` form).
    fn accessor(&mut self) {
        let is_getter = self.cur_text() == "get";
        let m = self.open();
        self.eat(Ident); // `get` / `set`
        if self.at(LParen) {
            self.param_list(); // `set(value)`
        }
        if self.eat(Colon) {
            self.block();
        } else if self.eat(Eq) {
            self.expr();
        }
        self.close(m, if is_getter { Getter } else { Setter });
    }

    // ---- statements & blocks --------------------------------------------------

    /// A block: indented (`NEWLINE INDENT stmt+ DEDENT`) or an inline run of `;`-
    /// separated statements terminated by a logical newline.
    fn block(&mut self) {
        if self.eat(Newline) {
            if self.at(Indent) {
                let m = self.open();
                self.advance(); // INDENT
                while !self.at(Dedent) && !self.eof() {
                    if self.at_any(&[Newline, Semicolon]) {
                        self.advance();
                        continue;
                    }
                    self.stmt();
                }
                self.eat(Dedent);
                self.close(m, Block);
            }
            // else: an empty body (no indentation followed).
        } else {
            // Inline body. A `Dedent` (emitted at EOF / when a surrounding block closes,
            // e.g. on truncated input with an open bracket) also terminates it — without
            // this the loop could spin on a `Dedent` that `stmt` can't consume.
            let m = self.open();
            while !self.at_any(&[Newline, Dedent]) && !self.eof() {
                if self.eat(Semicolon) {
                    continue;
                }
                self.stmt();
            }
            self.close(m, Block);
        }
    }

    /// One statement. Does not consume its trailing newline — the enclosing block /
    /// member loop skips terminators.
    fn stmt(&mut self) {
        match self.nth(0) {
            IfKw => self.if_stmt(),
            ForKw => self.for_stmt(),
            WhileKw => self.while_stmt(),
            MatchKw => self.match_stmt(),
            ReturnKw => self.return_stmt(),
            BreakKw => self.simple_stmt(BreakStmt),
            ContinueKw => self.simple_stmt(ContinueStmt),
            PassKw => self.simple_stmt(PassStmt),
            BreakpointKw => self.simple_stmt(BreakpointStmt),
            AssertKw => self.assert_stmt(),
            StaticKw if self.nth(1) == FuncKw => self.func_decl(),
            VarKw | StaticKw => self.var_decl(),
            ConstKw => self.const_decl(),
            _ => self.expr_stmt(),
        }
    }

    fn simple_stmt(&mut self, kind: SyntaxKind) {
        let m = self.open();
        self.advance();
        self.close(m, kind);
    }

    fn return_stmt(&mut self) {
        let m = self.open();
        self.expect(ReturnKw);
        if !self.at_any(&[Newline, Dedent, Semicolon]) && !self.eof() {
            self.expr();
        }
        self.close(m, ReturnStmt);
    }

    fn assert_stmt(&mut self) {
        let m = self.open();
        self.expect(AssertKw);
        if self.at(LParen) {
            self.arg_list();
        }
        self.close(m, AssertStmt);
    }

    fn if_stmt(&mut self) {
        let m = self.open();
        self.expect(IfKw);
        self.expr();
        self.expect(Colon);
        self.block();
        while self.at(ElifKw) {
            let e = self.open();
            self.advance();
            self.expr();
            self.expect(Colon);
            self.block();
            self.close(e, ElifClause);
        }
        if self.at(ElseKw) {
            let e = self.open();
            self.advance();
            self.expect(Colon);
            self.block();
            self.close(e, ElseClause);
        }
        self.close(m, IfStmt);
    }

    fn for_stmt(&mut self) {
        let m = self.open();
        self.expect(ForKw);
        self.opt_name();
        if self.eat(Colon) {
            self.type_ref();
        }
        self.expect(InKw);
        self.expr();
        self.expect(Colon);
        self.block();
        self.close(m, ForStmt);
    }

    fn while_stmt(&mut self) {
        let m = self.open();
        self.expect(WhileKw);
        self.expr();
        self.expect(Colon);
        self.block();
        self.close(m, WhileStmt);
    }

    fn match_stmt(&mut self) {
        let m = self.open();
        self.expect(MatchKw);
        self.expr();
        self.expect(Colon);
        if self.eat(Newline) && self.at(Indent) {
            self.advance(); // INDENT
            while !self.at(Dedent) && !self.eof() {
                if self.at_any(&[Newline, Semicolon]) {
                    self.advance();
                    continue;
                }
                self.match_arm();
            }
            self.eat(Dedent);
        }
        self.close(m, MatchStmt);
    }

    fn match_arm(&mut self) {
        let m = self.open();
        self.pattern();
        while self.eat(Comma) {
            if self.at(Colon) || self.at(WhenKw) {
                break;
            }
            self.pattern();
        }
        if self.at(WhenKw) {
            let g = self.open();
            self.advance();
            self.expr();
            self.close(g, PatternGuard);
        }
        self.expect(Colon);
        self.block();
        self.close(m, MatchArm);
    }

    fn pattern(&mut self) {
        match self.nth(0) {
            VarKw => {
                let m = self.open();
                self.advance();
                self.opt_name();
                self.close(m, PatternBind);
            }
            DotDot => {
                let m = self.open();
                self.advance();
                self.close(m, PatternRest);
            }
            LBrack => {
                let m = self.open();
                self.advance();
                while !self.at(RBrack) && !self.eof() {
                    self.pattern();
                    if !self.eat(Comma) {
                        break;
                    }
                }
                self.expect(RBrack);
                self.close(m, PatternArray);
            }
            LBrace => {
                let m = self.open();
                self.advance();
                while !self.at(RBrace) && !self.eof() {
                    self.pattern();
                    if self.eat(Colon) {
                        self.pattern();
                    }
                    if !self.eat(Comma) {
                        break;
                    }
                }
                self.expect(RBrace);
                self.close(m, PatternDict);
            }
            _ => {
                let m = self.open();
                self.expr();
                self.close(m, PatternLiteral);
            }
        }
    }

    fn expr_stmt(&mut self) {
        let m = self.open();
        if self.expr().is_none() && !self.at_any(&[Newline, Dedent, Semicolon]) && !self.eof() {
            // Nothing parsed and we're on an unexpected token: recover by skipping it.
            if !self.at_any(RECOVERY) {
                self.advance_with_error("expected a statement");
            }
        }
        self.close(m, ExprStmt);
    }

    // ---- expressions (Pratt) --------------------------------------------------

    /// Parse a full expression (discarding the marker).
    fn expr(&mut self) -> Option<MarkClosed> {
        self.expr_bp(0)
    }

    /// Pratt loop: parse an operand, then fold in infix operators whose left binding
    /// power is at least `min_bp`.
    fn expr_bp(&mut self, min_bp: u8) -> Option<MarkClosed> {
        let mut lhs = self.lhs()?;
        loop {
            let op = self.nth(0);

            // ternary `a if c else b` (right-assoc)
            if op == IfKw {
                let (l, r) = bp(PREC_TERNARY, Assoc::Right);
                if l < min_bp {
                    break;
                }
                let m = self.open_before(lhs);
                self.advance(); // if
                self.expr_bp(0); // condition
                self.expect(ElseKw);
                self.expr_bp(r);
                lhs = self.close(m, TernaryExpr);
                continue;
            }

            // `is` / `is not` (rhs is a type)
            if op == IsKw {
                let (l, _) = bp(PREC_TYPE_TEST, Assoc::Left);
                if l < min_bp {
                    break;
                }
                let m = self.open_before(lhs);
                self.advance(); // is
                self.eat(NotKw);
                self.type_ref();
                lhs = self.close(m, IsExpr);
                continue;
            }

            // `as` (rhs is a type)
            if op == AsKw {
                let (l, _) = bp(PREC_CAST, Assoc::Left);
                if l < min_bp {
                    break;
                }
                let m = self.open_before(lhs);
                self.advance(); // as
                self.type_ref();
                lhs = self.close(m, CastExpr);
                continue;
            }

            // `in` / `not in`
            if op == InKw || (op == NotKw && self.nth(1) == InKw) {
                let (l, r) = bp(PREC_IN, Assoc::Left);
                if l < min_bp {
                    break;
                }
                let m = self.open_before(lhs);
                if op == NotKw {
                    self.advance(); // not
                }
                self.expect(InKw);
                self.expr_bp(r);
                lhs = self.close(m, InExpr);
                continue;
            }

            let Some((prec, assoc)) = infix_prec(op) else {
                break;
            };
            let (l, r) = bp(prec, assoc);
            if l < min_bp {
                break;
            }
            let m = self.open_before(lhs);
            self.advance(); // operator
            self.expr_bp(r);
            lhs = self.close(m, BinExpr);
        }
        Some(lhs)
    }

    /// An operand: a prefix-unary expression, or a primary followed by postfixes.
    fn lhs(&mut self) -> Option<MarkClosed> {
        let op = self.nth(0);
        let prefix = match op {
            NotKw | Bang => Some(PREC_NOT),
            Minus | Plus => Some(PREC_SIGN),
            Tilde => Some(PREC_BIT_NOT),
            AwaitKw => Some(PREC_AWAIT),
            _ => None,
        };
        if let Some(prec) = prefix {
            let m = self.open();
            self.advance();
            self.expr_bp(2 * prec);
            let kind = if op == AwaitKw { AwaitExpr } else { UnaryExpr };
            return Some(self.close(m, kind));
        }
        let primary = self.primary()?;
        Some(self.postfix(primary))
    }

    /// Postfix chain: calls `()`, indexing `[]`, and field access `.`.
    fn postfix(&mut self, lhs: MarkClosed) -> MarkClosed {
        let mut lhs = lhs;
        loop {
            match self.nth(0) {
                LParen => {
                    let m = self.open_before(lhs);
                    self.arg_list();
                    lhs = self.close(m, CallExpr);
                }
                LBrack => {
                    let m = self.open_before(lhs);
                    self.advance();
                    self.expr();
                    self.expect(RBrack);
                    lhs = self.close(m, IndexExpr);
                }
                Dot => {
                    let m = self.open_before(lhs);
                    self.advance();
                    if self.at(Ident) {
                        let n = self.open();
                        self.advance();
                        self.close(n, NameRef);
                    } else {
                        self.error("expected a member name".to_owned());
                    }
                    lhs = self.close(m, FieldExpr);
                }
                _ => break,
            }
        }
        lhs
    }

    /// A primary expression. Returns `None` (without consuming a terminator) when no
    /// expression is present, so callers can recover.
    fn primary(&mut self) -> Option<MarkClosed> {
        match self.nth(0) {
            Int | Float | String | StringName | NodePath | True | False | Null | ConstPi
            | ConstTau | ConstInf | ConstNan => {
                let m = self.open();
                self.advance();
                Some(self.close(m, Literal))
            }
            Ident | SelfKw | SuperKw => {
                let m = self.open();
                self.advance();
                Some(self.close(m, NameRef))
            }
            LParen => {
                let m = self.open();
                self.advance();
                self.expr();
                self.expect(RParen);
                Some(self.close(m, ParenExpr))
            }
            LBrack => Some(self.array_lit()),
            LBrace => Some(self.dict_lit()),
            Dollar => Some(self.get_node(GetNodeExpr, Dollar)),
            Percent => Some(self.get_node(UniqueNodeExpr, Percent)),
            FuncKw => Some(self.lambda()),
            PreloadKw => Some(self.preload_expr()),
            _ => {
                if self.at_any(&[Newline, Dedent, RParen, RBrack, RBrace, Comma]) || self.eof() {
                    self.error("expected an expression".to_owned());
                    None
                } else {
                    Some(self.advance_with_error("expected an expression"))
                }
            }
        }
    }

    fn array_lit(&mut self) -> MarkClosed {
        let m = self.open();
        self.expect(LBrack);
        while !self.at(RBrack) && !self.eof() {
            if self.expr().is_none() {
                break;
            }
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RBrack);
        self.close(m, ArrayLit)
    }

    fn dict_lit(&mut self) -> MarkClosed {
        let m = self.open();
        self.expect(LBrace);
        while !self.at(RBrace) && !self.eof() {
            let e = self.open();
            self.expr();
            if self.eat(Colon) || self.eat(Eq) {
                self.expr();
            }
            self.close(e, DictEntry);
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RBrace);
        self.close(m, DictLit)
    }

    /// `$Path` / `%Unique`: a sigil followed by a string or an identifier path.
    fn get_node(&mut self, node: SyntaxKind, sigil: SyntaxKind) -> MarkClosed {
        let m = self.open();
        self.expect(sigil);
        if self.eat(String) {
            // `$"Path/With Spaces"`
        } else if self.eat(Ident) {
            while self.eat(Slash) {
                self.eat(Ident);
            }
        }
        self.close(m, node)
    }

    fn lambda(&mut self) -> MarkClosed {
        let m = self.open();
        self.expect(FuncKw);
        self.opt_name();
        self.param_list();
        if self.eat(Arrow) {
            self.type_ref();
        }
        self.expect(Colon);
        self.block();
        self.close(m, LambdaExpr)
    }

    fn preload_expr(&mut self) -> MarkClosed {
        let m = self.open();
        self.expect(PreloadKw);
        if self.at(LParen) {
            self.arg_list();
        }
        self.close(m, PreloadExpr)
    }

    /// `'(' (expr (',' expr)*)? ')'`
    fn arg_list(&mut self) {
        let m = self.open();
        self.expect(LParen);
        while !self.at(RParen) && !self.eof() {
            if self.expr().is_none() {
                break;
            }
            if !self.eat(Comma) {
                break;
            }
        }
        self.expect(RParen);
        self.close(m, ArgList);
    }
}

/// Binary (non-special) operator precedence + associativity.
fn infix_prec(op: SyntaxKind) -> Option<(u8, Assoc)> {
    let p = match op {
        Eq | PlusEq | MinusEq | StarEq | SlashEq | StarStarEq | PercentEq | AmpEq | PipeEq
        | CaretEq | ShlEq | ShrEq => return Some((PREC_ASSIGN, Assoc::Right)),
        OrKw | PipePipe => PREC_OR,
        AndKw | AmpAmp => PREC_AND,
        EqEq | Neq | Lt | Gt | Le | Ge => PREC_CMP,
        Pipe => PREC_BIT_OR,
        Caret => PREC_BIT_XOR,
        Amp => PREC_BIT_AND,
        Shl | Shr => PREC_SHIFT,
        Plus | Minus => PREC_ADD,
        Star | Slash | Percent => PREC_FACTOR,
        StarStar => PREC_POWER, // left-assoc in GDScript
        _ => return None,
    };
    Some((p, Assoc::Left))
}
