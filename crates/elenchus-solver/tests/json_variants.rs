//! The JSON form of every report variant must be (a) valid JSON and (b) stable.
//! Same inputs as `output_variants.rs`, but asserting `Report::to_json()`:
//! each is parsed back with `serde_json` (validity oracle) and snapshotted.

use elenchus_solver::verify_source;

/// (snapshot name, program) for each output variant.
fn cases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("consistent_minimal", "FACT x a\nCHECK x\n"),
        (
            "consistent_with_derived",
            "FACT x a\nRULE r:\n    WHEN x a\n    THEN x b\nCHECK x\n",
        ),
        (
            "warning_single",
            "FACT x a\nAXIOM w:\n    WHEN x a\n    THEN x b\nCHECK x\n",
        ),
        (
            "warning_multiple_with_derived",
            "FACT s ready\nAXIOM need_two:\n    WHEN s ready\n    THEN s checked\n    AND s signed\nRULE mark:\n    WHEN s ready\n    THEN s seen\nCHECK s\n",
        ),
        (
            "conflict_exclusive_violation",
            "FACT x a\nFACT x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n",
        ),
        (
            "conflict_implication_violation",
            "FACT x a\nNOT x b\nAXIOM w:\n    WHEN x a\n    THEN x b\nCHECK x\n",
        ),
        (
            "conflict_fact_contradiction",
            "FACT x a\nNOT x a\nCHECK x\n",
        ),
        (
            "conflict_derived_contradiction",
            "FACT x a\nNOT x b\nRULE r:\n    WHEN x a\n    THEN x b\nCHECK x\n",
        ),
        (
            "conflict_multiple_sorted",
            "FACT y c\nNOT y c\nFACT x a\nFACT x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n",
        ),
        (
            "conflict_system_unsatisfiable",
            "AXIOM a_implies_b:\n    WHEN x a\n    THEN x b\nAXIOM a_implies_not_b:\n    WHEN x a\n    THEN NOT x b\nAXIOM atleast_a_c:\n    ATLEAST\n        x a\n        x c\nAXIOM c_implies_a:\n    WHEN x c\n    THEN x a\nCHECK x BIDIRECTIONAL\n",
        ),
        (
            "underdetermined_with_witness_hint",
            "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n",
        ),
    ]
}

#[test]
fn json_is_valid_and_stable_for_every_variant() {
    for (name, src) in cases() {
        let report = verify_source("v.vrf", src).unwrap();
        let json = report.to_json();

        // (a) it is valid JSON ...
        let value: serde_json::Value = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("invalid JSON for {name}: {e}\n{json}"));
        // ... with the documented shape.
        assert!(
            value.get("status").and_then(|v| v.as_str()).is_some(),
            "{name}: status"
        );
        assert!(
            value.get("exit_code").and_then(|v| v.as_i64()).is_some(),
            "{name}: exit_code"
        );
        for key in ["conflicts", "warnings", "derived"] {
            assert!(
                value.get(key).and_then(|v| v.as_array()).is_some(),
                "{name}: {key} array"
            );
        }
        assert!(
            value.get("underdetermined").is_some(),
            "{name}: underdetermined key"
        );

        // (b) it is stable.
        insta::assert_snapshot!(name, json);
    }
}
