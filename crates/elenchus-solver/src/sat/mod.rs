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
//! **Incremental solving** ([`Incremental`]): one clause database answering a
//! sequence of assumption queries, learned clauses persisting across them —
//! the engine of the deletion-minimization and TRY-counting paths. Its
//! deterministic [`Stats`] counters are the cross-hardware performance metric
//! (`tests/perf_gates.rs`); [`SolverConfig`] switches the measured heuristics
//! on for verdict/count-only callers.
//!
//! **Conflict budget** ([`Budget`]): an optional shared pool of analyzed
//! conflicts. Solvers holding clones of one handle draw from the same
//! allowance; running out aborts the solve with [`BudgetExhausted`] — an
//! explicit resource error, never a wrong or truncated answer. Deterministic
//! like everything else here: the same budget aborts at the same conflict on
//! any hardware.
//!
//! Pieces mirror varisat's modules: the trail + decision levels
//! (`prop/assignment.rs`), two-watched-literal propagation (`prop/long.rs`),
//! 1-UIP conflict analysis with clause learning (`analyze_conflict.rs`) plus
//! optional learned-clause minimization ([`SolverConfig::ccmin`]),
//! non-chronological backjumping, VSIDS decisions with phase saving, and
//! assumption-based solving. Remaining infrastructure (proof/DRAT logging,
//! clause-DB GC, the `partial_ref` context, multithreading) is intentionally
//! omitted; Luby restarts were implemented, measured on the work counters, and
//! rejected (see [`SolverConfig::TURBO`]).

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::Cell;

mod incremental;
mod models;
mod solver;

pub use incremental::Incremental;
pub use models::{Models, all_models, models, models_budgeted, models_upto};

use solver::{RunFail, Solver};

/// A shared, deterministic conflict budget: a pool of "conflicts the caller is
/// willing to pay for". Cloned handles share **one** pool (single-threaded
/// reference counting — no atomics needed), so every solver of a verification
/// run can draw from the same allowance. A solve that would analyze more
/// conflicts than the pool holds aborts with [`BudgetExhausted`] instead —
/// never a wrong or silently truncated answer.
///
/// The unit is *analyzed conflicts*, the same deterministic counter as
/// [`Stats::conflicts`]: a budget of `n` admits exactly `n` conflicts, the
/// `n+1`-th aborts, bit-identically on any hardware. No budget (the default
/// everywhere) costs nothing on the non-conflict path and one branch per
/// conflict.
#[derive(Clone, Debug)]
pub struct Budget(Rc<Cell<u64>>);

impl Budget {
    /// A fresh pool admitting `max_conflicts` analyzed conflicts in total
    /// across every solver holding a clone of this handle.
    pub fn new(max_conflicts: u64) -> Self {
        Budget(Rc::new(Cell::new(max_conflicts)))
    }

    /// Conflicts still admitted. Shared across clones.
    pub fn remaining(&self) -> u64 {
        self.0.get()
    }

    /// Pay for one conflict; `false` when the pool is empty (the caller must
    /// abort with [`BudgetExhausted`] rather than analyze the conflict).
    pub(crate) fn spend(&self) -> bool {
        let left = self.0.get();
        if left == 0 {
            return false;
        }
        self.0.set(left - 1);
        true
    }
}

/// The conflict [`Budget`] ran out before the solve reached an answer.
///
/// This is a resource abort, **not** a verdict: the formula's status is simply
/// unknown. It can only occur when a budget was installed, so budget-free
/// entry points remain infallible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetExhausted;

impl core::fmt::Display for BudgetExhausted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("conflict budget exhausted")
    }
}

impl core::error::Error for BudgetExhausted {}

/// Deterministic work counters, accumulated over a solver's lifetime.
///
/// The CDCL search is fully deterministic, so these numbers are **bit-identical
/// on any machine** — unlike wall-clock time, they are an honest performance
/// metric on shared/noisy hardware (CI). Tests compare them across solving
/// strategies to prove one does strictly less work than another.
///
/// All increments are saturating: a counter that reaches [`u64::MAX`] pins
/// there instead of panicking (debug) or silently wrapping (release), so a
/// value always reads as "at least this much work". Saturation is unreachable
/// in practice — at 10⁹ increments per second it takes ~584 years — the
/// arithmetic is total purely so no build profile can misbehave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Literals decided on (assumptions and VSIDS branches alike).
    pub decisions: u64,
    /// Literals taken off the propagation queue (the unit-propagation workload).
    pub propagations: u64,
    /// Conflicts hit (= clauses learned).
    pub conflicts: u64,
    /// Total literals across all learned clauses (the clause-learning volume),
    /// counted after minimization when [`SolverConfig::ccmin`] is on.
    pub learned_literals: u64,
}

/// Search heuristics. `Default` is the **reference profile** — everything off,
/// bit-identical to the solver's historical behavior.
///
/// Heuristics change the *path* of the search, so they can change **which** model
/// or core is found (all results stay correct). Enable them only where the caller
/// consumes SAT/UNSAT verdicts or model counts — those are semantically unique,
/// heuristic-invariant. Callers whose reported output embeds model/core contents
/// must stay on the reference profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SolverConfig {
    /// Learned-clause minimization (MiniSat-style): drop a learned literal whose
    /// reason clause is entirely subsumed by the rest of the learned clause —
    /// shorter learned clauses propagate faster and prune more.
    pub ccmin: bool,
}

impl SolverConfig {
    /// Every heuristic that earned its keep on the deterministic work counters
    /// (measured on pigeonhole and deletion-minimization stress workloads; e.g.
    /// ccmin cut php(9,8) conflicts 12511 → 7413 and learned literals by 48%).
    /// Luby restarts were implemented, measured, and **rejected**: without
    /// clause deletion they only degraded these workloads (php(9,8) conflicts
    /// +156%) and never fired on program-scale queries. The profile for
    /// verdict/count-only callers; content-bearing callers use `Default` (the
    /// reference profile).
    pub const TURBO: SolverConfig = SolverConfig { ccmin: true };
}

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
    match solve_assuming_budgeted(cnf, assumptions, None) {
        Ok(solved) => solved,
        // No budget was installed, so exhaustion cannot occur.
        Err(BudgetExhausted) => unreachable!("budget-free solve cannot exhaust"),
    }
}

/// [`solve_assuming`] under an optional shared conflict [`Budget`]: the answer
/// is identical to the budget-free call, or [`BudgetExhausted`] when the pool
/// runs out first — the budget can only withhold an answer, never change one.
pub fn solve_assuming_budgeted(
    cnf: &Cnf,
    assumptions: &[SatLit],
    budget: Option<&Budget>,
) -> Result<Solved, BudgetExhausted> {
    let mut s = Solver::new(cnf);
    s.set_budget(budget.cloned());
    s.assumptions = assumptions.to_vec();
    match s.run() {
        Ok(()) => Ok(Solved::Sat(s.model())),
        Err(RunFail::Unsat(core)) => Ok(Solved::Unsat(core)),
        Err(RunFail::Exhausted) => Err(BudgetExhausted),
    }
}

/// Solve a CNF. Returns a full model (`var -> bool`) or `None` if unsatisfiable.
pub fn solve(cnf: &Cnf) -> Option<Vec<bool>> {
    match solve_assuming(cnf, &[]) {
        Solved::Sat(model) => Some(model),
        Solved::Unsat(_) => None,
    }
}
