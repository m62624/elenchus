//! Exhaustive snapshots of every report output variant, so the rendered format
//! is standardized and locked: each Status (CONSISTENT / WARNING / UNDERDETERMINED
//! / CONFLICT) and each report element (DERIVED, the several conflict kinds, the
//! UNDERDETERMINED witness hint, SUMMARY, EXIT_CODE) gets a snapshot.

use elenchus_solver::verify_source;

fn report(src: &str) -> String {
    format!(
        "{}",
        verify_source("v.vrf", &format!("DOMAIN d\n{src}")).unwrap()
    )
}

// --- CONSISTENT ------------------------------------------------------------

#[test]
fn consistent_minimal() {
    insta::assert_snapshot!(report("FACT x a\nCHECK x\n"));
}

#[test]
fn consistent_with_derived() {
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#
    ));
}

#[test]
fn consistent_with_defeated_default() {
    // A defeasible RULE whose default is suppressed by an established UNLESS: the
    // report carries an informational DEFEATED line, verdict stays CONSISTENT.
    insta::assert_snapshot!(report(
        r#"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        FACT pengu is penguin
        CHECK
        "#
    ));
}

// --- WARNING ---------------------------------------------------------------

#[test]
fn warning_single() {
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#
    ));
}

#[test]
fn warning_multiple_with_derived() {
    insta::assert_snapshot!(report(
        r#"
        FACT s ready
        PREMISE need_two:
            WHEN s ready
            THEN s checked
            AND s signed
        RULE mark:
            WHEN s ready
            THEN s seen
        CHECK s
        "#
    ));
}

// --- CONFLICT (every kind) -------------------------------------------------

#[test]
fn conflict_exclusive_violation() {
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#
    ));
}

#[test]
fn conflict_implication_violation() {
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        NOT x b
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#
    ));
}

#[test]
fn conflict_fact_contradiction() {
    insta::assert_snapshot!(report("FACT x a\nNOT x a\nCHECK x\n"));
}

#[test]
fn conflict_derived_contradiction() {
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        NOT x b
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#
    ));
}

#[test]
fn conflict_multiple_sorted() {
    // A fact contradiction (line 1) and an EXCLUSIVE violation (line 5) — both
    // reported, ordered by source line.
    insta::assert_snapshot!(report(
        r#"
        FACT y c
        NOT y c
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#
    ));
}

#[test]
fn conflict_system_unsatisfiable() {
    // No single clause is violated under the (all-unknown) facts, but the premises
    // are jointly unsatisfiable — only the backward pass (BIDIRECTIONAL) finds it.
    // a→b, a→¬b force ¬a; ATLEAST(a,c) forces c; c→a then contradicts ¬a.
    insta::assert_snapshot!(report(
        r#"
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
        "#
    ));
}

// --- CONFLICT via ASSUME (RETRACT) -----------------------------------------

#[test]
fn conflict_assumptions_retract() {
    // FACT + PREMISE are consistent; the three ASSUME guesses can't all hold.
    // The report leads with a RETRACT block (no raw conflict pool) naming only
    // the assumptions — this snapshot locks that layout.
    insta::assert_snapshot!(report(
        r#"
        FACT rel reviewed
        PREMISE prod_needs_safety:
            WHEN rel in_prod
            THEN rel has_rollback
            OR   rel has_feature_flag
        ASSUME rel in_prod
        ASSUME NOT rel has_rollback
        ASSUME NOT rel has_feature_flag
        CHECK rel
        "#
    ));
}

#[test]
fn conflict_assume_vs_fact_retract() {
    // A hard FACT and a soft ASSUME collide: only the ASSUME is retractable, so
    // the RETRACT set names it (with its `NOT` polarity) and never the FACT.
    insta::assert_snapshot!(report("FACT x a\nASSUME NOT x a\nCHECK x\n"));
}

// --- UNDERDETERMINED -------------------------------------------------------

#[test]
fn underdetermined_with_witness_hint() {
    insta::assert_snapshot!(report(
        r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#
    ));
}

// --- EXISTS witness / unwitnessed ------------------------------------------

#[test]
fn conflict_exists_witness() {
    // The named witness is forced false → CONFLICT blamed on the EXISTS premise.
    insta::assert_snapshot!(report(
        "NOT auth is ready\nPREMISE covered:\n    EXISTS h WITNESS auth\n        h is ready\n"
    ));
}

#[test]
fn warning_exists_unwitnessed() {
    // EXISTS with no SET and no WITNESS → WARNING nudging to name a witness.
    insta::assert_snapshot!(report(
        "PREMISE someone_ready:\n    EXISTS h\n        h is ready\n"
    ));
}

// --- FACT … BECAUSE (justification) ----------------------------------------

#[test]
fn conflict_fact_because_false() {
    // The cited ground is FALSE → CONFLICT, with a trace explaining why.
    insta::assert_snapshot!(report(
        "NOT db reachable\nFACT api healthy BECAUSE db reachable\nCHECK api\n"
    ));
}

#[test]
fn warning_fact_because_unknown() {
    // The cited ground is UNKNOWN → WARNING nudging to establish it.
    insta::assert_snapshot!(report("FACT api healthy BECAUSE db reachable\nCHECK api\n"));
}
