//! Snapshot tests for the compiled IR on the hardest variants: large EXCLUSIVE
//! expansions, ONEOF/ATLEAST, multi-condition multi-consequent implications with
//! negation, rules, and cross-file IMPORT merge with provenance.

use elenchus_compiler::{MemoryResolver, compile, compile_source};

/// A deliberately dense single source exercising every desugaring path.
const COMPLEX: &str = "\
FACT Sys ready
NOT  Sys broken

AXIOM modes:
    EXCLUSIVE
        Sys mode idle
        Sys mode run
        Sys mode halt
        Sys mode error

AXIOM exactly_one_owner:
    ONEOF
        Sys owner alice
        Sys owner bob

AXIOM at_least_one_check:
    ATLEAST
        Sys checked unit
        Sys checked integration

AXIOM deploy_gate:
    WHEN Sys ready
    AND  Sys checked unit
    AND  NOT Sys broken
    THEN Sys can deploy
    AND  Sys notify ops

RULE derive_busy:
    WHEN Sys mode run
    THEN Sys busy

CHECK Sys BIDIRECTIONAL
";

#[test]
fn complex_program_ir() {
    // EXCLUSIVE(4) -> C(4,2)=6 ; ONEOF(2) -> 1 pairwise + 1 at-least-one ;
    // ATLEAST(2) -> 1 ; deploy_gate (3 antecedent incl NOT, 2 consequents) -> 2.
    let c = compile_source("complex.vrf", COMPLEX).unwrap();
    assert_eq!(c.clauses.len(), 6 + 2 + 1 + 2);
    insta::assert_debug_snapshot!(c);
}

#[test]
fn imported_ir_keeps_provenance_and_unifies_atoms() {
    let mut r = MemoryResolver::new();
    r.add(
        "physics.vrf",
        include_str!("../../../docs/examples/physics.vrf"),
    );
    r.add(
        "import-demo.vrf",
        include_str!("../../../docs/examples/import-demo.vrf"),
    );
    let c = compile("import-demo.vrf", &r).unwrap();
    insta::assert_debug_snapshot!(c);
}
