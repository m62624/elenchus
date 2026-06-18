//! Exhaustive snapshots of every report output variant, so the rendered format
//! is standardized and locked: each Status (CONSISTENT / WARNING / UNDERDETERMINED
//! / CONFLICT) and each report element (DERIVED, the several conflict kinds, the
//! UNDERDETERMINED witness hint, SUMMARY, EXIT_CODE) gets a snapshot.

use elenchus_solver::verify_source;

fn report(src: &str) -> String {
    format!("{}", verify_source("v.vrf", src).unwrap())
}

// --- CONSISTENT ------------------------------------------------------------

#[test]
fn consistent_minimal() {
    insta::assert_snapshot!(report("FACT x a\nCHECK x\n"));
}

#[test]
fn consistent_with_derived() {
    insta::assert_snapshot!(report(
        "FACT x a\nRULE r:\n    WHEN x a\n    THEN x b\nCHECK x\n"
    ));
}

// --- WARNING ---------------------------------------------------------------

#[test]
fn warning_single() {
    insta::assert_snapshot!(report(
        "FACT x a\nAXIOM w:\n    WHEN x a\n    THEN x b\nCHECK x\n"
    ));
}

#[test]
fn warning_multiple_with_derived() {
    insta::assert_snapshot!(report(
        "FACT s ready\nAXIOM need_two:\n    WHEN s ready\n    THEN s checked\n    AND s signed\nRULE mark:\n    WHEN s ready\n    THEN s seen\nCHECK s\n"
    ));
}

// --- CONFLICT (every kind) -------------------------------------------------

#[test]
fn conflict_exclusive_violation() {
    insta::assert_snapshot!(report(
        "FACT x a\nFACT x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n"
    ));
}

#[test]
fn conflict_implication_violation() {
    insta::assert_snapshot!(report(
        "FACT x a\nNOT x b\nAXIOM w:\n    WHEN x a\n    THEN x b\nCHECK x\n"
    ));
}

#[test]
fn conflict_fact_contradiction() {
    insta::assert_snapshot!(report("FACT x a\nNOT x a\nCHECK x\n"));
}

#[test]
fn conflict_derived_contradiction() {
    insta::assert_snapshot!(report(
        "FACT x a\nNOT x b\nRULE r:\n    WHEN x a\n    THEN x b\nCHECK x\n"
    ));
}

#[test]
fn conflict_multiple_sorted() {
    // A fact contradiction (line 1) and an EXCLUSIVE violation (line 5) — both
    // reported, ordered by source line.
    insta::assert_snapshot!(report(
        "FACT y c\nNOT y c\nFACT x a\nFACT x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n"
    ));
}

#[test]
fn conflict_system_unsatisfiable() {
    // No single clause is violated under the (all-unknown) facts, but the axioms
    // are jointly unsatisfiable — only the backward pass (BIDIRECTIONAL) finds it.
    // a→b, a→¬b force ¬a; ATLEAST(a,c) forces c; c→a then contradicts ¬a.
    insta::assert_snapshot!(report(
        "AXIOM a_implies_b:\n    WHEN x a\n    THEN x b\nAXIOM a_implies_not_b:\n    WHEN x a\n    THEN NOT x b\nAXIOM atleast_a_c:\n    ATLEAST\n        x a\n        x c\nAXIOM c_implies_a:\n    WHEN x c\n    THEN x a\nCHECK x BIDIRECTIONAL\n"
    ));
}

// --- UNDERDETERMINED -------------------------------------------------------

#[test]
fn underdetermined_with_witness_hint() {
    insta::assert_snapshot!(report(
        "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n"
    ));
}
