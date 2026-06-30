//! Tests for the in-crate CDCL SAT solver, through its public `sat` API.
use elenchus_solver::sat::*;

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
