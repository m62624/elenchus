//! Behavioural tests for the solver, driven entirely through the public API
//! (parse → compile → solve), so they live as integration tests.
use elenchus_compiler::{KIND_UNSAT, Value, kw};
use elenchus_solver::*;
use std::collections::BTreeSet;

/// Verify a single inline source under a default `DOMAIN t` (so test programs
/// need not repeat it); atoms land in domain `t`, labelled `t.subject …`.
fn vs(src: &str) -> Result<Report, CompileError> {
    verify_source("t.vrf", &format!("DOMAIN t\n{src}"))
}

#[test]
fn clean_consistent() {
    let r = vs("FACT x a\nCHECK x\n").unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty() && r.warnings.is_empty());
}

#[test]
fn fact_contradiction_is_conflict() {
    let r = vs("FACT x a\nNOT x a\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts.len(), 1);
}

#[test]
fn exclusive_violation_is_conflict() {
    let src = include_str!("../../../docs/examples/conflict.vrf");
    let r = verify_source("conflict.vrf", src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(
        r.conflicts[0].origin.premise.as_deref(),
        Some("fly_xor_swim")
    );
    assert_eq!(r.conflicts[0].atoms.len(), 2);
}

#[test]
fn exclusive_with_unknown_is_consistent_not_warning() {
    // flying TRUE, swimming UNKNOWN — at most one can hold, no conflict, no warning.
    let r = vs(r"
        FACT A has flying
        PREMISE e:
            EXCLUSIVE
                A has flying
                A has swimming
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.warnings.is_empty());
}

#[test]
fn exists_witness_that_holds_is_consistent() {
    // The author names the witness; the engine checks it holds. `auth is ready` is
    // asserted, so the existential is satisfied — no SET needed.
    let r = vs("FACT auth is ready\nPREMISE p:\n    EXISTS h WITNESS auth\n        h is ready\nCHECK auth\n")
        .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty());
}

#[test]
fn exists_witness_that_fails_is_conflict() {
    // The named witness is forced FALSE elsewhere → the existential cannot be met by
    // it: CONFLICT, blamed on the EXISTS premise.
    let r = vs("NOT auth is ready\nPREMISE p:\n    EXISTS h WITNESS auth\n        h is ready\n")
        .unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].origin.premise.as_deref(), Some("p"));
    assert_eq!(r.conflicts[0].origin.kind, kw::EXISTS);
}

#[test]
fn exists_unwitnessed_is_warning() {
    // ∃ that names neither a SET nor a WITNESS cannot be checked — the forge voice:
    // WARNING with a hint to name a witness, not a clause and not a blow-up.
    let r = vs("PREMISE someone_ready:\n    EXISTS h\n        h is ready\n").unwrap();
    assert_eq!(r.status, Status::Warning);
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(
        r.warnings[0].origin.premise.as_deref(),
        Some("someone_ready")
    );
    assert_eq!(r.warnings[0].blocked_by, vec![String::from("t.h is ready")]);
    assert!(r.warnings[0].hint.as_deref().unwrap().contains("WITNESS"));
}

#[test]
fn exists_unwitnessed_warning_does_not_hide_a_conflict() {
    // A real CONFLICT still wins over the unwitnessed-∃ WARNING (verdict precedence).
    let r = vs("FACT x a\nNOT x a\nPREMISE p:\n    EXISTS h\n        h is ready\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
}

// --- FACT … BECAUSE (justification / L2) -----------------------------------

#[test]
fn fact_because_ground_holds_is_consistent() {
    // The author cites a ground; the engine checks it holds. `db reachable` is
    // asserted TRUE, so the justification stands — no clause, no noise.
    let r = vs("FACT db reachable\nFACT api healthy BECAUSE db reachable\nCHECK api\n").unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty() && r.warnings.is_empty());
    // Neither the belief nor the ground is an inert leftover: the justification uses
    // both, so the orphan lint must stay silent.
    assert!(r.orphans.is_empty());
}

#[test]
fn fact_because_ground_derived_by_rule_holds_is_consistent() {
    // The ground need not be asserted directly — a RULE that derives it also
    // satisfies the justification (the check reads the settled model value).
    let r = vs(
        "FACT db up\nRULE r:\n    WHEN db up\n    THEN db reachable\nFACT api healthy BECAUSE db reachable\nCHECK api\n",
    )
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
}

#[test]
fn fact_because_ground_false_is_conflict() {
    // The cited ground is forced FALSE → the stated reason does not hold: CONFLICT,
    // blamed on the BECAUSE, and its trace explains why the ground is false.
    let r = vs("NOT db reachable\nFACT api healthy BECAUSE db reachable\nCHECK api\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].origin.kind, kw::BECAUSE);
    assert!(r.conflicts[0].atoms[0].contains("t.db reachable is FALSE"));
    // The trace names the NOT that forced the ground false.
    assert!(!r.conflicts[0].trace.is_empty());
}

#[test]
fn fact_because_ground_unknown_is_warning() {
    // The cited ground is never established (stays UNKNOWN) → the reason is
    // unestablished: WARNING with a hint to establish it, not a clause.
    let r = vs("FACT api healthy BECAUSE db reachable\nCHECK api\n").unwrap();
    assert_eq!(r.status, Status::Warning);
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(r.warnings[0].origin.kind, kw::BECAUSE);
    assert_eq!(
        r.warnings[0].blocked_by,
        vec![String::from("t.db reachable")]
    );
    assert!(r.warnings[0].hint.as_deref().unwrap().contains("FACT"));
}

#[test]
fn fact_because_unknown_ground_does_not_hide_a_conflict() {
    // A real CONFLICT wins over the unestablished-ground WARNING (verdict precedence).
    let r = vs("FACT x a\nNOT x a\nFACT api healthy BECAUSE db reachable\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
}

#[test]
fn plain_fact_is_unaffected_by_the_justification_layer() {
    // A FACT with no BECAUSE never produces a justification conflict/warning.
    let r = vs("FACT api healthy\nCHECK api\n").unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty() && r.warnings.is_empty());
}

#[test]
fn because_chain_all_grounds_hold_is_consistent() {
    // A ground may itself be a justified FACT, so BECAUSE composes: each link is
    // checked independently and, when every link holds, the whole chain stands. No
    // extra machinery — the chain is emergent from the one-hop check.
    let r = vs(
        "FACT net up\nFACT db reachable BECAUSE net up\nFACT api healthy BECAUSE db reachable\nCHECK api\n",
    )
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty() && r.warnings.is_empty());
}

#[test]
fn because_chain_surfaces_the_weakest_link() {
    // The deepest ground (`net up`) is never established; the chain's WARNING points
    // straight at that weakest link, not at the top-level claim.
    let r =
        vs("FACT db reachable BECAUSE net up\nFACT api healthy BECAUSE db reachable\nCHECK api\n")
            .unwrap();
    assert_eq!(r.status, Status::Warning);
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(r.warnings[0].blocked_by, vec![String::from("t.net up")]);
}

#[test]
fn because_chain_deep_false_ground_is_conflict() {
    // A false ground anywhere in the chain surfaces as a CONFLICT at that link.
    let r = vs(
        "NOT net up\nFACT db reachable BECAUSE net up\nFACT api healthy BECAUSE db reachable\nCHECK api\n",
    )
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert!(r.conflicts[0].atoms[0].contains("t.net up is FALSE"));
}

#[test]
fn fact_because_unknown_ground_under_bidirectional_stays_warning() {
    // The interned-but-free ground does not manufacture a spurious UNDERDETERMINED
    // under BIDIRECTIONAL (the backward pass does not project an unconstrained
    // ground); the justification WARNING is the verdict.
    let r = vs("FACT api healthy BECAUSE db reachable\nCHECK BIDIRECTIONAL\n").unwrap();
    assert_eq!(r.status, Status::Warning);
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(r.warnings[0].origin.kind, kw::BECAUSE);
}

#[test]
fn implication_missing_consequent_is_warning() {
    // WHEN flying THEN wing: flying TRUE, wing UNKNOWN → blocked → WARNING.
    let r = vs(r#"
        FACT A has flying
        PREMISE w:
            WHEN A has flying
            THEN A has wing
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Warning);
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(r.warnings[0].blocked_by, vec![String::from("t.A has wing")]);
}

#[test]
fn warning_hint_points_at_rule_when_atom_is_a_free_input() {
    // `A has wing` is nothing's consequent: it can only be set by a FACT, or by
    // turning a PREMISE meant to establish it into a RULE. The hint says so.
    let r = vs(r"
        FACT A has flying
        PREMISE w:
            WHEN A has flying
            THEN A has wing
        ")
    .unwrap();
    let hint = r.warnings[0].hint.as_deref().unwrap();
    assert!(hint.contains("make that PREMISE a RULE"), "{hint}");
    assert!(hint.contains("t.A has wing"), "{hint}");
}

#[test]
fn warning_hint_points_at_antecedent_when_a_rule_could_derive_it() {
    // `c ready` IS a RULE consequent, but the rule has not fired (its `x trigger`
    // is UNKNOWN). The blocking premise's hint sends you to the rule's input.
    let r = vs(r"
        RULE d:
            WHEN x trigger
            THEN c ready
        FACT go now
        PREMISE p:
            WHEN go now
            THEN c ready
        ")
    .unwrap();
    assert_eq!(r.status, Status::Warning);
    let hint = r.warnings[0].hint.as_deref().unwrap();
    assert!(
        hint.contains("derived by a RULE that has not fired"),
        "{hint}"
    );
}

#[test]
fn human_report_dedupes_repeated_fix_lines() {
    // Three *distinct* premises all blocked: p1 and p2 by the SAME atom
    // (`gate one_ok`, via different antecedents so the clauses don't dedupe)
    // → one identical fix, deduped; p3 by a different atom → its own fix. So:
    // 3 warnings, but only 2 distinct `fix:` lines in the human report (while
    // every warning still carries its hint in the structured data / JSON).
    let r = vs(r"
        FACT a on
        FACT b on
        PREMISE p1:
            WHEN a on
            THEN gate one_ok
        PREMISE p2:
            WHEN b on
            THEN gate one_ok
        PREMISE p3:
            WHEN a on
            THEN gate two_ok
        ")
    .unwrap();
    assert_eq!(r.warnings.len(), 3);
    // Every warning keeps its hint in the structured form.
    assert!(r.warnings.iter().all(|w| w.hint.is_some()));
    let distinct_hints: BTreeSet<&str> = r
        .warnings
        .iter()
        .filter_map(|w| w.hint.as_deref())
        .collect();
    assert_eq!(distinct_hints.len(), 2);
    // The human report prints exactly one `fix:` per distinct hint.
    let text = format!("{r}");
    let shown = text
        .lines()
        .filter(|l| l.trim_start().starts_with("fix:"))
        .count();
    assert_eq!(shown, distinct_hints.len());
}

#[test]
fn implication_satisfied_is_consistent() {
    let r = vs(r"
        FACT A has flying
        FACT A has wing
        PREMISE w:
            WHEN A has flying
            THEN A has wing
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
}

#[test]
fn implication_violated_is_conflict() {
    // antecedent TRUE, consequent FALSE → CONFLICT.
    let r = vs(r"
        FACT A has flying
        NOT A has wing
        PREMISE w:
            WHEN A has flying
            THEN A has wing
        ")
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
}

#[test]
fn rule_derives_fact() {
    let r = vs(r#"
        FACT A has flying
        RULE o:
            WHEN A has flying
            THEN A needs oxygen
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert_eq!(r.derived.len(), 1);
    assert_eq!(r.derived[0].atom, "t.A needs oxygen");
}

#[test]
fn rule_derivation_contradiction_is_conflict() {
    // rule derives `A needs oxygen` TRUE, but it's asserted FALSE.
    let r = vs(r"
        FACT A has flying
        NOT A needs oxygen
        RULE o:
            WHEN A has flying
            THEN A needs oxygen
        ")
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
}

#[test]
fn bidirectional_finds_alternative_model_underdetermined() {
    // EXCLUSIVE(a,b) with no facts: {FF, TF, FT} all satisfy → not unique.
    let r = vs(r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Underdetermined);
}

#[test]
fn fact_pins_unique_model_consistent() {
    // Same premise, but FACT x a forces b false → the only model → CONSISTENT.
    let r = vs(r#"
        FACT x a
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
}

#[test]
fn no_bidirectional_skips_backward_pass() {
    // Plain CHECK: alternatives are not searched → stays CONSISTENT, not UNDERDETERMINED.
    let r = vs(r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
}

#[test]
fn creature_example_forward_pass() {
    let src = include_str!("../../../docs/examples/creature.vrf");
    let r = verify_source("creature.vrf", src).unwrap();
    // fly_xor_swim & no_dual_temp consistent; wings_need_bone → 2 warnings
    // (wing, bone); needs_oxygen derived; no conflicts.
    assert_eq!(r.status, Status::Warning);
    assert!(r.conflicts.is_empty());
    assert_eq!(r.warnings.len(), 2);
    assert_eq!(r.derived.len(), 1);
    assert_eq!(r.derived[0].atom, "creatures.Creature_A needs oxygen");
}

#[test]
fn roles_puzzle_is_uniquely_solved() {
    // 3 people × 3 roles, ONEOF per person and per role, two clues. The
    // backward (SAT) pass must find the assignment satisfiable AND unique —
    // i.e. CONSISTENT, not UNDERDETERMINED.
    let src = include_str!("../../../docs/examples/roles-puzzle.vrf");
    let r = verify_source("roles-puzzle.vrf", src).unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty());
    assert!(r.underdetermined.is_none());
}

#[test]
fn roles_puzzle_underdetermined_without_a_clue() {
    // Drop the `NOT bob is qa` clue and the solution is no longer unique
    // (bob/carol can swap dev/qa) — the SAT pass reports UNDERDETERMINED.
    // Normalize CRLF first: on a Windows checkout include_str! embeds the file
    // with \r\n, so a literal "...\n" match would otherwise miss the line.
    let src = include_str!("../../../docs/examples/roles-puzzle.vrf")
        .replace("\r\n", "\n")
        .replace("NOT  bob is qa\n", "");
    let r = verify_source("roles-puzzle.vrf", &src).unwrap();
    assert_eq!(r.status, Status::Underdetermined);
}

#[test]
fn socrates_chain_is_a_conflict() {
    // human → animal → living → mortal (3 derivations), then mortal EXCLUSIVE
    // immortal with `immortal` asserted → CONFLICT on the exclusivity premise.
    let src = include_str!("../../../docs/examples/socrates.vrf");
    let r = verify_source("socrates.vrf", src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(
        r.conflicts[0].origin.premise.as_deref(),
        Some("mortal_xor_immortal")
    );
    assert_eq!(r.derived.len(), 3); // animal, living, mortal
}

// --- conflict explainability: derivation trace + minimal unsat core ------

#[test]
fn forward_conflict_carries_a_trace_of_its_facts() {
    let r = vs(r#"
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#)
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
    let t = &r.conflicts[0].trace;
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].atom, "t.x a");
    assert_eq!(t[0].value, Value::True);
    assert!(matches!(t[0].reason, TraceReason::Asserted(_)));
    assert!(r.unsat_core.is_empty());
}

#[test]
fn derivation_chain_is_traced_back_to_the_root_fact() {
    // human → animal → living → mortal, then mortal XOR immortal (immortal asserted).
    let src = include_str!("../../../docs/examples/socrates.vrf");
    let r = verify_source("socrates.vrf", src).unwrap();
    let t = &r.conflicts[0].trace;
    // human (fact) + animal, living, mortal (derived) + immortal (fact) = 5 steps,
    // supports before dependents.
    assert_eq!(t.len(), 5);
    assert_eq!(t[0].atom, "philosophy.socrates is human");
    assert!(matches!(t[0].reason, TraceReason::Asserted(_)));
    let mortal = t
        .iter()
        .find(|s| s.atom == "philosophy.socrates is mortal")
        .unwrap();
    match &mortal.reason {
        TraceReason::Derived { from, .. } => {
            assert_eq!(from, &vec![String::from("philosophy.socrates is living")]);
        }
        _ => panic!("mortal should be derived, not asserted"),
    }
}

#[test]
fn direct_fact_contradiction_has_no_trace() {
    let r = vs("FACT x a\nNOT x a\nCHECK x\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert!(r.conflicts[0].trace.is_empty());
}

#[test]
fn jointly_unsatisfiable_reports_a_minimal_core() {
    // ONEOF(a,b); a→c; b→c; NOT c. Unsat only via case-split, so the forward
    // pass misses it and the backward pass produces the core.
    let src = r#"
        PREMISE one:
            ONEOF
                x a
                x b
        PREMISE ac:
            WHEN x a
            THEN x c
        PREMISE bc:
            WHEN x b
            THEN x c
        NOT x c
        CHECK x BIDIRECTIONAL
        "#;
    let r = vs(src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts[0].origin.kind, KIND_UNSAT);
    assert_eq!(r.unsat_core.len(), 4);
    let labels: Vec<&str> = r.unsat_core.iter().map(|c| c.label.as_str()).collect();
    assert!(labels.contains(&"one"));
    assert!(labels.contains(&"t.x c")); // the bare NOT fact is labelled by its atom
}

#[test]
fn unsat_core_excludes_irrelevant_constructs() {
    // The same unsat cluster, plus an unrelated fact + premise that must not
    // appear in the (irreducible) core.
    let src = r#"
        PREMISE one:
            ONEOF
                x a
                x b
        PREMISE ac:
            WHEN x a
            THEN x c
        PREMISE bc:
            WHEN x b
            THEN x c
        NOT x c
        FACT z here
        PREMISE noise:
            EXCLUSIVE
                z here
                z gone
        CHECK x BIDIRECTIONAL
        "#;
    let r = vs(src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.unsat_core.len(), 4);
    let labels: Vec<&str> = r.unsat_core.iter().map(|c| c.label.as_str()).collect();
    assert!(!labels.contains(&"noise"));
    assert!(!labels.iter().any(|l| l.contains("here")));
}

#[test]
fn unsat_core_blames_the_rules_that_form_it() {
    // Same case-split unsat as `jointly_unsatisfiable_reports_a_minimal_core`,
    // but the implications are RULEs (→ `c.rules`), not PREMISEs (→ `c.clauses`).
    // This is the only path that drives the RULE branch of `constructs`, so the
    // minimal core must name the two rules (plus `one` and the `NOT x c` fact).
    let src = r#"
        PREMISE one:
            ONEOF
                x a
                x b
        RULE ac:
            WHEN x a
            THEN x c
        RULE bc:
            WHEN x b
            THEN x c
        NOT x c
        CHECK x BIDIRECTIONAL
        "#;
    let r = vs(src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.conflicts[0].origin.kind, KIND_UNSAT);
    assert_eq!(r.unsat_core.len(), 4);
    let labels: Vec<&str> = r.unsat_core.iter().map(|c| c.label.as_str()).collect();
    assert!(labels.contains(&"one"));
    assert!(labels.contains(&"ac"));
    assert!(labels.contains(&"bc"));
    assert!(labels.contains(&"t.x c")); // the bare NOT fact, labelled by its atom
}

#[test]
fn consistent_report_has_empty_core_and_no_trace() {
    let r = vs("FACT x a\nCHECK x BIDIRECTIONAL\n").unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.unsat_core.is_empty());
    assert!(r.conflicts.is_empty());
}

// --- ASSUME: soft (retractable) hypotheses -----------------------------

#[test]
fn compatible_assumptions_behave_like_facts() {
    // ASSUME that does not clash with anything → ordinary CONSISTENT, and the
    // assumption participates like a fact (no retract, no conflict).
    let r = vs("ASSUME rel in_prod\nFACT rel reviewed\nCHECK rel\n").unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.retract.is_empty());
    assert!(r.conflicts.is_empty());
}

#[test]
fn assume_drives_a_rule_like_a_fact() {
    // A soft assumption fires forward chaining just like a hard fact.
    let r = vs(r"
        ASSUME A has flying
        RULE o:
            WHEN A has flying
            THEN A needs oxygen
        CHECK A
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert_eq!(r.derived.len(), 1);
    assert_eq!(r.derived[0].atom, "t.A needs oxygen");
}

#[test]
fn clashing_assumptions_yield_conflict_with_a_retract_set() {
    // in_prod needs a rollback OR a feature flag; assuming in_prod plus
    // neither makes the premise unsatisfiable — but only via the guesses.
    let src = r#"
        FACT rel reviewed
        PREMISE prod_needs_safety:
            WHEN rel in_prod
            THEN rel has_rollback
            OR   rel has_feature_flag
        ASSUME rel in_prod
        ASSUME NOT rel has_rollback
        ASSUME NOT rel has_feature_flag
        CHECK rel
        "#;
    let r = vs(src).unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.exit_code(), 2);
    // All three guesses are jointly to blame: dropping any one fixes it.
    assert_eq!(r.retract.len(), 3, "{:?}", r.retract);
    let labels: Vec<&str> = r.retract.iter().map(|it| it.label.as_str()).collect();
    assert!(labels.contains(&"t.rel in_prod"));
    assert!(labels.contains(&"NOT t.rel has_rollback"));
    assert!(labels.contains(&"NOT t.rel has_feature_flag"));
    // Every retract item is an ASSUME — a FACT/PREMISE is never blamed.
    assert!(r.retract.iter().all(|it| it.origin.kind == kw::ASSUME));
    // The human report leads with RETRACT and hides the raw conflict pool.
    let shown = format!("{r}");
    assert!(shown.contains("RETRACT"), "{shown}");
    assert!(!shown.contains("CONFLICT  "), "{shown}");
}

#[test]
fn assume_vs_fact_retracts_only_the_assumption() {
    // FACT x a is ground truth; ASSUME NOT x a is the only removable thing.
    let r = vs("FACT x a\nASSUME NOT x a\nCHECK x\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.retract.len(), 1);
    assert_eq!(r.retract[0].label, "NOT t.x a");
    assert_eq!(r.retract[0].origin.kind, kw::ASSUME);
}

#[test]
fn hard_conflict_is_not_blamed_on_assumptions() {
    // The FACTs themselves contradict; an unrelated ASSUME must NOT appear in
    // a retract set (the hard program is already broken).
    let r = vs("FACT x a\nNOT x a\nASSUME y b\nCHECK x\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert!(r.retract.is_empty(), "{:?}", r.retract);
}

#[test]
fn two_assumptions_directly_contradict() {
    let r = vs("ASSUME x a\nASSUME NOT x a\nCHECK x\n").unwrap();
    assert_eq!(r.status, Status::Conflict);
    assert_eq!(r.retract.len(), 2, "{:?}", r.retract);
}

#[test]
fn assume_retract_is_in_json() {
    let r = vs("FACT x a\nASSUME NOT x a\nCHECK x\n").unwrap();
    let j = r.to_json();
    assert!(j.contains("\"retract\":["), "{j}");
    assert!(j.contains("\"kind\":\"ASSUME\""), "{j}");
    assert!(j.contains("NOT t.x a"), "{j}");
}

// --- near-duplicate atom hints (advisory typo detector) ----------------

#[test]
fn hint_flags_underscore_vs_space_and_is_advisory_only() {
    // The real trap: `is rolled_back` (obj) vs `is_rolled_back` (pred) are
    // DIFFERENT atoms — no contradiction, so the verdict stays CONSISTENT —
    // but the hint warns they were probably meant to be one atom.
    let r = vs(r#"
        FACT auth is rolled_back
        NOT auth is_rolled_back
        CHECK
        "#)
    .unwrap();
    assert_eq!(
        r.status,
        Status::Consistent,
        "hint must not change the verdict"
    );
    assert_eq!(r.exit_code(), 0, "hint must not change the exit code");
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    assert!(r.hints[0].reason.contains('_') || r.hints[0].reason.contains("case"));
}

#[test]
fn hint_flags_case_only_difference() {
    let r = vs("FACT Engine has_fuel\nNOT Engine Has_fuel\nCHECK\n").unwrap();
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
}

#[test]
fn hint_flags_single_char_typo_same_subject() {
    // alphabetic, same subject, edit distance 1, len >= 5 → signal B.
    let r = vs("FACT svc deployed\nNOT svc deployd\nCHECK\n").unwrap();
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
}

#[test]
fn no_hint_for_short_distinct_atoms() {
    // `x a` vs `x b`: distance 1 but intentionally different — must NOT flag.
    let r = vs("FACT x a\nNOT x b\nCHECK\n").unwrap();
    assert!(r.hints.is_empty(), "{:?}", r.hints);
}

#[test]
fn no_hint_for_distinct_words() {
    let r = vs("FACT p is lead\nNOT p is dev\nNOT p is qa\nCHECK\n").unwrap();
    assert!(r.hints.is_empty(), "{:?}", r.hints);
}

#[test]
fn russian_case_typo_is_flagged() {
    // Signal A is script-agnostic: lowercasing works for Cyrillic too.
    let r = vs("FACT кот спит\nNOT Кот спит\nCHECK\n").unwrap();
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
}

#[test]
fn russian_single_char_typo_is_flagged() {
    let r = vs("FACT кот пушистый\nNOT кот пушстый\nCHECK\n").unwrap();
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
}

#[test]
fn cjk_one_char_difference_is_not_flagged() {
    // Caseless script: a one-character change is a different word, not a typo,
    // so the edit-distance signal is skipped (only exact fold-equality fires).
    let r = vs("FACT a 是黑\nNOT a 是白\nCHECK\n").unwrap();
    assert!(r.hints.is_empty(), "{:?}", r.hints);
}

#[test]
fn cjk_underscore_vs_space_is_flagged() {
    // Signal A still applies to any script: `a 猫_黑` (pred) vs `a 猫 黑`
    // (pred+obj) fold to the same name.
    let r = vs("FACT a 猫_黑\nNOT a 猫 黑\nCHECK\n").unwrap();
    assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
}

// --- orphan facts (advisory: assertions no premise/rule references) -----

#[test]
fn orphan_fact_is_flagged_but_advisory_only() {
    // `x a` is asserted but never referenced by a premise or rule: inert.
    // The verdict stays CONSISTENT and the exit code stays 0.
    let r = vs("FACT x a\nCHECK\n").unwrap();
    assert_eq!(
        r.status,
        Status::Consistent,
        "orphan must not change verdict"
    );
    assert_eq!(r.exit_code(), 0, "orphan must not change exit code");
    assert_eq!(r.orphans.len(), 1, "{:?}", r.orphans);
    assert_eq!(r.orphans[0].atom, "t.x a");
    assert_eq!(r.orphans[0].origin.kind, kw::FACT);
}

#[test]
fn fact_used_by_a_premise_is_not_orphan() {
    // `x a` feeds an EXCLUSIVE constraint → referenced → not an orphan.
    let r = vs(r"
        FACT x a
        PREMISE p:
            EXCLUSIVE
                x a
                x b
        CHECK
        ")
    .unwrap();
    assert!(r.orphans.is_empty(), "{:?}", r.orphans);
}

#[test]
fn fact_used_by_a_rule_antecedent_is_not_orphan() {
    let r = vs(r"
        FACT x a
        RULE r:
            WHEN x a
            THEN x c
        CHECK
        ")
    .unwrap();
    assert!(r.orphans.is_empty(), "{:?}", r.orphans);
}

#[test]
fn negation_and_assumption_orphans_keep_their_surface_polarity() {
    // A `NOT` orphan and an `ASSUME NOT` orphan render with the polarity the
    // model wrote, so the report points at the exact line it typed.
    let r = vs("NOT x a\nASSUME NOT y b\nCHECK\n").unwrap();
    assert_eq!(r.orphans.len(), 2, "{:?}", r.orphans);
    let text = format!("{r}");
    assert!(text.contains("ORPHAN    NOT t.x a"), "{text}");
    assert!(text.contains("ORPHAN    ASSUME NOT t.y b"), "{text}");
}

#[test]
fn orphan_is_in_json() {
    let r = vs("FACT x a\nCHECK\n").unwrap();
    let j = r.to_json();
    assert!(j.contains("\"orphans\":["), "{j}");
    assert!(j.contains("\"atom\":\"t.x a\""), "{j}");
    assert!(j.contains("\"kind\":\"FACT\""), "{j}");
}

#[test]
fn unused_import_is_advisory_only_in_the_report() {
    // main imports physics but never references it. The advisory shows up but
    // the verdict stays CONSISTENT (exit 0) — it is purely informational.
    let mut r = MemoryResolver::new();
    r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT x a\nCHECK\n",
    );
    let rep = verify("main.vrf", &r).unwrap();
    assert_eq!(rep.status, Status::Consistent);
    assert_eq!(rep.exit_code(), 0);
    assert_eq!(rep.unused_imports.len(), 1);
    assert_eq!(rep.unused_imports[0].domain, "physics");
    let text = format!("{rep}");
    assert!(text.contains("UNUSED IMPORT  physics"), "{text}");
    assert!(
        rep.to_json().contains("\"unused_imports\":[{"),
        "{}",
        rep.to_json()
    );
}

#[test]
fn a_derived_atom_does_not_make_its_consumer_orphan() {
    // `x c` is derived by the rule and then referenced by the premise; the
    // seeding fact `x a` is referenced by the rule. Nothing is orphan.
    let r = vs(r"
        FACT x a
        RULE r:
            WHEN x a
            THEN x c
        PREMISE p:
            WHEN x c
            THEN x d
        CHECK
        ")
    .unwrap();
    assert!(r.orphans.is_empty(), "{:?}", r.orphans);
}

// --- Defeasible rules: RULE … UNLESS (L3) --------------------------------------

#[test]
fn defeasible_established_exception_suppresses_the_default() {
    // The antecedent holds, but the exception is *established* TRUE → the default is
    // retracted, so the rule derives nothing and `NOT … can_fly` is consistent.
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        FACT pengu is penguin
        NOT pengu can_fly
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty());
    // The default was defeated: nothing derived can_fly.
    assert!(!r.derived.iter().any(|d| d.atom.contains("can_fly")));
    // The defeat is surfaced as an informational note naming the exception.
    assert_eq!(r.defeated.len(), 1);
    assert!(r.defeated[0].consequent.contains("can_fly"));
    assert_eq!(
        r.defeated[0].blocked_by,
        vec![String::from("t.pengu is penguin")]
    );
}

#[test]
fn defeasible_backward_pass_agrees_when_fully_pinned() {
    // Under BIDIRECTIONAL the exception enters the rule's clause (¬bird ∨ penguin ∨
    // can_fly). With every atom pinned by a fact the clause is satisfied and there is
    // exactly one model → CONSISTENT, matching the forward pass (no false conflict).
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        FACT pengu is penguin
        NOT pengu can_fly
        CHECK BIDIRECTIONAL
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(r.conflicts.is_empty());
}

#[test]
fn a_default_that_fires_normally_records_no_defeat() {
    // When no exception is established, the rule fires and there is no DEFEATED note.
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        CHECK
        ")
    .unwrap();
    assert!(r.defeated.is_empty());
}

#[test]
fn defeasible_unknown_exception_lets_the_default_fire() {
    // The exception is never established (stays UNKNOWN) → assume-normal: the default
    // still fires and derives the consequent.
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(
        r.derived
            .iter()
            .any(|d| d.atom.contains("can_fly") && d.value == Value::True)
    );
}

#[test]
fn defeasible_false_exception_lets_the_default_fire() {
    // An exception forced FALSE does not defeat the rule (only an established TRUE
    // does) → the default fires, contradicting `NOT … can_fly`: CONFLICT.
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        NOT pengu is penguin
        NOT pengu can_fly
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
}

#[test]
fn defeasible_default_without_exception_fires_and_can_conflict() {
    // No exception is established, so the default fires and derives can_fly; a
    // `NOT … can_fly` then contradicts the derived value → CONFLICT (non-monotonic:
    // adding `FACT pengu is penguin` would make this consistent).
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        NOT pengu can_fly
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Conflict);
}

#[test]
fn defeasible_multiple_unless_blocks_if_any_fires() {
    // Two exceptions; the rule is suppressed when *any* is established TRUE.
    let r = vs(r"
        RULE fly:
            WHEN robin is bird
            THEN robin can_fly
            UNLESS robin is penguin
            UNLESS robin is injured
        FACT robin is bird
        FACT robin is injured
        NOT robin can_fly
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(!r.derived.iter().any(|d| d.atom.contains("can_fly")));
}

#[test]
fn defeasible_per_element_over_a_set_defeats_only_the_exceptional_one() {
    // `FOR EACH` instantiates the defeasible rule per element: tweety (no exception)
    // flies, pengu (a penguin) is defeated — one rule, two outcomes.
    let r = vs(r"
        SET creatures
            tweety
            pengu
        RULE fly FOR EACH x IN creatures:
            WHEN x is bird
            THEN x can_fly
            UNLESS x is penguin
        FACT tweety is bird
        FACT pengu is bird
        FACT pengu is penguin
        NOT pengu can_fly
        CHECK
        ")
    .unwrap();
    assert_eq!(r.status, Status::Consistent);
    assert!(
        r.derived
            .iter()
            .any(|d| d.atom.contains("tweety") && d.atom.contains("can_fly"))
    );
    assert!(!r.derived.iter().any(|d| d.atom.contains("pengu")));
}

#[test]
fn defeasible_exception_atom_is_not_flagged_as_orphan() {
    // An atom mentioned only in an UNLESS is referenced by the rule, so the orphan
    // lint stays silent.
    let r = vs(r"
        RULE fly:
            WHEN pengu is bird
            THEN pengu can_fly
            UNLESS pengu is penguin
        FACT pengu is bird
        FACT pengu is penguin
        CHECK
        ")
    .unwrap();
    assert!(r.orphans.is_empty(), "{:?}", r.orphans);
}

#[test]
fn premise_with_unless_is_a_compile_error() {
    // UNLESS is RULE-only; a PREMISE carrying one is rejected at compile time.
    let err = vs(r"
        PREMISE p:
            WHEN a is x
            THEN b is y
            UNLESS c is z
        CHECK
        ")
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("UNLESS"), "{msg}");
    assert!(msg.contains("RULE"), "{msg}");
}
