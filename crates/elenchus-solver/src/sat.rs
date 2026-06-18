//! A compact, single-threaded CDCL SAT solver in `no_std`, replicating the core
//! algorithm of varisat (jix/varisat) in a readable, lazy style.
//!
//! The solver is a small **state machine**: [`Solver::step`] performs exactly one
//! transition and returns a [`Step`] saying which way the search went
//! (propagated / hit a conflict / made a decision / SAT / UNSAT). [`Solver::search`]
//! drives steps to a terminal state. Model enumeration is a lazy [`Models`]
//! iterator that solves **incrementally** — each `next()` adds a blocking clause
//! and continues from the existing state rather than re-solving from scratch.
//!
//! Pieces mirror varisat's modules: the trail + decision levels
//! (`prop/assignment.rs`), two-watched-literal propagation (`prop/long.rs`),
//! 1-UIP conflict analysis with clause learning (`analyze_conflict.rs`),
//! non-chronological backjumping, and VSIDS decisions with phase saving.
//! Its infrastructure (proof/DRAT logging, clause-DB GC, assumptions, restarts,
//! the `partial_ref` context, multithreading) is intentionally omitted.

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
    pub fn new(var: Var, positive: bool) -> Self {
        SatLit((var << 1) | (!positive as u32))
    }
    pub fn positive(var: Var) -> Self {
        Self::new(var, true)
    }
    pub fn negative(var: Var) -> Self {
        Self::new(var, false)
    }
    pub fn var(self) -> Var {
        self.0 >> 1
    }
    pub fn is_negative(self) -> bool {
        self.0 & 1 == 1
    }
    pub fn negate(self) -> SatLit {
        SatLit(self.0 ^ 1)
    }
    fn code(self) -> usize {
        self.0 as usize
    }
}

/// A CNF formula over `num_vars` variables.
#[derive(Clone, Debug, Default)]
pub struct Cnf {
    pub num_vars: usize,
    pub clauses: Vec<Vec<SatLit>>,
}

impl Cnf {
    pub fn new(num_vars: usize) -> Self {
        Cnf {
            num_vars,
            clauses: Vec::new(),
        }
    }
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

/// The outcome of a single CDCL transition. Makes the search direction explicit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    /// A clause was falsified; it was analyzed, learned, and backjumped.
    LearntFromConflict,
    /// A new decision literal was assigned.
    Decided,
    /// Every variable is assigned — the formula is satisfied.
    Sat,
    /// A conflict at decision level 0 — the formula is unsatisfiable.
    Unsat,
}

struct Solver {
    num_vars: usize,
    clauses: Vec<Vec<SatLit>>, // originals + learned + blocking
    watches: Vec<Vec<Watch>>,  // indexed by literal code; a clause watching `w` lives in watches[!w]
    assign: Vec<Option<bool>>, // per var
    level: Vec<u32>,           // per var (valid when assigned)
    reason: Vec<Reason>,       // per var (valid when assigned)
    trail: Vec<SatLit>,
    decisions: Vec<usize>, // trail index where each decision level starts
    qhead: usize,
    activity: Vec<f64>,
    var_inc: f64,
    polarity: Vec<bool>, // phase saving
    seen: Vec<bool>,     // reusable scratch for analyze (invariant: all-false between calls)
    ok: bool,            // false once the formula is known UNSAT
}

impl Solver {
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
        };
        for clause in &cnf.clauses {
            s.add_clause(clause);
        }
        s
    }

    // -- assignment queries --

    fn lit_is_true(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(!l.is_negative())
    }
    fn lit_is_false(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(l.is_negative())
    }
    fn current_level(&self) -> u32 {
        self.decisions.len() as u32
    }

    // -- clause loading --

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
            let kept = Watch { cref, blocking: other };

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

    /// Perform one CDCL transition.
    fn step(&mut self) -> Step {
        if let Some(cref) = self.propagate() {
            if self.decisions.is_empty() {
                return Step::Unsat;
            }
            let (learned, backjump) = self.analyze(cref);
            self.backtrack(backjump);
            self.learn(learned);
            Step::LearntFromConflict
        } else {
            match self.pick_branch() {
                None => Step::Sat,
                Some(lit) => {
                    self.decisions.push(self.trail.len());
                    self.enqueue(lit, Reason::Decision);
                    Step::Decided
                }
            }
        }
    }

    /// Drive steps until SAT or UNSAT. Re-entrant: after [`Solver::block`] adds a
    /// clause and resets to level 0, calling this again continues the search.
    fn search(&mut self) -> bool {
        if !self.ok {
            return false;
        }
        loop {
            match self.step() {
                Step::Sat => return true,
                Step::Unsat => {
                    self.ok = false;
                    return false;
                }
                _ => {}
            }
        }
    }

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

/// Solve a CNF. Returns a full model (`var -> bool`) or `None` if unsatisfiable.
pub fn solve(cnf: &Cnf) -> Option<Vec<bool>> {
    let mut s = Solver::new(cnf);
    if s.search() { Some(s.model()) } else { None }
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
            assert!(clause.iter().any(|&lit| m[lit.var() as usize] != lit.is_negative()));
        }
    }
}
