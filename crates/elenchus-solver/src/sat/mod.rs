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

use alloc::vec::Vec;

mod models;
mod solver;

#[cfg(test)]
mod tests;

pub use models::{Models, all_models, models, models_upto};

use solver::Solver;

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
