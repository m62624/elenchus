//! Lazy, incremental model enumeration: each `next()` adds a blocking clause and
//! continues from the existing solver state rather than re-solving from scratch.
use alloc::vec::Vec;

use super::solver::Solver;
use super::{Cnf, Var};

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
