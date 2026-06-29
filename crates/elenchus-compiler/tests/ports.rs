//! `VAR` port resolution: DEFAULT / external value / UNKNOWN, and the four hard
//! errors (undeclared bare proposition, unknown external key, conflict, ambiguity).

use elenchus_compiler::{
    AtomKey, CompileError, Compiled, PlaceholderStatus, PortBinding, Value, compile_source,
    compile_source_with,
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
        matches!(err, CompileError::PortConflict { ref name, .. } if name == "k"),
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
