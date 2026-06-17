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
