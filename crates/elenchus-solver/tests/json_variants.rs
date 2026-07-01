//! The JSON form of every report variant must be (a) valid JSON and (b) stable.
//! Same inputs as `output_variants.rs`, but asserting `Report::to_json()`:
//! each is parsed back with `serde_json` (validity oracle) and snapshotted.

use elenchus_solver::{MemoryResolver, verify_source, verify_with};

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
        (
            "conflict_exists_witness",
            "NOT auth is ready\nPREMISE covered:\n    EXISTS h WITNESS auth\n        h is ready\n",
        ),
        (
            "warning_exists_unwitnessed",
            "PREMISE someone_ready:\n    EXISTS h\n        h is ready\n",
        ),
        (
            "conflict_fact_because_false",
            "NOT db reachable\nFACT api healthy BECAUSE db reachable\nCHECK api\n",
        ),
        (
            "warning_fact_because_unknown",
            "FACT api healthy BECAUSE db reachable\nCHECK api\n",
        ),
        // Advisory report elements (never change the verdict) — one case each so the
        // populated JSON shape of every array is snapshotted, not just its empty form.
        ("orphans_lint", "FACT lonely atom\nCHECK lonely\n"),
        (
            "hints_similar_atoms",
            "FACT server running\nFACT server runnng\nCHECK server\n",
        ),
        (
            "placeholders_var_default",
            "VAR flag DEFAULT true\nFACT x a\nCHECK x\n",
        ),
        (
            "unsat_core_joint",
            "PREMISE one:\n    ONEOF\n        x a\n        x b\nPREMISE ac:\n    WHEN x a\n    THEN x c\nPREMISE bc:\n    WHEN x b\n    THEN x c\nNOT x c\nCHECK x BIDIRECTIONAL\n",
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
        // Every documented array key must always be present (an empty array when the
        // report has no such element) — the complete set the JSON contract promises.
        for key in [
            "conflicts",
            "warnings",
            "derived",
            "unsat_core",
            "retract",
            "hints",
            "orphans",
            "unused_imports",
            "placeholders",
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

/// `unused_imports` needs a multi-file graph, so it uses the resolver API rather
/// than the single-source `cases()` above. Same oracle: valid JSON + stable snapshot.
#[test]
fn json_unused_import_variant_is_valid_and_stable() {
    let mut r = MemoryResolver::new();
    r.add(
        "root.vrf",
        "DOMAIN root\nIMPORT \"other.vrf\"\nFACT x a\nCHECK x\n",
    )
    .add("other.vrf", "DOMAIN other\nFACT y b\n");
    let report = verify_with("root.vrf", &r, &[]).unwrap();
    let json = report.to_json();
    let value: serde_json::Value =
        serde_json::from_str(&json).unwrap_or_else(|e| panic!("invalid JSON: {e}\n{json}"));
    // The import is never referenced (no `other.<atom>` used), so it is flagged.
    assert!(
        !value["unused_imports"].as_array().unwrap().is_empty(),
        "expected a non-empty unused_imports: {json}"
    );
    insta::assert_snapshot!("unused_import", json);
}
