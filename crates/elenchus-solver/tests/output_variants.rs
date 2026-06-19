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
    insta::assert_snapshot!(report(r#"
        FACT x a
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#));
}

// --- WARNING ---------------------------------------------------------------

#[test]
fn warning_single() {
    insta::assert_snapshot!(report(r#"
        FACT x a
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#));
}

#[test]
fn warning_multiple_with_derived() {
    insta::assert_snapshot!(report(r#"
        FACT s ready
        PREMISE need_two:
            WHEN s ready
            THEN s checked
            AND s signed
        RULE mark:
            WHEN s ready
            THEN s seen
        CHECK s
        "#));
}

// --- CONFLICT (every kind) -------------------------------------------------

#[test]
fn conflict_exclusive_violation() {
    insta::assert_snapshot!(report(r#"
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#));
}

#[test]
fn conflict_implication_violation() {
    insta::assert_snapshot!(report(r#"
        FACT x a
        NOT x b
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#));
}

#[test]
fn conflict_fact_contradiction() {
    insta::assert_snapshot!(report("FACT x a\nNOT x a\nCHECK x\n"));
}

#[test]
fn conflict_derived_contradiction() {
    insta::assert_snapshot!(report(r#"
        FACT x a
        NOT x b
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#));
}

#[test]
fn conflict_multiple_sorted() {
    // A fact contradiction (line 1) and an EXCLUSIVE violation (line 5) — both
    // reported, ordered by source line.
    insta::assert_snapshot!(report(r#"
        FACT y c
        NOT y c
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#));
}

#[test]
fn conflict_system_unsatisfiable() {
    // No single clause is violated under the (all-unknown) facts, but the premises
    // are jointly unsatisfiable — only the backward pass (BIDIRECTIONAL) finds it.
    // a→b, a→¬b force ¬a; ATLEAST(a,c) forces c; c→a then contradicts ¬a.
    insta::assert_snapshot!(report(r#"
        PREMISE a_implies_b:
            WHEN x a
            THEN x b
        PREMISE a_implies_not_b:
            WHEN x a
            THEN NOT x b
        PREMISE atleast_a_c:
            ATLEAST
                x a
                x c
        PREMISE c_implies_a:
            WHEN x c
            THEN x a
        CHECK x BIDIRECTIONAL
        "#));
}

// --- UNDERDETERMINED -------------------------------------------------------

#[test]
fn underdetermined_with_witness_hint() {
    insta::assert_snapshot!(report(r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#));
}
