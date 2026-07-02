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

// --- the incremental (re-entrant) solver -------------------------------------

#[test]
fn incremental_reentrant_queries_share_one_database() {
    // A chain (a → b), (b → c): assuming a forces the whole chain.
    let mut c = Cnf::new(3);
    c.add_clause(vec![SatLit::negative(0), SatLit::positive(1)]);
    c.add_clause(vec![SatLit::negative(1), SatLit::positive(2)]);
    let mut inc = Incremental::new(&c);
    match inc.solve(&[SatLit::positive(0)]) {
        Solved::Sat(m) => assert!(m[0] && m[1] && m[2]),
        Solved::Unsat(_) => panic!("chain under `a` is SAT"),
    }
    // Assuming a ∧ ¬c contradicts the chain; the core names only assumptions.
    let asm = [SatLit::positive(0), SatLit::negative(2)];
    match inc.solve(&asm) {
        Solved::Unsat(core) => {
            assert!(!core.is_empty());
            assert!(core.iter().all(|l| asm.contains(l)));
        }
        Solved::Sat(_) => panic!("a ∧ ¬c violates the chain"),
    }
    // The solver stays usable after an UNSAT query.
    assert!(matches!(inc.solve(&[SatLit::negative(2)]), Solved::Sat(_)));
}

#[test]
fn incremental_added_clauses_persist_across_queries() {
    let mut c = Cnf::new(2);
    c.add_clause(vec![SatLit::positive(0), SatLit::positive(1)]);
    let mut inc = Incremental::new(&c);
    assert!(matches!(inc.solve(&[]), Solved::Sat(_)));
    inc.add_clause(&[SatLit::negative(0)]);
    inc.add_clause(&[SatLit::negative(1)]);
    // (a∨b) ∧ ¬a ∧ ¬b is now UNSAT regardless of assumptions — and stays so.
    assert!(matches!(inc.solve(&[]), Solved::Unsat(_)));
    assert!(matches!(
        inc.solve(&[SatLit::positive(0)]),
        Solved::Unsat(_)
    ));
}

#[test]
fn incremental_stats_count_work_and_never_reset() {
    // The four-combos formula forces decisions, propagation, and conflicts.
    let mut c = Cnf::new(2);
    let (a, b) = (0u32, 1u32);
    c.add_clause(vec![SatLit::positive(a), SatLit::positive(b)]);
    c.add_clause(vec![SatLit::negative(a), SatLit::positive(b)]);
    c.add_clause(vec![SatLit::positive(a), SatLit::negative(b)]);
    c.add_clause(vec![SatLit::negative(a), SatLit::negative(b)]);
    let mut inc = Incremental::new(&c);
    assert!(matches!(inc.solve(&[]), Solved::Unsat(_)));
    let first = inc.stats().clone();
    assert!(first.decisions >= 1);
    assert!(first.propagations >= 1);
    assert!(first.conflicts >= 1);
    assert!(first.learned_literals >= 1);
    // Counters are cumulative: a second query can only grow them.
    assert!(matches!(inc.solve(&[]), Solved::Unsat(_)));
    let second = inc.stats().clone();
    assert!(second.decisions >= first.decisions);
    assert!(second.propagations >= first.propagations);
    assert!(second.conflicts >= first.conflicts);
    assert!(second.learned_literals >= first.learned_literals);
}

#[test]
fn incremental_reuse_beats_scratch_on_work_counters() {
    // The honest, hardware-independent speed test: a deletion-minimization-shaped
    // query sequence costs strictly fewer conflicts on one shared database than on
    // fresh solvers, because learned clauses persist. Deterministic — exact on CI.
    //
    // The formula mirrors the real unsat-core workload: m independent UNSAT pairs
    // (all four combos of a_j, b_j excluded), every clause guarded by its own
    // selector variable s_k, queries assuming selector subsets.
    let pairs = 3usize;
    let base = 2 * pairs; // a_j = 2j, b_j = 2j+1
    let selectors = 4 * pairs; // one per clause
    let mut c = Cnf::new(base + selectors);
    let mut sel = Vec::new();
    for j in 0..pairs as u32 {
        let (a, b) = (2 * j, 2 * j + 1);
        for (i, combo) in [
            [SatLit::positive(a), SatLit::positive(b)],
            [SatLit::negative(a), SatLit::positive(b)],
            [SatLit::positive(a), SatLit::negative(b)],
            [SatLit::negative(a), SatLit::negative(b)],
        ]
        .iter()
        .enumerate()
        {
            let s = (base + 4 * j as usize + i) as u32;
            sel.push(SatLit::positive(s));
            c.add_clause(vec![SatLit::negative(s), combo[0], combo[1]]);
        }
    }
    // Query 0: every clause active (UNSAT). Queries 1..: drop one clause each —
    // the other pairs stay complete, so every query is UNSAT too.
    let mut queries = vec![sel.clone()];
    for i in 0..sel.len() {
        let mut q = sel.clone();
        q.remove(i);
        queries.push(q);
    }

    let mut shared = Incremental::new(&c);
    for q in &queries {
        assert!(matches!(shared.solve(q), Solved::Unsat(_)));
    }
    let shared_conflicts = shared.stats().conflicts;

    let mut scratch_conflicts = 0;
    for q in &queries {
        let mut fresh = Incremental::new(&c);
        assert!(matches!(fresh.solve(q), Solved::Unsat(_)));
        scratch_conflicts += fresh.stats().conflicts;
    }
    assert!(
        shared_conflicts < scratch_conflicts,
        "shared database must hit fewer conflicts: {shared_conflicts} vs {scratch_conflicts}"
    );
}
