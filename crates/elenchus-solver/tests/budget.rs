//! Pipeline-level conflict-budget tests.
//!
//! The budget is **one shared pool for the whole verification run** (backward
//! pass, core minimization, retract, TRY counting all draw from it), and it can
//! only withhold the report, never change it. The search is deterministic, so
//! the minimal sufficient limit probed here is the same on any hardware —
//! these tests cannot flake.
use elenchus_solver::{SolveOptions, Status, VerifyError, verify_source, verify_source_opts};
use std::fmt::Write as _;

fn opts(limit: u64) -> SolveOptions {
    SolveOptions {
        max_conflicts: Some(limit),
    }
}

/// A deletion-minimization-shaped conflict program (the bench workload): many
/// ASSUMEs, one irreducible contradiction — retract + verified fixes.
fn retract_program(assumes: usize) -> String {
    let mut src = String::from("DOMAIN d\nFACT app is deployed\n");
    for i in 0..assumes {
        writeln!(src, "ASSUME app trait{i}").unwrap();
    }
    src.push_str("ASSUME app is stalled\nNOT app is stalled\nCHECK app\n");
    src
}

/// Pigeonhole php(p, h) in the DSL: `ATLEAST` per pigeon, `EXCLUSIVE` per hole.
/// UNSAT for p > h and hard for resolution, so the backward pass and the core
/// minimization must analyze real conflicts — a budget target that cannot be
/// short-circuited by unit propagation or `analyzeFinal`.
fn php_program(pigeons: usize, holes: usize) -> String {
    let mut src = String::from("DOMAIN php\n");
    for i in 0..pigeons {
        writeln!(src, "PREMISE pigeon{i}:\n    ATLEAST").unwrap();
        for j in 0..holes {
            writeln!(src, "        p{i} in h{j}").unwrap();
        }
    }
    for j in 0..holes {
        writeln!(src, "PREMISE hole{j}:\n    EXCLUSIVE").unwrap();
        for i in 0..pigeons {
            writeln!(src, "        p{i} in h{j}").unwrap();
        }
    }
    src.push_str("CHECK p0 BIDIRECTIONAL\n");
    src
}

/// An *open* pigeonhole (p == h, satisfiable, many models) with one `TRY` per
/// atom: the backward pass and all 17 counting sessions (base + 16 hypotheses)
/// draw from the same pool — the cross-phase, global-budget workload.
fn try_php_program(n: usize) -> String {
    let mut src = php_program(n, n);
    for i in 0..n {
        for j in 0..n {
            writeln!(src, "TRY p{i} in h{j}").unwrap();
        }
    }
    src
}

/// The smallest limit that lets `src` verify. Budgets are monotone (a larger
/// pool re-runs the identical deterministic search with more allowance), so
/// binary search over "does it finish" is exact.
fn minimal_sufficient_limit(name: &str, src: &str) -> u64 {
    let mut hi = 1u64;
    while verify_source_opts(name, src, &[], &opts(hi)).is_err() {
        hi *= 2;
        assert!(hi < (1 << 24), "no reasonable budget suffices — a bug");
    }
    let mut lo = 0u64; // invariant: lo insufficient or zero, hi sufficient
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if verify_source_opts(name, src, &[], &opts(mid)).is_ok() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    if verify_source_opts(name, src, &[], &opts(lo)).is_ok() {
        lo
    } else {
        hi
    }
}

/// Default options are the plain entry point, byte for byte.
#[test]
fn default_options_reproduce_plain_verify() {
    for src in [retract_program(10), php_program(5, 4), try_php_program(4)] {
        let plain = verify_source("budget.vrf", &src).unwrap();
        let via_opts =
            verify_source_opts("budget.vrf", &src, &[], &SolveOptions::default()).unwrap();
        assert_eq!(via_opts, plain);
        assert_eq!(alloc_free_display(&via_opts), alloc_free_display(&plain));
    }
}

fn alloc_free_display(r: &elenchus_solver::Report) -> String {
    format!("{r}")
}

/// The pipeline boundary: at the probed minimal limit the report is identical
/// to the budget-free run; one conflict less is an explicit ConflictBudget
/// error carrying that limit — never a truncated or altered report.
#[test]
fn pipeline_boundary_on_core_and_try() {
    for (src, expect_status) in [
        // Closed pigeonhole: backward pass proves UNSAT, then core minimization.
        (php_program(5, 4), Status::Conflict),
        // Open pigeonhole + 16 TRY sessions: cross-phase pool sharing.
        (try_php_program(4), Status::Underdetermined),
    ] {
        let plain = verify_source("budget.vrf", &src).unwrap();
        assert_eq!(plain.status, expect_status);

        let minimal = minimal_sufficient_limit("budget.vrf", &src);
        assert!(minimal > 0, "these workloads must actually conflict");

        let budgeted = verify_source_opts("budget.vrf", &src, &[], &opts(minimal)).unwrap();
        assert_eq!(budgeted, plain);

        assert_eq!(
            verify_source_opts("budget.vrf", &src, &[], &opts(minimal - 1)),
            Err(VerifyError::ConflictBudget { limit: minimal - 1 })
        );
    }
}

/// The error surface: Display strings and the CompileError passthrough.
#[test]
fn verify_error_display_and_compile_passthrough() {
    let err = verify_source_opts("budget.vrf", &php_program(5, 4), &[], &opts(0)).unwrap_err();
    assert_eq!(format!("{err}"), "conflict budget exceeded (0 conflicts)");

    let bad = verify_source_opts("budget.vrf", "NOT a program", &[], &SolveOptions::default())
        .unwrap_err();
    let plain_err = verify_source("budget.vrf", "NOT a program").unwrap_err();
    match bad {
        VerifyError::Compile(e) => assert_eq!(e, plain_err),
        other => panic!("expected a compile error, got: {other}"),
    }
}

/// The resolver-based `_opts` entry point: same boundary contract, and the
/// `Error` trait surface (`source`, `Display`) behaves for both variants.
#[test]
fn verify_opts_resolver_boundary_and_error_trait() {
    use core::error::Error as _;
    use elenchus_solver::{MemoryResolver, verify, verify_opts};

    let src = php_program(5, 4);
    let mut r = MemoryResolver::new();
    r.add("root.vrf", &src);

    let plain = verify("root.vrf", &r).unwrap();
    let via_opts = verify_opts("root.vrf", &r, &[], &SolveOptions::default()).unwrap();
    assert_eq!(via_opts, plain);

    let err = verify_opts("root.vrf", &r, &[], &opts(0)).unwrap_err();
    assert_eq!(err, VerifyError::ConflictBudget { limit: 0 });
    assert!(
        err.source().is_none(),
        "a resource abort has no cause chain"
    );

    let bad = verify_opts("missing.vrf", &r, &[], &SolveOptions::default()).unwrap_err();
    assert!(bad.source().is_some(), "a compile error is the cause");
    let plain_msg = format!("{}", verify("missing.vrf", &r).unwrap_err());
    assert_eq!(format!("{bad}"), plain_msg, "Display passes through");
}
