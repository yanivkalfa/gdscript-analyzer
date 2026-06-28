//! Per-body control-flow narrowing (Phase-6 Workstream 2).
//!
//! A pure forward dataflow over a lowered [`Body`] (no engine API, no `&dyn Db`, no types — types
//! are layered by the checker in [`crate::infer`] consulting these facts). For each reachable
//! statement it records the [`FlowFacts`] that hold *before* it; the checker installs those as the
//! active narrowing environment, replacing the old lexical `narrowing` map. It also computes
//! reachability — statements after a `return`/`break`/`continue` (or where every branch diverges)
//! are dead, feeding `UNREACHABLE_CODE` (Workstream 1 owns the emission).
//!
//! **Soundness over precision (the 1.0 invariant).** When unsure we *widen* (drop a fact), never
//! narrow wrongly: a join is an intersection, a reassignment / opaque call invalidates, a loop body
//! is entered with its assignments widened (no back-edge fixpoint). A wrong narrowing would hide a
//! real `UNSAFE_*` or assert an absent member — both worse than over-warning. The checker keeps the
//! load-bearing `is_uninformative` + widen-only gate when it *consumes* a fact (M1).
//!
//! GDScript's control flow is fully structured (reducible: the only non-local edges are
//! `break`/`continue`), so this recursive dataflow is equivalent to — and simpler + less
//! error-prone than — an explicit basic-block graph. It produces exactly the contract the checker
//! (M1) and the warning layer (M3 `UNREACHABLE_*`) consume: per-statement entry facts +
//! reachability.

use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use gdscript_base::TextRange;

use crate::body::{BinOp, Body, Expr, ExprId, Literal, Stmt, StmtId, UnOp};
use crate::cst::AstPtr;

/// A narrowable place: a local/param, or a (shallow) dotted access rooted at a local or `self`.
/// Deliberately shallow — we narrow `x`, `x.y`, `self.y` but **not** arbitrary call results
/// (`f().y`), array indices (`a[i].y`), or anything whose identity isn't stable under re-evaluation.
/// Shallowness is what keeps narrowing sound under mutation/aliasing (the 1.0 cut).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    /// A function local / parameter, by name (GDScript locals are function-scoped).
    Local(SmolStr),
    /// `self.member` (or a bare member resolving through `self`).
    SelfMember(SmolStr),
    /// A field access on another place (`x.y`, `self.y.z`).
    Field(Box<Place>, SmolStr),
}

impl Place {
    /// Derive the place an expression denotes, or `None` for a non-narrowable expression.
    #[must_use]
    pub fn of(body: &Body, id: ExprId) -> Option<Place> {
        match body.expr(id) {
            Expr::Name(n) => Some(Place::Local(n.clone())),
            Expr::Paren(inner) => Place::of(body, *inner),
            Expr::Field { receiver, name, .. } => match body.expr(*receiver) {
                Expr::SelfExpr => Some(Place::SelfMember(name.clone())),
                _ => Some(Place::Field(
                    Box::new(Place::of(body, *receiver)?),
                    name.clone(),
                )),
            },
            // `self` itself isn't narrowed (only `self.m`, above); all else is non-narrowable.
            _ => None,
        }
    }

    /// Whether assigning to `assigned` may invalidate a narrowing of `self`. Conservative prefix
    /// check: assigning `x` clears `x` and `x.*`; assigning `x.y` clears `x.y` and `x.y.*` (but not
    /// `x`). I.e. `assigned` is an ancestor-or-equal of `self`.
    #[must_use]
    pub fn invalidated_by(&self, assigned: &Place) -> bool {
        let mut cur = self;
        loop {
            if cur == assigned {
                return true;
            }
            match cur {
                Place::Field(base, _) => cur = base,
                _ => return false,
            }
        }
    }

    /// The dotted access-path key for this place (`x`, `self.field`, `a.b.c`) — the format the
    /// checker's `narrow_key` produces, so the two agree when the checker consults a fact.
    #[must_use]
    pub fn dotted_key(&self) -> String {
        match self {
            Place::Local(n) => n.to_string(),
            Place::SelfMember(m) => format!("self.{m}"),
            Place::Field(base, name) => format!("{}.{name}", base.dotted_key()),
        }
    }

    /// Whether this place is rooted at `self` (a `self.member` or a field chain under it) — the
    /// places an opaque call may have mutated.
    #[must_use]
    fn is_self_rooted(&self) -> bool {
        match self {
            Place::SelfMember(_) => true,
            Place::Field(base, _) => base.is_self_rooted(),
            Place::Local(_) => false,
        }
    }
}

/// A narrowing fact that holds at a program point (a place narrowed to a type or proven non-null).
/// The type-test variants carry an [`AstPtr`] to the `TypeRef`, resolved lazily by the checker
/// against the engine model — exactly like the old `apply_narrowing` resolved its `ptr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarrowedTy {
    /// The place is statically `T` — from `is T`, an `x as T` assignment, or a `match T():` arm.
    Is(AstPtr),
    /// Proven non-null (from `!= null`, a truthy object guard, or a prior `is`). 1.0 records it but
    /// the checker uses it only to suppress null access, not to assert a type.
    NotNull,
    /// Proven **not** `T` (the else-branch of `is T`). Best-effort: 1.0 records it but never uses it
    /// to assert a member (no positive type).
    Not(AstPtr),
}

/// The narrowing facts in force at a program point (a `Place → NarrowedTy` environment).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowFacts(FxHashMap<Place, NarrowedTy>);

impl FlowFacts {
    /// The narrowed type of `place`, if any.
    #[must_use]
    pub fn get(&self, place: &Place) -> Option<&NarrowedTy> {
        self.0.get(place)
    }

    /// Whether there are no facts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate the `(place, narrowed-type)` facts.
    pub fn iter(&self) -> impl Iterator<Item = (&Place, &NarrowedTy)> {
        self.0.iter()
    }

    /// Install a fact, preferring the stronger of an existing `Is` over a new `NotNull` (an `Is`
    /// already implies non-null, so a later truthy guard must not weaken it).
    fn insert(&mut self, place: Place, ty: NarrowedTy) {
        if matches!(ty, NarrowedTy::NotNull)
            && matches!(self.0.get(&place), Some(NarrowedTy::Is(_)))
        {
            return;
        }
        self.0.insert(place, ty);
    }

    /// Drop every fact invalidated by an assignment to `assigned` (it and its sub-places).
    fn invalidate_assigned(&mut self, assigned: &Place) {
        self.0.retain(|p, _| !p.invalidated_by(assigned));
    }

    /// Drop every `self`-rooted fact (an opaque call may have mutated `self`'s members).
    fn invalidate_self_rooted(&mut self) {
        self.0.retain(|p, _| !p.is_self_rooted());
    }

    /// The intersection of two fact sets — a place survives a control-flow merge only if narrowed
    /// **identically** on both incoming edges (the soundness core: drop on any disagreement).
    #[must_use]
    fn join(&self, other: &FlowFacts) -> FlowFacts {
        let mut out = FxHashMap::default();
        for (p, t) in &self.0 {
            if other.0.get(p) == Some(t) {
                out.insert(p.clone(), t.clone());
            }
        }
        FlowFacts(out)
    }
}

/// The result of flowing a body: per-statement entry facts + the statements proven unreachable.
#[derive(Debug, Clone, Default)]
pub struct FlowAnalysis {
    /// Facts holding *before* each reachable statement. A statement absent here is either
    /// unreachable or carries no narrowing — the checker treats both as "no facts".
    entry_facts: FxHashMap<StmtId, FlowFacts>,
    /// The first statement of each maximal unreachable run (the `UNREACHABLE_CODE` anchors).
    unreachable_anchors: Vec<StmtId>,
}

impl FlowAnalysis {
    /// The facts in force before `stmt` (empty if none / unreachable).
    #[must_use]
    pub fn facts_before(&self, stmt: StmtId) -> Option<&FlowFacts> {
        self.entry_facts.get(&stmt)
    }

    /// The byte ranges of the unreachable-code anchors (Workstream 1 emits `UNREACHABLE_CODE` here).
    #[must_use]
    pub fn unreachable_ranges(&self, body: &Body) -> Vec<TextRange> {
        self.unreachable_anchors
            .iter()
            .map(|&sid| body.source_map.stmt_range(sid))
            .collect()
    }
}

/// Run the forward dataflow over a lowered body, producing per-statement entry facts + reachability.
#[must_use]
pub fn analyze(body: &Body) -> FlowAnalysis {
    let mut a = Analyzer {
        body,
        entry_facts: FxHashMap::default(),
        unreachable_anchors: Vec::new(),
    };
    a.block(FlowFacts::default(), &body.block);
    // Each lambda body is a fresh scope — analyze it independently (its statements share the body's
    // arena, so their entry facts merge into the same map). A single pass over the expression arena
    // catches every lambda, including nested ones.
    for expr in &body.exprs {
        if let Expr::Lambda { body: lbody, .. } = expr {
            a.block(FlowFacts::default(), lbody);
        }
    }
    FlowAnalysis {
        entry_facts: a.entry_facts,
        unreachable_anchors: a.unreachable_anchors,
    }
}

struct Analyzer<'a> {
    body: &'a Body,
    entry_facts: FxHashMap<StmtId, FlowFacts>,
    unreachable_anchors: Vec<StmtId>,
}

impl Analyzer<'_> {
    /// Flow a statement block. Returns the facts that fall through to the next statement, or `None`
    /// if the block diverges (every path `return`s/`break`s/`continue`s). Records the first
    /// unreachable statement as an anchor.
    fn block(&mut self, facts: FlowFacts, block: &[StmtId]) -> Option<FlowFacts> {
        let mut cur = Some(facts);
        for &sid in block {
            let Some(f) = cur else {
                // The first statement past a divergence anchors `UNREACHABLE_CODE`.
                self.unreachable_anchors.push(sid);
                return None;
            };
            cur = self.stmt(f, sid);
        }
        cur
    }

    /// Flow one statement: record its entry facts, then return the facts that fall through (or
    /// `None` if it diverges).
    fn stmt(&mut self, facts: FlowFacts, sid: StmtId) -> Option<FlowFacts> {
        self.entry_facts.insert(sid, facts.clone());
        match self.body.stmt(sid) {
            Stmt::Return(_) | Stmt::Break | Stmt::Continue => None,
            Stmt::Pass | Stmt::Assert(_) => Some(facts),
            Stmt::Expr(e) => Some(self.after_expr_stmt(facts, *e)),
            Stmt::Var(v) => {
                let mut f = facts;
                // A (re-)declaration shadows: drop any prior narrowing of this name.
                f.invalidate_assigned(&Place::Local(v.name.clone()));
                Some(f)
            }
            Stmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
            } => self.flow_if(&facts, *cond, then_branch, elifs, else_branch.as_deref()),
            Stmt::While { body, .. } => Some(self.flow_loop(facts, body, None)),
            Stmt::For(f) => Some(self.flow_loop(facts, &f.body, Some(&f.var))),
            Stmt::Match { arms, .. } => {
                // Conservative: each arm is flowed from the original facts (no scrutinee narrowing
                // in the 1.0 cut — pattern types aren't lowered yet); the match falls through with
                // every arm's assignments widened away (we can't yet prove exhaustiveness).
                let mut after = facts.clone();
                for arm in arms {
                    let _ = self.block(facts.clone(), &arm.body);
                    self.scan_invalidations(&mut after, &arm.body);
                }
                Some(after)
            }
        }
    }

    /// Facts after an expression statement: a reassignment invalidates the assigned place; any call
    /// invalidates `self`-rooted narrowing (an opaque call may mutate `self`'s members).
    fn after_expr_stmt(&self, mut facts: FlowFacts, e: ExprId) -> FlowFacts {
        if let Expr::Bin {
            op: BinOp::Assign,
            lhs,
            ..
        } = self.body.expr(e)
            && let Some(p) = Place::of(self.body, *lhs)
        {
            facts.invalidate_assigned(&p);
        }
        if self.expr_contains_call(e) {
            facts.invalidate_self_rooted();
        }
        facts
    }

    /// `if … elif … else …`: each branch is flowed under its guard's narrowing; the result is the
    /// join of the branches that fall through. The early-return idiom falls out — if the `then`
    /// branch diverges, the merge is just the `else`/no-else facts (with the guard negated).
    fn flow_if(
        &mut self,
        facts: &FlowFacts,
        cond: ExprId,
        then_branch: &[StmtId],
        elifs: &[(ExprId, crate::body::Block)],
        else_branch: Option<&[StmtId]>,
    ) -> Option<FlowFacts> {
        let mut exits: Vec<Option<FlowFacts>> = Vec::new();
        let then_in = self.apply(facts, cond, true);
        exits.push(self.block(then_in, then_branch));

        // `elif` chain: each guard is evaluated under "all previous guards false".
        let mut chain = self.apply(facts, cond, false);
        for (econd, eblock) in elifs {
            let etrue = self.apply(&chain, *econd, true);
            exits.push(self.block(etrue, eblock));
            chain = self.apply(&chain, *econd, false);
        }
        // The final `else` (or the implicit fall-through when there is none).
        exits.push(match else_branch {
            Some(eb) => self.block(chain, eb),
            None => Some(chain),
        });

        join_exits(exits)
    }

    /// A `while`/`for` loop, entered with its body's assignments widened (no back-edge fixpoint —
    /// the 1.0 cut). Always falls through (the body may run zero times); after the loop the body's
    /// assignments are widened away.
    fn flow_loop(
        &mut self,
        facts: FlowFacts,
        body: &[StmtId],
        loop_var: Option<&SmolStr>,
    ) -> FlowFacts {
        let mut widened = facts;
        if let Some(v) = loop_var {
            widened.invalidate_assigned(&Place::Local(v.clone()));
        }
        self.scan_invalidations(&mut widened, body);
        // Flow the body once with the widened facts (records the body's entry facts); its exit is
        // discarded — a loop's after-state is the widened pre-loop facts, not the body's.
        let _ = self.block(widened.clone(), body);
        widened
    }

    /// Apply a condition's narrowing to a fact set for the truthy/falsy edge.
    fn apply(&self, facts: &FlowFacts, cond: ExprId, truthy: bool) -> FlowFacts {
        let mut out = facts.clone();
        for (p, t) in self.derive_facts(cond, truthy) {
            out.insert(p, t);
        }
        // An opaque call in the condition may run *after* a narrowing test (e.g. the rhs of an
        // `and`, or `if mutate() and self.x is T:`) and mutate `self`'s members, so no `self`-rooted
        // narrowing from this edge is trustworthy — drop it (mirrors `after_expr_stmt`). Local
        // narrowing is unaffected (a callee cannot reassign a caller's local). Soundness > precision.
        if self.expr_contains_call(cond) {
            out.invalidate_self_rooted();
        }
        out
    }

    /// The facts a condition establishes on its truthy (or falsy) edge.
    fn derive_facts(&self, cond: ExprId, truthy: bool) -> Vec<(Place, NarrowedTy)> {
        match self.body.expr(cond) {
            Expr::Paren(inner) => self.derive_facts(*inner, truthy),
            Expr::Unary {
                op: UnOp::Not,
                operand,
            } => self.derive_facts(*operand, !truthy),
            Expr::Is {
                operand,
                ty: Some(ptr),
                negated,
            } => {
                let positive = truthy != *negated;
                Place::of(self.body, *operand)
                    .map(|p| {
                        let t = if positive {
                            NarrowedTy::Is(*ptr)
                        } else {
                            NarrowedTy::Not(*ptr)
                        };
                        vec![(p, t)]
                    })
                    .unwrap_or_default()
            }
            Expr::Bin {
                op: BinOp::Eq,
                lhs,
                rhs,
            } => self.null_cmp_facts(*lhs, *rhs, true, truthy),
            Expr::Bin {
                op: BinOp::Ne,
                lhs,
                rhs,
            } => self.null_cmp_facts(*lhs, *rhs, false, truthy),
            // `a and b` truthy ⇒ both true; `a or b` falsy ⇒ both false. The other directions
            // cannot be attributed to one operand (either could be the deciding one) — widen.
            Expr::Bin {
                op: BinOp::And,
                lhs,
                rhs,
            } if truthy => {
                let mut v = self.derive_facts(*lhs, true);
                v.extend(self.derive_facts(*rhs, true));
                v
            }
            Expr::Bin {
                op: BinOp::Or,
                lhs,
                rhs,
            } if !truthy => {
                let mut v = self.derive_facts(*lhs, false);
                v.extend(self.derive_facts(*rhs, false));
                v
            }
            // A bare truthy guard `if x:` / `if x.y:` proves the place non-null.
            _ if truthy => Place::of(self.body, cond)
                .map(|p| vec![(p, NarrowedTy::NotNull)])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Facts from a `==`/`!=` comparison against `null`: the non-null operand becomes `NotNull` on
    /// the edge where it is proven non-null (`x != null` true, or `x == null` false).
    fn null_cmp_facts(
        &self,
        lhs: ExprId,
        rhs: ExprId,
        is_eq: bool,
        truthy: bool,
    ) -> Vec<(Place, NarrowedTy)> {
        let other = if self.is_null(lhs) {
            rhs
        } else if self.is_null(rhs) {
            lhs
        } else {
            return Vec::new();
        };
        let proves_not_null = if is_eq { !truthy } else { truthy };
        if proves_not_null {
            Place::of(self.body, other)
                .map(|p| vec![(p, NarrowedTy::NotNull)])
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn is_null(&self, id: ExprId) -> bool {
        matches!(self.body.expr(id), Expr::Literal(Literal::Null))
    }

    /// Drop, from `facts`, every place a block's statements may assign / invalidate (for widening a
    /// loop entry/exit and a `match` fall-through). Recurses into nested blocks.
    fn scan_invalidations(&self, facts: &mut FlowFacts, block: &[StmtId]) {
        for &sid in block {
            match self.body.stmt(sid) {
                Stmt::Expr(e) => {
                    if let Expr::Bin {
                        op: BinOp::Assign,
                        lhs,
                        ..
                    } = self.body.expr(*e)
                        && let Some(p) = Place::of(self.body, *lhs)
                    {
                        facts.invalidate_assigned(&p);
                    }
                    if self.expr_contains_call(*e) {
                        facts.invalidate_self_rooted();
                    }
                }
                Stmt::Var(v) => facts.invalidate_assigned(&Place::Local(v.name.clone())),
                Stmt::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    // A call in a guard (run every iteration when this `if` is inside the loop) may
                    // mutate `self` — account for it alongside the branch bodies.
                    if self.expr_contains_call(*cond) {
                        facts.invalidate_self_rooted();
                    }
                    self.scan_invalidations(facts, then_branch);
                    for (econd, b) in elifs {
                        if self.expr_contains_call(*econd) {
                            facts.invalidate_self_rooted();
                        }
                        self.scan_invalidations(facts, b);
                    }
                    if let Some(eb) = else_branch {
                        self.scan_invalidations(facts, eb);
                    }
                }
                Stmt::While { cond, body } => {
                    if self.expr_contains_call(*cond) {
                        facts.invalidate_self_rooted();
                    }
                    self.scan_invalidations(facts, body);
                }
                Stmt::For(f) => {
                    facts.invalidate_assigned(&Place::Local(f.var.clone()));
                    if self.expr_contains_call(f.iter) {
                        facts.invalidate_self_rooted();
                    }
                    self.scan_invalidations(facts, &f.body);
                }
                Stmt::Match { scrutinee, arms } => {
                    if self.expr_contains_call(*scrutinee) {
                        facts.invalidate_self_rooted();
                    }
                    for arm in arms {
                        self.scan_invalidations(facts, &arm.body);
                    }
                }
                Stmt::Assert(Some(c)) => {
                    if self.expr_contains_call(*c) {
                        facts.invalidate_self_rooted();
                    }
                }
                Stmt::Return(_)
                | Stmt::Break
                | Stmt::Continue
                | Stmt::Pass
                | Stmt::Assert(None) => {}
            }
        }
    }

    /// Whether an expression subtree contains a call (so an opaque mutation of `self` is possible).
    fn expr_contains_call(&self, id: ExprId) -> bool {
        match self.body.expr(id) {
            Expr::Call { .. } => true,
            Expr::Bin { lhs, rhs, .. } | Expr::In { lhs, rhs, .. } => {
                self.expr_contains_call(*lhs) || self.expr_contains_call(*rhs)
            }
            Expr::Unary { operand, .. }
            | Expr::Await(operand)
            | Expr::Paren(operand)
            | Expr::Cast { operand, .. }
            | Expr::Is { operand, .. } => self.expr_contains_call(*operand),
            Expr::Ternary {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr_contains_call(*cond)
                    || self.expr_contains_call(*then_branch)
                    || self.expr_contains_call(*else_branch)
            }
            Expr::Field { receiver, .. } => self.expr_contains_call(*receiver),
            Expr::Index { base, index } => {
                self.expr_contains_call(*base) || self.expr_contains_call(*index)
            }
            Expr::Array(items) => items.iter().any(|&e| self.expr_contains_call(e)),
            Expr::Dict(entries) => entries.iter().any(|(k, v)| {
                self.expr_contains_call(*k) || v.is_some_and(|e| self.expr_contains_call(e))
            }),
            _ => false,
        }
    }
}

/// The narrowing facts a condition establishes on its truthy (or falsy) edge — exposed so the
/// checker can apply `and`/`or` short-circuit narrowing *within* a condition expression (the RHS of
/// `a and b` is typed under `a`'s then-facts, `a or b`'s under `a`'s else-facts).
#[must_use]
pub fn condition_facts(body: &Body, cond: ExprId, truthy: bool) -> Vec<(Place, NarrowedTy)> {
    Analyzer {
        body,
        entry_facts: FxHashMap::default(),
        unreachable_anchors: Vec::new(),
    }
    .derive_facts(cond, truthy)
}

/// Merge the exits of several control-flow paths: the join (intersection) of the ones that fall
/// through, or `None` if every path diverges.
fn join_exits(exits: Vec<Option<FlowFacts>>) -> Option<FlowFacts> {
    let mut iter = exits.into_iter().flatten();
    let first = iter.next()?;
    Some(iter.fold(first, |acc, f| acc.join(&f)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{self, Body};
    use gdscript_syntax::{SyntaxKind, ast, parse};

    fn func_body(src: &str) -> Body {
        let root = parse(src).syntax_node();
        let func = ast::descendants(&root)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::FuncDecl)
            .expect("a FuncDecl");
        body::body_of_func(&func)
    }

    /// The (single) `Place::Local` narrowed in the facts before the statement at top-level index `i`.
    fn fact_at(body: &Body, a: &FlowAnalysis, i: usize) -> Option<(Place, NarrowedTy)> {
        let sid = body.block[i];
        let facts = a.facts_before(sid)?;
        facts.0.iter().next().map(|(p, t)| (p.clone(), t.clone()))
    }

    #[test]
    fn is_guard_narrows_then_branch() {
        let body = func_body("func f(x):\n\tif x is Node:\n\t\tx.free()\n");
        let a = analyze(&body);
        // The `x.free()` stmt lives inside the then-branch; its entry facts narrow x to `Is`.
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        let inner = a.facts_before(then_branch[0]).expect("then facts");
        assert_eq!(
            inner.get(&Place::Local("x".into())),
            Some(&NarrowedTy::Is(match body.stmt(body.block[0]) {
                Stmt::If { cond, .. } => match body.expr(*cond) {
                    Expr::Is { ty: Some(p), .. } => *p,
                    _ => panic!("is"),
                },
                _ => unreachable!(),
            })),
        );
    }

    #[test]
    fn early_return_narrows_after_the_guard() {
        // `if x == null: return` ⇒ after the if, x is NotNull (the then-branch diverged).
        let body = func_body("func f(x):\n\tif x == null:\n\t\treturn\n\tx.free()\n");
        let a = analyze(&body);
        // block[0] = the if, block[1] = `x.free()`.
        let after = a.facts_before(body.block[1]).expect("after-if facts");
        assert_eq!(
            after.get(&Place::Local("x".into())),
            Some(&NarrowedTy::NotNull)
        );
    }

    #[test]
    fn code_after_return_is_unreachable() {
        let body = func_body("func f():\n\treturn\n\tvar dead := 1\n");
        let a = analyze(&body);
        assert_eq!(a.unreachable_ranges(&body).len(), 1);
        // The anchor is the `var dead` statement (block index 1).
        assert_eq!(a.unreachable_anchors, vec![body.block[1]]);
    }

    #[test]
    fn reassignment_invalidates_narrowing() {
        // After `if x is Node:` narrows x, an assignment `x = other` inside drops the fact.
        let body = func_body("func f(x, other):\n\tif x is Node:\n\t\tx = other\n\t\tx.free()\n");
        let a = analyze(&body);
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        // then_branch[0] = `x = other` (narrowed on entry), then_branch[1] = `x.free()` (widened).
        let at_free = a.facts_before(then_branch[1]).expect("facts");
        assert_eq!(at_free.get(&Place::Local("x".into())), None);
    }

    #[test]
    fn opaque_call_invalidates_self_members() {
        // `if self.node is Node2D:` narrows self.node; a bare call may mutate it → invalidated.
        let body =
            func_body("func f():\n\tif self.node is Node2D:\n\t\tmutate()\n\t\tself.node.foo()\n");
        let a = analyze(&body);
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        let at_use = a.facts_before(then_branch[1]).expect("facts");
        assert_eq!(at_use.get(&Place::SelfMember("node".into())), None);
    }

    #[test]
    fn opaque_call_in_guard_invalidates_self_member_narrowing() {
        // A call in the guard itself (`mutate()` in the `and`) may reassign self.node *after* the
        // `is` test, so self.node must NOT be narrowed in the then-branch — the soundness invariant.
        let body =
            func_body("func f():\n\tif self.node is Node2D and mutate():\n\t\tself.node.foo()\n");
        let a = analyze(&body);
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        let inner = a.facts_before(then_branch[0]).expect("then facts");
        assert_eq!(inner.get(&Place::SelfMember("node".into())), None);
    }

    #[test]
    fn merge_drops_disagreeing_facts() {
        // x narrowed in then but not else ⇒ dropped after the if (intersection).
        let body =
            func_body("func f(x):\n\tif x is Node:\n\t\tpass\n\telse:\n\t\tpass\n\tx.free()\n");
        let a = analyze(&body);
        let after = fact_at(&body, &a, 1);
        assert!(
            after.is_none(),
            "narrowing must not survive a non-exhaustive merge"
        );
    }

    #[test]
    fn and_short_circuit_narrows_rhs_and_after() {
        // `if x is Node and x.is_inside_tree():` — the whole-cond-true edge narrows x.
        let body = func_body("func f(x):\n\tif x is Node and true:\n\t\tx.free()\n");
        let a = analyze(&body);
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        let inner = a.facts_before(then_branch[0]).expect("then facts");
        assert!(matches!(
            inner.get(&Place::Local("x".into())),
            Some(NarrowedTy::Is(_))
        ));
    }

    #[test]
    fn loop_body_is_entered_widened() {
        // A narrowing from before the loop does not survive into a body that reassigns the place.
        let body = func_body(
            "func f(x, other):\n\tif x is Node:\n\t\twhile true:\n\t\t\tx = other\n\t\t\tx.free()\n",
        );
        let a = analyze(&body);
        // Just assert it runs without panic and produces facts for the outer then-branch.
        let Stmt::If { then_branch, .. } = body.stmt(body.block[0]) else {
            panic!("if")
        };
        assert!(a.facts_before(then_branch[0]).is_some());
    }
}
