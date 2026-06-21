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
            r#"
        FACT x a
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#,
        ),
        (
            "warning_single",
            r#"
        FACT x a
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#,
        ),
        (
            "warning_multiple_with_derived",
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
        "#,
        ),
        (
            "conflict_exclusive_violation",
            r#"
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#,
        ),
        (
            "conflict_implication_violation",
            r#"
        FACT x a
        NOT x b
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#,
        ),
        (
            "conflict_fact_contradiction",
            "FACT x a\nNOT x a\nCHECK x\n",
        ),
        (
            "conflict_derived_contradiction",
            r#"
        FACT x a
        NOT x b
        RULE r:
            WHEN x a
            THEN x b
        CHECK x
        "#,
        ),
        (
            "conflict_multiple_sorted",
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
        "#,
        ),
        (
            "conflict_system_unsatisfiable",
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
        "#,
        ),
        (
            "underdetermined_with_witness_hint",
            r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#,
        ),
        (
            "conflict_assumptions_retract",
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
        "#,
        ),
        (
            "conflict_assume_vs_fact_retract",
            "FACT x a\nASSUME NOT x a\nCHECK x\n",
        ),
    ]
}

#[test]
fn json_is_valid_and_stable_for_every_variant() {
    for (name, src) in cases() {
        let report = verify_source("v.vrf", &format!("DOMAIN d\n{src}")).unwrap();
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
        for key in [
            "conflicts",
            "warnings",
            "derived",
            "unsat_core",
            "retract",
            "hints",
        ] {
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
