//! Deterministic performance gates.
//!
//! Wall-clock is not a trustworthy metric on shared CI hardware, so these gates
//! measure **work**, not time: the solver's [`Stats`] counters are exact — the
//! search is deterministic, so every number here is bit-identical on any
//! machine. A heuristic stays in [`SolverConfig::TURBO`] only while it keeps
//! strictly reducing work on these workloads; wall-clock benchmarks live in
//! `benches/` and are informational only.
use elenchus_solver::sat::*;

/// Pigeonhole PHP(p, h): p pigeons into h holes — UNSAT for p > h, and hard for
/// resolution, so it makes the CDCL core actually work (many conflicts).
fn php(p: usize, h: usize) -> Cnf {
    let v = |i: usize, j: usize| (i * h + j) as Var;
    let mut c = Cnf::new(p * h);
    // Every pigeon sits somewhere…
    for i in 0..p {
        c.add_clause((0..h).map(|j| SatLit::positive(v(i, j))).collect());
    }
    // …and no hole holds two.
    for j in 0..h {
        for a in 0..p {
            for b in (a + 1)..p {
                c.add_clause(vec![SatLit::negative(v(a, j)), SatLit::negative(v(b, j))]);
            }
        }
    }
    c
}

fn solve_php_with(config: SolverConfig) -> Stats {
    let cnf = php(8, 7);
    let mut inc = Incremental::with_config(&cnf, config);
    assert!(matches!(inc.solve(&[]), Solved::Unsat(_)));
    inc.stats().clone()
}

/// The gate that keeps ccmin in TURBO: learned-clause minimization must cut
/// both the clause-learning volume and the number of conflicts on a hard UNSAT
/// instance. If this ever fails, the heuristic stopped paying for itself —
/// remove it rather than weakening the gate.
#[test]
fn ccmin_strictly_reduces_work_on_pigeonhole() {
    let reference = solve_php_with(SolverConfig::default());
    let ccmin = solve_php_with(SolverConfig { ccmin: true });
    assert!(
        ccmin.learned_literals < reference.learned_literals,
        "ccmin must shrink learned clauses: {} vs {}",
        ccmin.learned_literals,
        reference.learned_literals
    );
    assert!(
        ccmin.conflicts < reference.conflicts,
        "ccmin must reduce conflicts: {} vs {}",
        ccmin.conflicts,
        reference.conflicts
    );
    assert!(
        ccmin.propagations < reference.propagations,
        "ccmin must reduce propagation work: {} vs {}",
        ccmin.propagations,
        reference.propagations
    );
}

/// TURBO is exactly the measured winners — a change here must come with fresh
/// measurements (see the SolverConfig docs for the recorded numbers).
#[test]
fn turbo_profile_is_the_measured_winner_set() {
    assert_eq!(SolverConfig::TURBO, SolverConfig { ccmin: true });
}

/// Both profiles agree on the verdict, of course — heuristics only change the
/// path. (The full equivalence evidence is the brute-force proptests; this is
/// the smoke check at a size the oracle can't reach.)
#[test]
fn profiles_agree_on_pigeonhole_verdicts() {
    for (p, h, sat) in [(8usize, 7usize, false), (7, 7, true)] {
        let cnf = php(p, h);
        for config in [SolverConfig::default(), SolverConfig::TURBO] {
            let mut inc = Incremental::with_config(&cnf, config);
            assert_eq!(
                matches!(inc.solve(&[]), Solved::Sat(_)),
                sat,
                "php({p},{h})"
            );
        }
    }
}
