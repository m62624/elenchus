//! A compact, single-threaded CDCL SAT solver in `no_std`, replicating the core
//! algorithm of varisat (jix/varisat) in a readable, lazy style.
//!
//! `Solver::run` drives the CDCL loop to a terminal state: it propagates, and on a
//! conflict analyzes/backjumps/learns, otherwise it decides (`Solver::decide`).
//! Model enumeration is a lazy [`Models`] iterator that solves **incrementally** —
//! each `next()` adds a blocking clause and continues from the existing state
//! rather than re-solving from scratch.
//!
//! **Assumptions** ([`solve_assuming`]): literals forced true before VSIDS
//! branching. They are decided first; a contradicted assumption yields an unsat
//! **core** (a sufficient subset of the assumptions) via MiniSat's `analyzeFinal`.
//! This is the primitive behind incremental cores and what-if queries.
//!
//! Pieces mirror varisat's modules: the trail + decision levels
//! (`prop/assignment.rs`), two-watched-literal propagation (`prop/long.rs`),
//! 1-UIP conflict analysis with clause learning (`analyze_conflict.rs`),
//! non-chronological backjumping, VSIDS decisions with phase saving, and
//! assumption-based solving. Remaining infrastructure (proof/DRAT logging,
//! clause-DB GC, restarts, the `partial_ref` context, multithreading) is
//! intentionally omitted.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

// --- literals & formulas ---------------------------------------------------

/// A boolean variable, identified by a dense index.
pub type Var = u32;

/// A literal: a variable plus a sign, packed as `var << 1 | negative`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SatLit(u32);

impl SatLit {
    /// A literal for `var`, positive (true) or negative (`NOT var`).
    pub fn new(var: Var, positive: bool) -> Self {
        SatLit((var << 1) | (!positive as u32))
    }
    /// The positive literal `var`.
    pub fn positive(var: Var) -> Self {
        Self::new(var, true)
    }
    /// The negative literal `NOT var`.
    pub fn negative(var: Var) -> Self {
        Self::new(var, false)
    }
    /// The underlying variable.
    pub fn var(self) -> Var {
        self.0 >> 1
    }
    /// Whether this is the negative polarity.
    pub fn is_negative(self) -> bool {
        self.0 & 1 == 1
    }
    /// The same variable with the opposite sign.
    pub fn negate(self) -> SatLit {
        SatLit(self.0 ^ 1)
    }
    /// The packed code, used directly as an index into the watch lists.
    fn code(self) -> usize {
        self.0 as usize
    }
}

/// A CNF formula over `num_vars` variables.
#[derive(Clone, Debug, Default)]
pub struct Cnf {
    /// Number of variables; every [`Var`] used must be `< num_vars`.
    pub num_vars: usize,
    /// The clauses, each a disjunction of literals (the formula is their AND).
    pub clauses: Vec<Vec<SatLit>>,
}

impl Cnf {
    /// An empty formula over `num_vars` variables.
    pub fn new(num_vars: usize) -> Self {
        Cnf {
            num_vars,
            clauses: Vec::new(),
        }
    }
    /// Append one clause (a disjunction of literals).
    pub fn add_clause(&mut self, lits: Vec<SatLit>) {
        self.clauses.push(lits);
    }
}

// --- internal state --------------------------------------------------------

/// Why a variable was assigned — needed for conflict analysis and backtracking.
#[derive(Clone, Copy)]
enum Reason {
    Decision,
    Unit,
    Long(usize),
}

/// One watched-literal entry: a clause plus a cached "other" literal so a true
/// blocking literal lets us skip the clause entirely.
#[derive(Clone, Copy)]
struct Watch {
    cref: usize,
    blocking: SatLit,
}

/// What the decision phase produced. The search loop reacts to each.
enum Decision {
    /// A literal (an assumption or a VSIDS branch) was enqueued; propagate next.
    Propagated,
    /// Every variable is assigned under the assumptions — satisfiable.
    Sat,
    /// An assumption is contradicted; carries a sufficient core (a subset of the
    /// assumptions). An empty core means UNSAT independent of the assumptions.
    UnsatCore(Vec<SatLit>),
}

/// The full CDCL search state: the assignment trail with decision levels, the
/// clause database with two-watched-literal indices, VSIDS activities with phase
/// saving, and a reusable `seen` scratch buffer for conflict analysis.
struct Solver {
    num_vars: usize,
    clauses: Vec<Vec<SatLit>>, // originals + learned + blocking
    watches: Vec<Vec<Watch>>, // indexed by literal code; a clause watching `w` lives in watches[!w]
    assign: Vec<Option<bool>>, // per var
    level: Vec<u32>,          // per var (valid when assigned)
    reason: Vec<Reason>,      // per var (valid when assigned)
    trail: Vec<SatLit>,
    decisions: Vec<usize>, // trail index where each decision level starts
    qhead: usize,
    activity: Vec<f64>,
    var_inc: f64,
    polarity: Vec<bool>, // phase saving
    seen: Vec<bool>,     // reusable scratch for analyze (invariant: all-false between calls)
    ok: bool,            // false once the formula is known UNSAT
    // Literals forced true before VSIDS branching. Decision levels 1..=len map
    // one-to-one to assumptions[0..]; an already-true assumption still consumes a
    // (dummy) level so that mapping holds. Empty for a plain solve.
    assumptions: Vec<SatLit>,
}

impl Solver {
    /// Build a solver and load every clause of `cnf` under the empty assignment.
    fn new(cnf: &Cnf) -> Self {
        let n = cnf.num_vars;
        let mut s = Solver {
            num_vars: n,
            clauses: Vec::new(),
            watches: vec![Vec::new(); 2 * n],
            assign: vec![None; n],
            level: vec![0; n],
            reason: vec![Reason::Decision; n],
            trail: Vec::new(),
            decisions: Vec::new(),
            qhead: 0,
            activity: vec![0.0; n],
            var_inc: 1.0,
            polarity: vec![false; n],
            seen: vec![false; n],
            ok: true,
            assumptions: Vec::new(),
        };
        for clause in &cnf.clauses {
            s.add_clause(clause);
        }
        s
    }

    // -- assignment queries --

    /// Is `l` currently assigned true? (Unassigned counts as neither true nor false.)
    fn lit_is_true(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(!l.is_negative())
    }
    /// Is `l` currently assigned false?
    fn lit_is_false(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(l.is_negative())
    }
    /// The current decision level (= number of open decisions).
    fn current_level(&self) -> u32 {
        self.decisions.len() as u32
    }

    // -- clause loading --

    /// Register clause `cref` to be watched by literals `a` and `b`. A clause
    /// watching a literal is stored under that literal's *negation's* code, so
    /// it is revisited exactly when the watched literal becomes false.
    fn watch(&mut self, cref: usize, a: SatLit, b: SatLit) {
        self.watches[a.negate().code()].push(Watch { cref, blocking: b });
        self.watches[b.negate().code()].push(Watch { cref, blocking: a });
    }

    /// Attach a clause under the *current* assignment. Both watched literals must
    /// be non-false, or the clause is unit/conflicting and is handled directly.
    /// This is what makes incremental clause addition (blocking clauses added
    /// mid-search, at level 0) correct — naively watching `lits[0..2]` would break
    /// the invariant when one is already false.
    fn add_clause(&mut self, lits: &[SatLit]) {
        if !self.ok {
            return;
        }
        if lits.is_empty() {
            self.ok = false;
            return;
        }
        if lits.len() == 1 {
            let l = lits[0];
            if self.lit_is_false(l) {
                self.ok = false;
            } else if !self.lit_is_true(l) {
                self.enqueue(l, Reason::Unit);
            }
            return;
        }

        // Find up to two non-false literals to watch.
        let mut clause = lits.to_vec();
        let mut first = None;
        let mut second = None;
        for (i, &l) in clause.iter().enumerate() {
            if !self.lit_is_false(l) {
                if first.is_none() {
                    first = Some(i);
                } else {
                    second = Some(i);
                    break;
                }
            }
        }
        let cref = self.clauses.len();
        match (first, second) {
            // Every literal is false under the current assignment → conflict.
            (None, _) => self.ok = false,
            // Exactly one non-false literal → the clause is unit; assert it.
            (Some(a), None) => {
                clause.swap(0, a);
                self.watch(cref, clause[0], clause[1]);
                let unit = clause[0];
                self.clauses.push(clause);
                if !self.lit_is_true(unit) {
                    self.enqueue(unit, Reason::Long(cref));
                }
            }
            // Two non-false literals → watch them (moved to positions 0 and 1).
            (Some(a), Some(b)) => {
                clause.swap(0, a);
                clause.swap(1, b);
                self.watch(cref, clause[0], clause[1]);
                self.clauses.push(clause);
            }
        }
    }

    /// Assign `l` true at the current level with the given `reason`, and push it
    /// onto the trail for propagation.
    fn enqueue(&mut self, l: SatLit, reason: Reason) {
        let v = l.var() as usize;
        self.assign[v] = Some(!l.is_negative());
        self.level[v] = self.current_level();
        self.reason[v] = reason;
        self.trail.push(l);
    }

    // -- propagation (two-watched-literal) --

    /// Unit-propagate to a fixpoint. Returns the conflicting clause, if any.
    fn propagate(&mut self) -> Option<usize> {
        while self.qhead < self.trail.len() {
            let p = self.trail[self.qhead];
            self.qhead += 1;
            if let Some(cref) = self.propagate_lit(p) {
                return Some(cref);
            }
        }
        None
    }

    /// Process the clauses watching `!p` after `p` became true.
    fn propagate_lit(&mut self, p: SatLit) -> Option<usize> {
        let fl = p.negate(); // the watched literal that just became false
        let mut ws = core::mem::take(&mut self.watches[p.code()]);
        let mut read = 0;
        let mut write = 0;
        let mut conflict = None;

        while read < ws.len() {
            let w = ws[read];
            read += 1;

            // A satisfied clause (true blocking literal) needs no inspection.
            if self.lit_is_true(w.blocking) {
                ws[write] = w;
                write += 1;
                continue;
            }

            let cref = w.cref;
            if self.clauses[cref][0] == fl {
                self.clauses[cref].swap(0, 1);
            }
            let other = self.clauses[cref][0];
            let kept = Watch {
                cref,
                blocking: other,
            };

            if other != w.blocking && self.lit_is_true(other) {
                ws[write] = kept;
                write += 1;
                continue;
            }

            // Try to move the watch to a non-false unwatched literal.
            if let Some(repl) = self.find_replacement(cref, fl) {
                self.watches[repl.negate().code()].push(kept);
                continue; // watch left this list
            }

            // No replacement: keep watching `fl`; the clause is unit or conflicting.
            ws[write] = kept;
            write += 1;
            if self.lit_is_false(other) {
                while read < ws.len() {
                    ws[write] = ws[read];
                    write += 1;
                    read += 1;
                }
                conflict = Some(cref);
                break;
            }
            self.enqueue(other, Reason::Long(cref));
        }

        ws.truncate(write);
        self.watches[p.code()] = ws;
        conflict
    }

    /// Find a non-false literal in `clause[2..]`, swap it into the watched slot.
    fn find_replacement(&mut self, cref: usize, fl: SatLit) -> Option<SatLit> {
        let len = self.clauses[cref].len();
        for k in 2..len {
            let ck = self.clauses[cref][k];
            if !self.lit_is_false(ck) {
                self.clauses[cref][1] = ck;
                self.clauses[cref][k] = fl;
                return Some(ck);
            }
        }
        None
    }

    // -- conflict analysis (1-UIP) --

    /// VSIDS: raise variable `v`'s activity, rescaling all activities if it would
    /// overflow `f64`'s comfortable range.
    fn bump(&mut self, v: usize) {
        self.activity[v] += self.var_inc;
        if self.activity[v] > 1e100 {
            for a in &mut self.activity {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }
    }

    /// Learn an asserting clause from `conflict` and return (clause, backjump level).
    /// Uses the reusable `seen` buffer and restores it to all-false on exit.
    fn analyze(&mut self, conflict: usize) -> (Vec<SatLit>, u32) {
        let cur_level = self.current_level();
        let mut learned: Vec<SatLit> = vec![SatLit(0)]; // slot 0 = asserting literal
        let mut touched: Vec<Var> = Vec::new();
        let mut counter = 0usize;
        let mut idx = self.trail.len();
        let mut p: Option<SatLit> = None;
        let mut confl = conflict;

        loop {
            let start = if p.is_some() { 1 } else { 0 }; // a reason clause has p at index 0
            for j in start..self.clauses[confl].len() {
                let q = self.clauses[confl][j];
                let v = q.var() as usize;
                if !self.seen[v] && self.level[v] > 0 {
                    self.seen[v] = true;
                    touched.push(v as Var);
                    self.bump(v);
                    if self.level[v] == cur_level {
                        counter += 1;
                    } else {
                        learned.push(q);
                    }
                }
            }
            // The most recently assigned `seen` literal on the trail.
            loop {
                idx -= 1;
                if self.seen[self.trail[idx].var() as usize] {
                    break;
                }
            }
            let lit = self.trail[idx];
            self.seen[lit.var() as usize] = false;
            counter -= 1;
            p = Some(lit);
            if counter == 0 {
                break;
            }
            confl = match self.reason[lit.var() as usize] {
                Reason::Long(c) => c,
                _ => unreachable!("a resolved current-level literal must have a clause reason"),
            };
        }
        learned[0] = p.unwrap().negate();

        let backjump = self.assertion_level(&mut learned);
        self.var_inc *= 1.0 / 0.95; // VSIDS decay

        for v in touched {
            self.seen[v as usize] = false; // restore the scratch buffer
        }
        (learned, backjump)
    }

    /// Move the highest-level non-asserting literal to index 1 and return its
    /// level (the level to backjump to), or 0 for a unit clause.
    fn assertion_level(&self, learned: &mut [SatLit]) -> u32 {
        if learned.len() == 1 {
            return 0;
        }
        let mut max_i = 1;
        let mut max_l = self.level[learned[1].var() as usize];
        for (i, &lit) in learned.iter().enumerate().skip(2) {
            let l = self.level[lit.var() as usize];
            if l > max_l {
                max_l = l;
                max_i = i;
            }
        }
        learned.swap(1, max_i);
        max_l
    }

    /// MiniSat's `analyzeFinal`. `true_lit` is currently TRUE on the trail and is
    /// the negation of a contradicted assumption; walk its implication graph and
    /// collect the assumptions that entail it. Returns a *sufficient* core — a
    /// subset of [`Solver::assumptions`] (including the contradicted assumption
    /// itself) such that `cnf ∧ core` is unsatisfiable. Restores `seen` on exit.
    fn analyze_final(&mut self, true_lit: SatLit) -> Vec<SatLit> {
        let mut core = vec![true_lit.negate()]; // the contradicted assumption
        if self.current_level() == 0 {
            // `cnf` entails `~assumption` outright; the assumption alone suffices.
            return core;
        }
        let assn = self.assumptions.len() as u32;
        let start = self.decisions[0]; // trail index where level 1 begins
        self.seen[true_lit.var() as usize] = true;
        let mut touched = vec![true_lit.var()];
        let mut i = self.trail.len();
        while i > start {
            i -= 1;
            let x = self.trail[i].var() as usize;
            if !self.seen[x] {
                continue;
            }
            self.seen[x] = false;
            match self.reason[x] {
                // A decision sitting at an assumption level *is* an assumption.
                Reason::Decision => {
                    if self.level[x] > 0 && self.level[x] <= assn {
                        core.push(self.trail[i]);
                    }
                }
                Reason::Unit => {}
                // Pull in the antecedents (clause[1..] are the false literals).
                Reason::Long(cr) => {
                    for j in 1..self.clauses[cr].len() {
                        let v = self.clauses[cr][j].var();
                        if self.level[v as usize] > 0 && !self.seen[v as usize] {
                            self.seen[v as usize] = true;
                            touched.push(v);
                        }
                    }
                }
            }
        }
        for v in touched {
            self.seen[v as usize] = false;
        }
        core
    }

    /// Undo assignments above `level`, saving each unset variable's phase for
    /// later reuse, and rewind the propagation queue to that level.
    fn backtrack(&mut self, level: u32) {
        if self.current_level() <= level {
            return;
        }
        let new_len = self.decisions[level as usize];
        for i in new_len..self.trail.len() {
            let v = self.trail[i].var() as usize;
            self.polarity[v] = self.assign[v] == Some(true);
            self.assign[v] = None;
        }
        self.trail.truncate(new_len);
        self.decisions.truncate(level as usize);
        self.qhead = new_len;
    }

    /// Install a freshly learned clause and enqueue its asserting literal.
    fn learn(&mut self, learned: Vec<SatLit>) {
        if learned.len() == 1 {
            self.enqueue(learned[0], Reason::Unit);
        } else {
            let cref = self.clauses.len();
            self.watch(cref, learned[0], learned[1]);
            let assert_lit = learned[0];
            self.clauses.push(learned);
            self.enqueue(assert_lit, Reason::Long(cref));
        }
    }

    // -- decisions --

    /// Choose the next decision: the unassigned variable with the highest VSIDS
    /// activity, using its saved phase. `None` means all variables are assigned.
    fn pick_branch(&self) -> Option<SatLit> {
        let mut best: Option<usize> = None;
        let mut best_act = -1.0;
        for v in 0..self.num_vars {
            if self.assign[v].is_none() && self.activity[v] > best_act {
                best_act = self.activity[v];
                best = Some(v);
            }
        }
        best.map(|v| SatLit::new(v as Var, self.polarity[v]))
    }

    // -- the state machine --

    /// The decision phase: place the next not-yet-satisfied assumption (or detect a
    /// contradicted one and return its core), otherwise branch by VSIDS. Each
    /// assumption — even an already-true one (a dummy level) — consumes exactly one
    /// decision level, so level `i+1` always corresponds to `assumptions[i]`.
    fn decide(&mut self) -> Decision {
        while (self.current_level() as usize) < self.assumptions.len() {
            let p = self.assumptions[self.current_level() as usize];
            if self.lit_is_true(p) {
                self.decisions.push(self.trail.len()); // dummy level, nothing enqueued
            } else if self.lit_is_false(p) {
                return Decision::UnsatCore(self.analyze_final(p.negate()));
            } else {
                self.decisions.push(self.trail.len());
                self.enqueue(p, Reason::Decision);
                return Decision::Propagated;
            }
        }
        match self.pick_branch() {
            None => Decision::Sat,
            Some(lit) => {
                self.decisions.push(self.trail.len());
                self.enqueue(lit, Reason::Decision);
                Decision::Propagated
            }
        }
    }

    /// Drive the search to a terminal state under the current assumptions.
    /// `Ok(())` = SAT; `Err(core)` = UNSAT with a sufficient subset of the
    /// assumptions (empty when unsat regardless of them). Re-entrant: after
    /// [`Solver::block`] resets to level 0, calling it again continues the search.
    fn run(&mut self) -> Result<(), Vec<SatLit>> {
        if !self.ok {
            return Err(Vec::new());
        }
        loop {
            if let Some(cref) = self.propagate() {
                if self.current_level() == 0 {
                    self.ok = false;
                    return Err(Vec::new());
                }
                let (learned, backjump) = self.analyze(cref);
                self.backtrack(backjump);
                self.learn(learned);
            } else {
                match self.decide() {
                    Decision::Propagated => {}
                    Decision::Sat => return Ok(()),
                    Decision::UnsatCore(core) => return Err(core),
                }
            }
        }
    }

    /// Plain satisfiability (no assumptions): `true` if a model exists. Re-entrant
    /// for [`Models`] enumeration.
    fn search(&mut self) -> bool {
        self.run().is_ok()
    }

    /// Snapshot the assignment as `var -> bool` (any still-unassigned variable,
    /// possible when it is unconstrained, defaults to false).
    fn model(&self) -> Vec<bool> {
        self.assign.iter().map(|a| a.unwrap_or(false)).collect()
    }

    /// Forbid the current `model`'s projection, then reset to level 0 so the next
    /// [`Solver::search`] finds a different model. Returns `false` if the
    /// projection is empty (there is only one model to report).
    fn block(&mut self, project: &[Var], model: &[bool]) -> bool {
        if project.is_empty() {
            return false;
        }
        let block: Vec<SatLit> = project
            .iter()
            .map(|&v| {
                if model[v as usize] {
                    SatLit::negative(v)
                } else {
                    SatLit::positive(v)
                }
            })
            .collect();
        self.backtrack(0);
        self.add_clause(&block);
        true
    }
}

// --- public API ------------------------------------------------------------

/// The outcome of [`solve_assuming`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Solved {
    /// Satisfiable: a full model (`var -> bool`).
    Sat(Vec<bool>),
    /// Unsatisfiable under the assumptions: a *sufficient* subset of them (a core)
    /// such that `cnf ∧ core` is unsatisfiable. Empty means the formula is
    /// unsatisfiable regardless of the assumptions. Not guaranteed minimal.
    Unsat(Vec<SatLit>),
}

/// Solve `cnf` with every literal in `assumptions` forced true. Returns a model,
/// or an unsat core — a sufficient (not necessarily minimal) subset of
/// `assumptions`. Minimize the core separately if you need 1-minimality.
pub fn solve_assuming(cnf: &Cnf, assumptions: &[SatLit]) -> Solved {
    let mut s = Solver::new(cnf);
    s.assumptions = assumptions.to_vec();
    match s.run() {
        Ok(()) => Solved::Sat(s.model()),
        Err(core) => Solved::Unsat(core),
    }
}

/// Solve a CNF. Returns a full model (`var -> bool`) or `None` if unsatisfiable.
pub fn solve(cnf: &Cnf) -> Option<Vec<bool>> {
    match solve_assuming(cnf, &[]) {
        Solved::Sat(model) => Some(model),
        Solved::Unsat(_) => None,
    }
}

/// A lazy iterator over the models of a CNF, distinct on the `project` variables.
/// Solving is **incremental**: each step adds a blocking clause and continues
/// from the existing solver state instead of restarting from scratch.
pub struct Models {
    solver: Solver,
    project: Vec<Var>,
    done: bool,
}

impl Iterator for Models {
    type Item = Vec<bool>;

    fn next(&mut self) -> Option<Vec<bool>> {
        if self.done {
            return None;
        }
        if !self.solver.search() {
            self.done = true;
            return None;
        }
        let model = self.solver.model();
        if !self.solver.block(&self.project, &model) {
            self.done = true;
        }
        Some(model)
    }
}

/// Lazily enumerate all models of `cnf`, distinct over `project`.
pub fn all_models(cnf: &Cnf, project: Vec<Var>) -> Models {
    Models {
        solver: Solver::new(cnf),
        project,
        done: false,
    }
}

/// Up to `limit` models, distinct over `project` (eagerly collected).
pub fn models(cnf: &Cnf, project: &[Var], limit: usize) -> Vec<Vec<bool>> {
    all_models(cnf, project.to_vec()).take(limit).collect()
}

/// Count distinct models projected onto `project`, up to `limit`.
pub fn models_upto(cnf: &Cnf, project: &[Var], limit: usize) -> usize {
    all_models(cnf, project.to_vec()).take(limit).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_sat() {
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
        assert!(solve(&c).is_some());
    }

    #[test]
    fn unit_contradiction_unsat() {
        let mut c = Cnf::new(1);
        c.add_clause(vec![SatLit::positive(0)]);
        c.add_clause(vec![SatLit::negative(0)]);
        assert!(solve(&c).is_none());
    }

    #[test]
    fn all_four_combos_excluded_is_unsat() {
        let mut c = Cnf::new(2);
        let (a, b) = (0u32, 1u32);
        c.add_clause(vec![SatLit::positive(a), SatLit::positive(b)]);
        c.add_clause(vec![SatLit::negative(a), SatLit::positive(b)]);
        c.add_clause(vec![SatLit::positive(a), SatLit::negative(b)]);
        c.add_clause(vec![SatLit::negative(a), SatLit::negative(b)]);
        assert!(solve(&c).is_none());
    }

    #[test]
    fn forced_chain_has_unique_model() {
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::negative(0), SatLit::positive(1)]);
        c.add_clause(vec![SatLit::positive(0)]);
        let m = solve(&c).unwrap();
        assert!(m[0] && m[1]);
        assert_eq!(models_upto(&c, &[0, 1], 5), 1);
    }

    #[test]
    fn or_clause_has_three_models() {
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
        assert_eq!(models_upto(&c, &[0, 1], 10), 3);
    }

    #[test]
    fn lazy_models_iterator_is_incremental() {
        // (a∨b) has 3 models; the iterator yields them lazily one at a time.
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
        let first_two: Vec<_> = all_models(&c, vec![0, 1]).take(2).collect();
        assert_eq!(first_two.len(), 2);
        assert_ne!(first_two[0], first_two[1]);
        assert_eq!(all_models(&c, vec![0, 1]).count(), 3);
    }

    #[test]
    fn assumption_forces_a_model() {
        // (a ∨ b); assume ¬a ⇒ b must be true.
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
        match solve_assuming(&c, &[SatLit::negative(0)]) {
            Solved::Sat(m) => {
                assert!(!m[0] && m[1]);
            }
            Solved::Unsat(_) => panic!("should be SAT under ¬a"),
        }
    }

    #[test]
    fn contradicted_assumptions_yield_a_sufficient_core() {
        // (¬a ∨ ¬b); assume a and b ⇒ UNSAT, core ⊆ {a, b} and cnf ∧ core UNSAT.
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::negative(0), SatLit::negative(1)]);
        let assumptions = [SatLit::positive(0), SatLit::positive(1)];
        match solve_assuming(&c, &assumptions) {
            Solved::Unsat(core) => {
                assert!(!core.is_empty());
                assert!(core.iter().all(|l| assumptions.contains(l)));
                // cnf ∧ core is unsatisfiable.
                let mut cc = c.clone();
                for l in &core {
                    cc.add_clause(vec![*l]);
                }
                assert!(solve(&cc).is_none());
            }
            Solved::Sat(_) => panic!("a ∧ b violates (¬a ∨ ¬b)"),
        }
    }

    #[test]
    fn satisfiable_assumptions_round_trip() {
        // Independent vars; assuming a few is fine and the model honors them.
        let mut c = Cnf::new(3);
        c.add_clause(vec![
            SatLit::positive(0),
            SatLit::positive(1),
            SatLit::positive(2),
        ]);
        let assumptions = [SatLit::positive(0), SatLit::negative(2)];
        match solve_assuming(&c, &assumptions) {
            Solved::Sat(m) => {
                assert!(m[0] && !m[2]);
            }
            Solved::Unsat(_) => panic!("should be SAT"),
        }
    }

    #[test]
    fn larger_random_like_sat_is_solved() {
        let mut c = Cnf::new(5);
        let l = |v: u32, p: bool| SatLit::new(v, p);
        c.add_clause(vec![l(0, true), l(1, true), l(2, false)]);
        c.add_clause(vec![l(0, false), l(2, true), l(3, true)]);
        c.add_clause(vec![l(1, false), l(3, false), l(4, true)]);
        c.add_clause(vec![l(2, false), l(4, false)]);
        c.add_clause(vec![l(0, true), l(4, true)]);
        let m = solve(&c).expect("sat");
        for clause in &c.clauses {
            assert!(
                clause
                    .iter()
                    .any(|&lit| m[lit.var() as usize] != lit.is_negative())
            );
        }
    }
}
