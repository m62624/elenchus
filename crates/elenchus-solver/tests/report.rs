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
fn orphan_fact_appears_as_an_advisory_line_in_a_consistent_report() {
    // `spare wheel mounted` is referenced by no premise or rule, so the engine
    // appends an advisory ORPHAN line. `engine has_fuel`/`engine runs` are used by
    // the premise, so they are not flagged. The verdict stays CONSISTENT — the
    // ORPHAN is purely informational.
    let src = "\
DOMAIN garage
FACT engine has_fuel
FACT spare wheel mounted
PREMISE runs_on_fuel:
    WHEN engine has_fuel
    THEN engine runs
FACT engine runs
CHECK
";
    let r = verify_source("garage.vrf", src).unwrap();
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
