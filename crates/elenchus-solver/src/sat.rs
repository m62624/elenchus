//! A compact, single-threaded CDCL SAT solver in `no_std`, replicating the core
//! algorithm of varisat (jix/varisat):
//!
//! - a **trail** of assigned literals partitioned into **decision levels**
//!   (`prop/assignment.rs`);
//! - **two-watched-literal** unit propagation (`prop/long.rs`);
//! - **1-UIP conflict analysis** with clause learning (`analyze_conflict.rs`);
//! - **non-chronological backjumping** to the asserting level;
//! - **VSIDS** activity-based decisions with **phase saving**.
//!
//! varisat's surrounding infrastructure — proof/DRAT logging, clause-DB garbage
//! collection, the assumptions framework, the `partial_ref` context, restarts,
//! and multithreading — is intentionally left out; this is the bare solving core
//! we need for the backward pass (consistency + model finding over small inputs).

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

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

#[derive(Clone, Copy)]
enum Reason {
    Decision,
    Unit,
    Long(usize),
}

#[derive(Clone, Copy)]
struct Watch {
    cref: usize,
    blocking: SatLit,
}

struct Solver {
    num_vars: usize,
    clauses: Vec<Vec<SatLit>>, // originals + learned
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
    ok: bool,            // false once a top-level conflict makes it UNSAT
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
            ok: true,
        };
        for clause in &cnf.clauses {
            s.add_clause(clause);
        }
        s
    }

    fn lit_is_true(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(!l.is_negative())
    }
    fn lit_is_false(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(l.is_negative())
    }

    fn add_clause(&mut self, lits: &[SatLit]) {
        if !self.ok {
            return;
        }
        match lits.len() {
            0 => self.ok = false,
            1 => {
                let l = lits[0];
                if self.lit_is_false(l) {
                    self.ok = false;
                } else if !self.lit_is_true(l) {
                    self.enqueue(l, Reason::Unit);
                }
            }
            _ => {
                let cref = self.clauses.len();
                let w0 = lits[0];
                let w1 = lits[1];
                self.watches[w0.negate().code()].push(Watch {
                    cref,
                    blocking: w1,
                });
                self.watches[w1.negate().code()].push(Watch {
                    cref,
                    blocking: w0,
                });
                self.clauses.push(lits.to_vec());
            }
        }
    }

    fn enqueue(&mut self, l: SatLit, reason: Reason) {
        let v = l.var() as usize;
        self.assign[v] = Some(!l.is_negative());
        self.level[v] = self.decisions.len() as u32;
        self.reason[v] = reason;
        self.trail.push(l);
    }

    /// Two-watched-literal unit propagation. Returns the conflicting clause, if any.
    fn propagate(&mut self) -> Option<usize> {
        while self.qhead < self.trail.len() {
            let p = self.trail[self.qhead];
            self.qhead += 1;
            let fl = p.negate(); // the watched literal that just became false

            let mut ws = core::mem::take(&mut self.watches[p.code()]);
            let mut i = 0;
            let mut write = 0;
            let mut conflict = None;
            while i < ws.len() {
                let w = ws[i];
                i += 1;
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
                let nw = Watch {
                    cref,
                    blocking: other,
                };
                if other != w.blocking && self.lit_is_true(other) {
                    ws[write] = nw;
                    write += 1;
                    continue;
                }
                // Look for a non-false replacement among the unwatched literals.
                let mut found = false;
                let len = self.clauses[cref].len();
                for k in 2..len {
                    let ck = self.clauses[cref][k];
                    if !self.lit_is_false(ck) {
                        self.clauses[cref][1] = ck;
                        self.clauses[cref][k] = fl;
                        self.watches[ck.negate().code()].push(nw);
                        found = true;
                        break;
                    }
                }
                if found {
                    continue; // watch moved off `p`'s list
                }
                // No replacement: keep watching `fl`.
                ws[write] = nw;
                write += 1;
                if self.lit_is_false(other) {
                    // conflict: preserve the rest of the list unprocessed
                    while i < ws.len() {
                        ws[write] = ws[i];
                        write += 1;
                        i += 1;
                    }
                    conflict = Some(cref);
                    break;
                } else {
                    self.enqueue(other, Reason::Long(cref));
                }
            }
            ws.truncate(write);
            self.watches[p.code()] = ws;
            if conflict.is_some() {
                return conflict;
            }
        }
        None
    }

    fn bump(&mut self, v: usize) {
        self.activity[v] += self.var_inc;
        if self.activity[v] > 1e100 {
            for a in &mut self.activity {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }
    }

    /// 1-UIP conflict analysis. Returns (learned clause, backjump level).
    fn analyze(&mut self, conflict: usize) -> (Vec<SatLit>, u32) {
        let cur_level = self.decisions.len() as u32;
        let mut seen = vec![false; self.num_vars];
        let mut learned: Vec<SatLit> = vec![SatLit(0)]; // slot 0 = asserting literal
        let mut counter = 0usize;
        let mut idx = self.trail.len();
        let mut p: Option<SatLit> = None;
        let mut confl = conflict;

        loop {
            let start = if p.is_some() { 1 } else { 0 }; // reason clause has p at index 0
            for j in start..self.clauses[confl].len() {
                let q = self.clauses[confl][j];
                let v = q.var() as usize;
                if !seen[v] && self.level[v] > 0 {
                    seen[v] = true;
                    self.bump(v);
                    if self.level[v] == cur_level {
                        counter += 1;
                    } else {
                        learned.push(q);
                    }
                }
            }
            // Pick the most recently assigned `seen` literal on the trail.
            loop {
                idx -= 1;
                if seen[self.trail[idx].var() as usize] {
                    break;
                }
            }
            let lit = self.trail[idx];
            let v = lit.var() as usize;
            seen[v] = false;
            counter -= 1;
            p = Some(lit);
            if counter == 0 {
                break;
            }
            confl = match self.reason[v] {
                Reason::Long(c) => c,
                _ => unreachable!("a current-level resolved literal must have a clause reason"),
            };
        }
        learned[0] = p.unwrap().negate();

        let backjump = if learned.len() == 1 {
            0
        } else {
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
        };
        self.var_inc *= 1.0 / 0.95; // VSIDS decay
        (learned, backjump)
    }

    fn backtrack(&mut self, level: u32) {
        if self.decisions.len() as u32 <= level {
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

    fn run(&mut self) -> bool {
        if !self.ok {
            return false;
        }
        if self.propagate().is_some() {
            return false; // conflict implied by the unit/top-level clauses
        }
        loop {
            if let Some(cref) = self.propagate() {
                if self.decisions.is_empty() {
                    return false; // conflict at level 0 → UNSAT
                }
                let (learned, backjump) = self.analyze(cref);
                self.backtrack(backjump);
                if learned.len() == 1 {
                    self.enqueue(learned[0], Reason::Unit);
                } else {
                    let cref = self.clauses.len();
                    self.watches[learned[0].negate().code()].push(Watch {
                        cref,
                        blocking: learned[1],
                    });
                    self.watches[learned[1].negate().code()].push(Watch {
                        cref,
                        blocking: learned[0],
                    });
                    let assert_lit = learned[0];
                    self.clauses.push(learned);
                    self.enqueue(assert_lit, Reason::Long(cref));
                }
            } else {
                match self.pick_branch() {
                    None => return true, // every variable assigned → SAT
                    Some(lit) => {
                        self.decisions.push(self.trail.len());
                        self.enqueue(lit, Reason::Decision);
                    }
                }
            }
        }
    }

    fn model(&self) -> Vec<bool> {
        self.assign.iter().map(|a| a.unwrap_or(false)).collect()
    }
}

/// Solve a CNF. Returns a full model (`var -> bool`) or `None` if unsatisfiable.
pub fn solve(cnf: &Cnf) -> Option<Vec<bool>> {
    let mut s = Solver::new(cnf);
    if s.run() { Some(s.model()) } else { None }
}

/// Count distinct models, projected onto `project` variables, up to `limit`
/// (all-SAT via blocking clauses). Returns `min(#models_over_projection, limit)`.
pub fn models_upto(cnf: &Cnf, project: &[Var], limit: usize) -> usize {
    if project.is_empty() {
        // No projection dimension: satisfiable ⇒ exactly one (trivial) projection.
        return if solve(cnf).is_some() { 1.min(limit) } else { 0 };
    }
    let mut work = cnf.clone();
    let mut count = 0;
    while count < limit {
        match solve(&work) {
            None => break,
            Some(model) => {
                count += 1;
                // Block this exact projection so the next solve finds a different one.
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
                work.add_clause(block);
            }
        }
    }
    count
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
        // (a∨b)(¬a∨b)(a∨¬b)(¬a∨¬b) forbids every assignment of (a,b).
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
        // (¬a∨b) ∧ a  ⇒  a=T forces b=T; exactly one model over {a,b}.
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::negative(0), SatLit::positive(1)]);
        c.add_clause(vec![SatLit::positive(0)]);
        let m = solve(&c).unwrap();
        assert!(m[0] && m[1]);
        assert_eq!(models_upto(&c, &[0, 1], 5), 1);
    }

    #[test]
    fn or_clause_has_three_models() {
        // (a∨b) over {a,b}: TF, FT, TT — three models.
        let mut c = Cnf::new(2);
        c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
        assert_eq!(models_upto(&c, &[0, 1], 10), 3);
    }

    #[test]
    fn larger_random_like_sat_is_solved() {
        // A small satisfiable instance that needs propagation + backjumping.
        let mut c = Cnf::new(5);
        let l = |v: u32, p: bool| SatLit::new(v, p);
        c.add_clause(vec![l(0, true), l(1, true), l(2, false)]);
        c.add_clause(vec![l(0, false), l(2, true), l(3, true)]);
        c.add_clause(vec![l(1, false), l(3, false), l(4, true)]);
        c.add_clause(vec![l(2, false), l(4, false)]);
        c.add_clause(vec![l(0, true), l(4, true)]);
        let m = solve(&c).expect("sat");
        // verify the model satisfies every clause
        for clause in &c.clauses {
            assert!(clause.iter().any(|&lit| {
                let val = m[lit.var() as usize];
                val != lit.is_negative()
            }));
        }
    }
}
