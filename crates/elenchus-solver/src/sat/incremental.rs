//! An incremental (re-entrant) solver: one clause database answering a sequence
//! of assumption queries, MiniSat-style.
//!
//! Where [`solve_assuming`](super::solve_assuming) builds a fresh solver per call
//! and throws every learned clause away, [`Incremental`] keeps them: a learned
//! clause is a consequence of the loaded CNF alone (never of the assumptions), so
//! it stays valid for every later query. A deletion-minimization loop that asks
//! O(n) closely related questions over one formula pays for propagation once
//! instead of n times.
//!
//! **Determinism caveat**: because learned clauses persist, the *model* (and the
//! *core*) a query returns can differ from what a fresh [`solve_assuming`] on the
//! same formula would return. The SAT/UNSAT verdict — and any model *count* — is
//! semantically unique and always agrees. Use this type only where the caller
//! consumes verdicts or counts, not model/core contents (those callers keep the
//! fresh-solver path so their reported witnesses stay stable).

use super::solver::Solver;
use super::{Cnf, SatLit, Solved, Stats};

/// A persistent solver over one CNF, answering assumption queries incrementally.
/// See the [module docs](self) for the contract and the determinism caveat.
pub struct Incremental {
    solver: Solver,
}

impl Incremental {
    /// Load `cnf` once; every later [`Incremental::solve`] reuses this database.
    pub fn new(cnf: &Cnf) -> Self {
        Incremental {
            solver: Solver::new(cnf),
        }
    }

    /// Solve under `assumptions` (each forced true). Re-entrant: call as many
    /// times as needed; learned clauses accumulate across calls. The returned
    /// core, like [`solve_assuming`](super::solve_assuming)'s, is a sufficient
    /// subset of the assumptions (empty = UNSAT regardless of them).
    pub fn solve(&mut self, assumptions: &[SatLit]) -> Solved {
        match self.solver.solve_with(assumptions) {
            Ok(()) => Solved::Sat(self.solver.model()),
            Err(core) => Solved::Unsat(core),
        }
    }

    /// Add one clause to the database between queries (e.g. a guarded blocking
    /// clause `¬guard ∨ …`, active only when `guard` is assumed).
    pub fn add_clause(&mut self, lits: &[SatLit]) {
        self.solver.add_clause_root(lits);
    }

    /// Cumulative deterministic work counters — bit-identical on any hardware,
    /// the honest cross-machine performance metric (see [`Stats`]).
    pub fn stats(&self) -> &Stats {
        self.solver.stats()
    }
}
