//! Real multi-file import tests, driven through the on-disk `FileResolver`.
//!
//! Fixture graph (under tests/fixtures/):
//!
//!   car.vrf ──┬─ std/engine.vrf ──┐
//!             └─ std/safety.vrf ──┴─ ../core/base.vrf   (diamond: merged once)
//!
//! Cargo runs an integration test with the working directory set to the package
//! root, so these relative paths resolve and stay portable in snapshots.

use elenchus_compiler::{CompileError, FileResolver, compile};

fn fixture(rel: &str) -> String {
    format!("tests/fixtures/{rel}")
}

#[test]
fn full_car_graph_merges_with_diamond_dedup() {
    let c = compile(&fixture("car.vrf"), &FileResolver).unwrap();
    assert!(c.pending_imports.is_empty(), "all imports must be resolved");
    // core/base.vrf: ONEOF over 3 gears -> C(3,2)=3 pairwise + 1 at-least-one = 4,
    // merged ONCE despite being pulled by both engine.vrf and safety.vrf;
    // + engine_needs_fuel (1) + drive_safely (1) = 6 clauses total.
    assert_eq!(c.clauses.len(), 6);
    assert_eq!(c.rules.len(), 1); // running_implies_warm
    assert_eq!(c.facts.len(), 4);
}

#[test]
fn fact_unifies_with_imported_premise_atom() {
    let c = compile(&fixture("car.vrf"), &FileResolver).unwrap();
    // `Car has fuel` is written as a FACT in car.vrf and constrained by the
    // engine_needs_fuel premise imported from std/engine.vrf — same atom id.
    let has_fuel = c
        .atoms
        .iter()
        .position(|a| {
            a.subject == "Car" && a.predicate == "has" && a.object.as_deref() == Some("fuel")
        })
        .expect("Car has fuel atom") as u32;

    assert!(c.facts.iter().any(|f| f.atom == has_fuel));
    assert!(
        c.clauses.iter().any(
            |cl| cl.origin.premise.as_deref() == Some("engine_needs_fuel")
                && cl.lits.iter().any(|l| l.atom == has_fuel)
        ),
        "the imported premise must reference the same atom as the local fact"
    );
}

#[test]
fn base_provenance_points_at_the_shared_file() {
    let c = compile(&fixture("car.vrf"), &FileResolver).unwrap();
    // The gear clauses must be attributed to the shared core/base.vrf, not to
    // whichever domain happened to import it.
    let gear = c
        .clauses
        .iter()
        .find(|cl| cl.origin.premise.as_deref() == Some("exactly_one_gear"))
        .expect("exactly_one_gear clause");
    assert!(gear.origin.source.ends_with("core/base.vrf"));
}

#[test]
fn merged_graph_ir_snapshot() {
    let c = compile(&fixture("car.vrf"), &FileResolver).unwrap();
    insta::assert_debug_snapshot!(c);
}

#[test]
fn circular_file_imports_detected() {
    let e = compile(&fixture("cycle/x.vrf"), &FileResolver).unwrap_err();
    assert!(matches!(e, CompileError::CircularImport(_)), "got {e:?}");
}

#[test]
fn parse_error_in_imported_file_names_that_file() {
    let e = compile(&fixture("broken/main.vrf"), &FileResolver).unwrap_err();
    match e {
        CompileError::Parse { file, .. } => assert!(file.ends_with("bad.vrf"), "file = {file}"),
        other => panic!("expected a Parse error naming the imported file, got {other:?}"),
    }
}
