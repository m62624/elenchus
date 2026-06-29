//! `VAR` port resolution: DEFAULT / external value / UNKNOWN, and the four hard
//! errors (undeclared bare proposition, unknown external key, conflict, ambiguity).

use elenchus_compiler::{
    AtomKey, CompileError, Compiled, MemoryResolver, PlaceholderStatus, PortBinding, Value,
    compile_source, compile_source_with, compile_with, read_data_bindings, read_data_source,
};

/// One `(name, binding)` external input with a test origin.
fn set(name: &str, value: bool) -> (String, PortBinding) {
    (
        name.to_string(),
        PortBinding {
            value,
            origin: "test".to_string(),
        },
    )
}

/// The interned id of the bare-proposition atom `name` (predicate/object `None`).
fn bare_id(c: &Compiled, name: &str) -> Option<u32> {
    c.atoms
        .iter()
        .position(|k: &AtomKey| k.subject == name && k.predicate.is_none() && k.object.is_none())
        .map(|i| i as u32)
}

/// The confident value asserted on the bare proposition `name`, if any.
fn bare_value(c: &Compiled, name: &str) -> Option<Value> {
    let id = bare_id(c, name)?;
    c.facts.iter().find(|f| f.atom == id).map(|f| f.value)
}

#[test]
fn default_is_applied_when_unsupplied() {
    let c = compile_source("d.vrf", "DOMAIN d\nVAR k DEFAULT true\n").unwrap();
    assert_eq!(bare_value(&c, "k"), Some(Value::True));
    assert_eq!(c.placeholders.len(), 1);
    assert_eq!(c.placeholders[0].status, PlaceholderStatus::DefaultUsed);
    assert_eq!(c.placeholders[0].value, Some(true));
}

#[test]
fn external_value_overrides_default() {
    let c = compile_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k DEFAULT false\n",
        &[set("k", true)],
    )
    .unwrap();
    assert_eq!(bare_value(&c, "k"), Some(Value::True));
    assert_eq!(c.placeholders[0].status, PlaceholderStatus::Supplied);
    assert_eq!(c.placeholders[0].origin.as_deref(), Some("test"));
}

#[test]
fn unset_port_stays_unknown() {
    // No value, no DEFAULT: the atom is interned (so it appears) but no fact is
    // pushed — it stays UNKNOWN.
    let c = compile_source("d.vrf", "DOMAIN d\nVAR k\n").unwrap();
    assert!(bare_id(&c, "k").is_some(), "the port atom is interned");
    assert_eq!(bare_value(&c, "k"), None, "no fact → UNKNOWN");
    assert_eq!(c.placeholders[0].status, PlaceholderStatus::Unset);
    assert_eq!(c.placeholders[0].value, None);
}

#[test]
fn bare_proposition_in_a_body_must_be_declared() {
    let err =
        compile_source("d.vrf", "DOMAIN d\nPREMISE p:\n    WHEN k\n    THEN x a\n").unwrap_err();
    assert!(
        matches!(err, CompileError::UndeclaredPort { ref name, .. } if name == "k"),
        "got {err:?}"
    );
}

#[test]
fn bare_proposition_in_a_rule_must_be_declared() {
    // An undeclared bare prop in a RULE body (not just a PREMISE) is also caught.
    let err = compile_source("d.vrf", "DOMAIN d\nRULE r:\n    WHEN k\n    THEN x a\n").unwrap_err();
    assert!(
        matches!(err, CompileError::UndeclaredPort { ref name, .. } if name == "k"),
        "got {err:?}"
    );
}

#[test]
fn a_name_declared_in_two_domains_is_ambiguous_when_set() {
    // Same bare name `k` declared in two imported domains; an external value cannot
    // tell which port it means → AmbiguousPort (lists both domains).
    let mut r = MemoryResolver::new();
    r.add(
        "root.vrf",
        "DOMAIN r\nIMPORT \"a.vrf\"\nIMPORT \"b.vrf\"\nCHECK\n",
    )
    .add("a.vrf", "DOMAIN a\nVAR k\n")
    .add("b.vrf", "DOMAIN b\nVAR k\n");
    let err = compile_with("root.vrf", &r, &[set("k", true)]).unwrap_err();
    assert!(
        matches!(err, CompileError::AmbiguousPort { ref name, ref domains } if name == "k" && domains.contains("a") && domains.contains("b")),
        "got {err:?}"
    );
}

#[test]
fn data_file_rejects_each_non_provide_statement_with_its_line() {
    // read_data_source stops at the first non-PROVIDE statement and names its line.
    // One representative of every statement kind that can appear, so each arm of
    // `statement_line` is exercised.
    for stmt in [
        "FACT x a",
        "NOT x a",
        "ASSUME x a",
        "IMPORT \"o.vrf\"",
        "VAR k",
        "RULE r:\n    WHEN x a\n    THEN x b",
        "PREMISE p:\n    WHEN x a\n    THEN x b",
        "CHECK x",
        "SET s\n    one",
        "CLOSE rel TRANSITIVE",
    ] {
        let src = format!("PROVIDE ok: true\n{stmt}\n");
        let err = read_data_source("vals.vrf", &src).unwrap_err();
        assert!(
            matches!(err, CompileError::DataFileStatement { line, .. } if line == 2),
            "stmt {stmt:?} → {err:?}"
        );
    }
}

#[test]
fn unknown_external_key_is_an_error() {
    let err = compile_source_with("d.vrf", "DOMAIN d\nVAR k\n", &[set("nope", true)]).unwrap_err();
    assert!(
        matches!(err, CompileError::UnknownPort { ref name, .. } if name == "nope"),
        "got {err:?}"
    );
}

#[test]
fn two_disagreeing_bindings_conflict() {
    let err = compile_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k\n",
        &[set("k", true), set("k", false)],
    )
    .unwrap_err();
    assert!(
        matches!(err, CompileError::PortConflict { ref name, .. } if name == "d.k"),
        "got {err:?}"
    );
}

#[test]
fn agreeing_bindings_do_not_conflict() {
    let c = compile_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k\n",
        &[set("k", true), set("k", true)],
    )
    .unwrap();
    assert_eq!(bare_value(&c, "k"), Some(Value::True));
}

#[test]
fn a_used_bare_prop_resolves_against_its_declaration() {
    // The bare prop in the premise body and the VAR declaration intern to the same
    // atom; supplying it drives the premise.
    let src = "DOMAIN d\nVAR k\nPREMISE p:\n    WHEN k\n    THEN x a\n";
    let c = compile_source_with("d.vrf", src, &[set("k", true)]).unwrap();
    assert_eq!(bare_value(&c, "k"), Some(Value::True));
}

#[test]
fn in_file_provide_supplies_a_value() {
    let c = compile_source("d.vrf", "DOMAIN d\nVAR k\nPROVIDE k: true\n").unwrap();
    assert_eq!(bare_value(&c, "k"), Some(Value::True));
    assert_eq!(c.placeholders[0].status, PlaceholderStatus::Supplied);
    assert_eq!(c.placeholders[0].origin.as_deref(), Some("PROVIDE d.vrf"));
}

#[test]
fn provide_conflicts_with_a_disagreeing_external_value() {
    let err = compile_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k\nPROVIDE k: true\n",
        &[set("k", false)],
    )
    .unwrap_err();
    assert!(
        matches!(err, CompileError::PortConflict { ref name, .. } if name == "d.k"),
        "got {err:?}"
    );
}

#[test]
fn read_data_source_extracts_provide_pairs() {
    let pairs = read_data_source(
        "vals.vrf",
        "PROVIDE db_ready: true\nPROVIDE deploy_ok: false\n",
    )
    .unwrap();
    assert_eq!(
        pairs,
        vec![
            ("db_ready".to_string(), true),
            ("deploy_ok".to_string(), false)
        ]
    );
}

#[test]
fn read_data_bindings_tags_each_pair_with_a_data_origin() {
    // The shared bridge every surface (CLI --data, wasm/MCP data map) uses: pairs
    // become PortBindings tagged `data:<file>`, so origins read identically.
    let binds = read_data_bindings("vals.vrf", "PROVIDE k: true\nPROVIDE j: false\n").unwrap();
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].0, "k");
    assert!(binds[0].1.value);
    assert_eq!(binds[0].1.origin, "data:vals.vrf");
    assert_eq!(binds[1].0, "j");
    assert!(!binds[1].1.value);
}

#[test]
fn read_data_bindings_rejects_logic() {
    let err = read_data_bindings("vals.vrf", "PROVIDE k: true\nFACT x a\n").unwrap_err();
    assert!(
        matches!(err, CompileError::DataFileStatement { line, .. } if line == 2),
        "got {err:?}"
    );
}

#[test]
fn read_data_source_rejects_a_non_provide_statement() {
    let err = read_data_source("vals.vrf", "PROVIDE k: true\nFACT x a\n").unwrap_err();
    assert!(
        matches!(err, CompileError::DataFileStatement { ref file, line } if file == "vals.vrf" && line == 2),
        "got {err:?}"
    );
}
