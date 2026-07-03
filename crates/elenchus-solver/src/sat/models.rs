//! Lazy, incremental model enumeration: each `next()` adds a blocking clause and
//! continues from the existing solver state rather than re-solving from scratch.
use alloc::vec::Vec;

use super::solver::Solver;
use super::{Budget, BudgetExhausted, Cnf, Var};

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
        // `Models` never installs a budget, so `search` cannot exhaust one;
        // budgeted enumeration goes through `models_budgeted` instead.
        let found = self
            .solver
            .search()
            .unwrap_or_else(|_| unreachable!("budget-free enumeration cannot exhaust"));
        if !found {
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

/// [`models`] under an optional shared conflict [`Budget`]: the same solver
/// calls in the same order, so with enough budget the result is identical to
/// the budget-free call; when the pool runs out mid-enumeration the whole call
/// aborts with [`BudgetExhausted`] — never a silently short list.
pub fn models_budgeted(
    cnf: &Cnf,
    project: &[Var],
    limit: usize,
    budget: Option<&Budget>,
) -> Result<Vec<Vec<bool>>, BudgetExhausted> {
    let mut solver = Solver::new(cnf);
    solver.set_budget(budget.cloned());
    let mut found: Vec<Vec<bool>> = Vec::new();
    while found.len() < limit {
        if !solver.search()? {
            break;
        }
        let model = solver.model();
        let more = solver.block(project, &model);
        found.push(model);
        if !more {
            break;
        }
    }
    Ok(found)
}
