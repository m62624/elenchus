//! Wall-clock benchmarks — **informational only, never a CI gate**.
//!
//! Wall-clock numbers depend on the machine, so shared CI runners cannot judge
//! them honestly; the merge-blocking performance evidence is the deterministic
//! work-counter gates in `tests/perf_gates.rs` (bit-identical on any hardware).
//! Run these locally with `cargo bench -p elenchus-solver`, or on a PR by
//! adding the `benchmark` label (see `.github/workflows/bench.yml`) — read
//! those CI numbers as a sanity trend, not a verdict.
use criterion::{Criterion, criterion_group, criterion_main};
use elenchus_solver::sat::{Cnf, Incremental, SatLit, SolverConfig, Var};
use elenchus_solver::verify_source;
use std::fmt::Write as _;
use std::hint::black_box;

/// Pigeonhole PHP(p, h) — the classic hard UNSAT family (see tests/perf_gates.rs).
fn php(p: usize, h: usize) -> Cnf {
    let v = |i: usize, j: usize| (i * h + j) as Var;
    let mut c = Cnf::new(p * h);
    for i in 0..p {
        c.add_clause((0..h).map(|j| SatLit::positive(v(i, j))).collect());
    }
    for j in 0..h {
        for a in 0..p {
            for b in (a + 1)..p {
                c.add_clause(vec![SatLit::negative(v(a, j)), SatLit::negative(v(b, j))]);
            }
        }
    }
    c
}

/// The raw CDCL core on a hard instance, reference vs turbo profile.
fn bench_php(c: &mut Criterion) {
    let cnf = php(8, 7);
    c.bench_function("php(8,7) reference", |b| {
        b.iter(|| {
            let mut inc = Incremental::new(black_box(&cnf));
            black_box(inc.solve(&[]))
        })
    });
    c.bench_function("php(8,7) turbo (ccmin)", |b| {
        b.iter(|| {
            let mut inc = Incremental::with_config(black_box(&cnf), SolverConfig::TURBO);
            black_box(inc.solve(&[]))
        })
    });
}

/// A deletion-minimization-shaped conflict program: many ASSUMEs, one
/// irreducible contradiction — exercises the incremental selector solver end to
/// end (retract + verified fixes) through the public engine API.
fn stress_retract_program(assumes: usize) -> String {
    let mut src = String::from("DOMAIN d\nFACT app is deployed\n");
    for i in 0..assumes {
        writeln!(src, "ASSUME app trait{i}").unwrap();
    }
    // The contradiction: an assumed atom is also denied.
    src.push_str("ASSUME app is stalled\nNOT app is stalled\nCHECK app\n");
    src
}

/// A TRY-heavy open program: every hypothesis gets its own counting session.
fn stress_try_program(tries: usize) -> String {
    let mut src = String::from(
        "DOMAIN d\nPREMISE gate:\n    WHEN app built\n    THEN app tested\nCHECK app BIDIRECTIONAL\n",
    );
    for i in 0..tries {
        writeln!(src, "TRY app step{i}").unwrap();
    }
    src.push_str("TRY app built\n");
    src
}

fn bench_engine(c: &mut Criterion) {
    let retract = stress_retract_program(40);
    c.bench_function("engine retract (40 ASSUME)", |b| {
        b.iter(|| black_box(verify_source("bench.vrf", black_box(&retract)).unwrap()))
    });
    let tries = stress_try_program(30);
    c.bench_function("engine TRY (31 hypotheses, bidirectional)", |b| {
        b.iter(|| black_box(verify_source("bench.vrf", black_box(&tries)).unwrap()))
    });
}

criterion_group!(benches, bench_php, bench_engine);
criterion_main!(benches);
