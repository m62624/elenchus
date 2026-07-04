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

// --- TRY (abduction / L5) --------------------------------------------------

#[test]
fn try_closes_the_gap() {
    // An open model (a RULE whose antecedent is free): TRYing the antecedent pins
    // it, so the engine reports the hypothesis would close the gap. Verdict stays
    // UNDERDETERMINED — TRY is advisory, it never commits the candidate.
    insta::assert_snapshot!(report(
        r#"
        RULE gate:
            WHEN deploys is_ready
            THEN deploys unblocked
        CHECK BIDIRECTIONAL
        TRY deploys is_ready
        "#
    ));
}

#[test]
fn try_conflicts_with_established() {
    // A hypothesis that contradicts a FACT: the engine reports it would conflict.
    insta::assert_snapshot!(report(
        r#"
        FACT deploys is_ready
        RULE gate:
            WHEN deploys is_ready
            THEN deploys unblocked
        CHECK BIDIRECTIONAL
        TRY NOT deploys is_ready
        "#
    ));
}

#[test]
fn try_leaves_it_still_open() {
    // A hypothesis that pins one part but leaves another free: the model is still
    // not unique, so the engine reports the gap stays open.
    insta::assert_snapshot!(report(
        r#"
        RULE gate:
            WHEN deploys is_ready
            THEN deploys unblocked
        RULE gate2:
            WHEN backup done
            THEN backup safe
        CHECK BIDIRECTIONAL
        TRY deploys is_ready
        "#
    ));
}

// --- PROVE (entailment / the ⊨ goal) ----------------------------------------

#[test]
fn prove_proved_and_refuted() {
    // The theory entails the first goal (a fact + a rule force it) and refutes the
    // second (its negation is asserted). Both are advisory: verdict CONSISTENT.
    insta::assert_snapshot!(report(
        r#"
        FACT socrates is human
        NOT socrates is divine
        RULE mortal:
            WHEN socrates is human
            THEN socrates is mortal
        PROVE socrates is mortal
        PROVE socrates is divine
        CHECK socrates
        "#
    ));
}

#[test]
fn prove_open_goal() {
    // Nothing pins the goal either way — the honest three-valued answer is OPEN.
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        RULE r:
            WHEN x b
            THEN x c
        PROVE x c
        CHECK x
        "#
    ));
}

#[test]
fn prove_vacuous_on_inconsistent_theory() {
    // A contradictory theory entails everything; the goal line says so instead of
    // pretending the goal was meaningfully PROVED.
    insta::assert_snapshot!(report(
        r#"
        FACT x a
        NOT x a
        PROVE x b
        "#
    ));
}

#[test]
fn prove_negative_goal() {
    // `PROVE NOT …` asks entailment of the negation; the label keeps the polarity.
    insta::assert_snapshot!(report(
        r#"
        NOT door open
        PROVE NOT door open
        CHECK door
        "#
    ));
}

// --- HENCE … FROM (checked derivation / the proof kernel) --------------------

#[test]
fn hence_chain_holds_then_gap() {
    // Step 1 is a valid inference from what it cites; step 2 cites only step 1's
    // conclusion, which does not entail being buried — the gap is named on its
    // own line. Advisory: the verdict is untouched.
    insta::assert_snapshot!(report(
        r#"
        FACT socrates is human
        RULE all_mortal:
            WHEN socrates is human
            THEN socrates is mortal
        HENCE socrates is mortal FROM all_mortal, socrates is human
        HENCE socrates is buried FROM socrates is mortal
        CHECK socrates
        "#
    ));
}

// --- FOR EACH <x> MENTIONED (the universal schema) ---------------------------

#[test]
fn mentioned_schema_derives_for_every_written_individual() {
    // The classical syllogism with no SET: whoever is written about is covered.
    insta::assert_snapshot!(report(
        r#"
        FACT socrates is human
        FACT plato is human
        NOT rock is human
        RULE mortal FOR EACH x MENTIONED:
            WHEN x is human
            THEN x is mortal
        CHECK
        "#
    ));
}

// --- KNOWS / BELIEVES: the modal/epistemic (L6) layer -----------------------

#[test]
fn knows_a_falsehood_is_a_conflict() {
    // Knowledge is factive (axiom T): you cannot know what the world establishes
    // FALSE. `alice KNOWS door locked` against `NOT door locked` is a CONFLICT.
    insta::assert_snapshot!(report("NOT door locked\nKNOWS alice door locked\n"));
}

#[test]
fn knows_a_truth_is_silent() {
    // Knowing something the world establishes TRUE holds — no report, CONSISTENT.
    insta::assert_snapshot!(report("FACT door locked\nKNOWS alice door locked\n"));
}

#[test]
fn knows_the_unestablished_is_a_warning() {
    // A knowledge claim the world has not established cannot be confirmed factive:
    // a WARNING nudging you to assert it or downgrade to BELIEVES.
    insta::assert_snapshot!(report("KNOWS alice door locked\n"));
}

#[test]
fn believes_a_falsehood_is_a_false_belief() {
    // Belief is non-factive: a false belief is reported (WARNING-level) but is never
    // a CONFLICT — the world stays consistent, bob is simply wrong.
    insta::assert_snapshot!(report("NOT door locked\nBELIEVES bob door locked\n"));
}

#[test]
fn knowing_both_polarities_is_incoherent() {
    // One agent that KNOWS both φ and ¬φ is incoherent (axiom T makes both true): a
    // single CONFLICT, with no redundant per-claim "unconfirmed" warnings.
    insta::assert_snapshot!(report("KNOWS a x p\nKNOWS a NOT x p\n"));
}

#[test]
fn knows_a_negated_truth_is_silent() {
    // Negated knowledge works: alice correctly knows the door is NOT locked.
    insta::assert_snapshot!(report("NOT door locked\nKNOWS alice NOT door locked\n"));
}

#[test]
fn believes_the_unestablished_is_silent() {
    // Believing something the world has not established is allowed and unremarkable
    // (belief is non-factive): no report, CONSISTENT.
    insta::assert_snapshot!(report("BELIEVES bob door locked\n"));
}
