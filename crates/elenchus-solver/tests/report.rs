//! Snapshot tests for the human-readable report (the `Display` output a model
//! or developer actually reads).

use elenchus_solver::verify_source;

#[test]
fn creature_report() {
    let src = include_str!("../../../docs/examples/creature.vrf");
    let r = verify_source("creature.vrf", src).unwrap();
    insta::assert_snapshot!(format!("{r}"));
}

#[test]
fn conflict_report() {
    let src = include_str!("../../../docs/examples/conflict.vrf");
    let r = verify_source("conflict.vrf", src).unwrap();
    insta::assert_snapshot!(format!("{r}"));
}

#[test]
fn socrates_report_shows_the_derivation_trace() {
    // The flagship "why?" case: a 3-step chain leading into the contradiction,
    // rendered as the `why:` trace under the conflict.
    let src = include_str!("../../../docs/examples/socrates.vrf");
    let r = verify_source("socrates.vrf", src).unwrap();
    insta::assert_snapshot!(format!("{r}"));
}

#[test]
fn extension_plan_is_consistent_with_a_derivation_chain() {
    // A well-formed design encoded as a SAT problem: the chosen strategy
    // forward-chains four consequences and the plan checks out CONSISTENT.
    let src = include_str!("../../../docs/examples/extension-plan.vrf");
    let r = verify_source("extension-plan.vrf", src).unwrap();
    insta::assert_snapshot!(format!("{r}"));
}
