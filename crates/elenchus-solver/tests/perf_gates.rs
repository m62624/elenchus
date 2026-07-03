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
    assert!(matches!(inc.solve(&[]).unwrap(), Solved::Unsat(_)));
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
                matches!(inc.solve(&[]).unwrap(), Solved::Sat(_)),
                sat,
                "php({p},{h})"
            );
        }
    }
}

/// The exact work fingerprint of both profiles on php(8,7). The search is
/// deterministic, so these numbers are bit-identical on any machine — any
/// change here means the search *path* changed. That is exactly what a
/// "provably behavior-preserving" optimization (e.g. swapping the decision
/// scan for a heap) must NOT do; a deliberate search change must update this
/// fingerprint consciously, together with every affected snapshot.
#[test]
fn search_path_fingerprint_is_stable() {
    let reference = solve_php_with(SolverConfig::default());
    assert_eq!(
        (
            reference.decisions,
            reference.propagations,
            reference.conflicts,
            reference.learned_literals
        ),
        (4099, 44238, 3571, 66129),
        "reference profile fingerprint moved: {reference:?}"
    );
    let turbo = solve_php_with(SolverConfig::TURBO);
    assert_eq!(
        (
            turbo.decisions,
            turbo.propagations,
            turbo.conflicts,
            turbo.learned_literals
        ),
        (3550, 40677, 3165, 53970),
        "turbo profile fingerprint moved: {turbo:?}"
    );
}

/// Guarded-blocking model counting (the TRY path): one shared solver with a
/// retired guard per counting session must not do more decision/propagation
/// work than fresh solvers per session. This is the gate for the regression
/// class where stale sessions leak cost into later ones (e.g. un-retired
/// guards each becoming a free branching variable — the bug this caught).
/// The clause-loading and allocation savings of sharing are invisible to work
/// counters; wall-clock evidence for those lives in benches/sat.rs.
#[test]
fn guarded_counting_work_never_exceeds_scratch() {
    let hyps = 60usize;
    let base_vars = 2 + hyps; // one implication over vars 0,1 + free "TRY atoms"
    let project = [0 as Var, 1 as Var];

    // Count models (up to 2, distinct on `project`) of cnf ∧ assume, mirroring
    // the engine's guarded counting: mint a guard, block under it, then retire.
    let count2 = |inc: &mut Incremental, assume: &[SatLit]| -> usize {
        let guard = inc.add_var();
        let n = match inc.solve(assume).unwrap() {
            Solved::Unsat(_) => 0,
            Solved::Sat(model) => {
                let mut block = vec![SatLit::negative(guard)];
                block.extend(project.iter().map(|&v| {
                    if model[v as usize] {
                        SatLit::negative(v)
                    } else {
                        SatLit::positive(v)
                    }
                }));
                inc.add_clause(&block);
                let mut asm = assume.to_vec();
                asm.push(SatLit::positive(guard));
                match inc.solve(&asm).unwrap() {
                    Solved::Sat(_) => 2,
                    Solved::Unsat(_) => 1,
                }
            }
        };
        inc.add_clause(&[SatLit::negative(guard)]);
        n
    };

    // Shared: one solver, one lazily-minted guard per session (the engine's layout).
    let mut cnf = Cnf::new(base_vars);
    cnf.add_clause(vec![SatLit::negative(0), SatLit::positive(1)]);
    let mut shared = Incremental::new(&cnf);
    count2(&mut shared, &[]);
    for i in 0..hyps {
        let lit = SatLit::positive((2 + i) as Var);
        count2(&mut shared, &[lit]);
    }

    // Scratch: a fresh solver per session — the clone-per-hypothesis cost model
    // this replaced.
    let mut scratch = Stats::default();
    let mut sessions: Vec<Option<SatLit>> = vec![None];
    sessions.extend((0..hyps).map(|i| Some(SatLit::positive((2 + i) as Var))));
    for lit in sessions {
        let mut fresh = Incremental::new(&cnf);
        let asm: Vec<SatLit> = lit.into_iter().collect();
        count2(&mut fresh, &asm);
        let s = fresh.stats();
        scratch.decisions += s.decisions;
        scratch.propagations += s.propagations;
    }

    let sh = shared.stats();
    assert!(
        sh.decisions <= scratch.decisions,
        "shared guarded counting must not decide more than scratch: {} vs {}",
        sh.decisions,
        scratch.decisions
    );
    // Retiring a guard costs the shared solver exactly one root-level unit
    // propagation per session; a scratch solver is dropped before it would
    // propagate its own retirement unit, so it never pays it. Account for that
    // bounded O(1)/session difference precisely — no other slack.
    let retirements = (hyps + 1) as u64;
    assert!(
        sh.propagations <= scratch.propagations + retirements,
        "shared guarded counting must not propagate more than scratch (+1/session): {} vs {} + {}",
        sh.propagations,
        scratch.propagations,
        retirements
    );
}
