//! Cross-domain scoping of the quantifier grounding sources (`SET` names and
//! relation pairs), driven through the public multi-file `compile` path with a
//! `MemoryResolver`.
//!
//! The registries are keyed by `(domain, name)`: a set or a relation belongs to
//! the domain it is declared in, and a `FOR EACH`/`CLOSE`/`EXISTS … IN` — whose
//! grammar only takes a *bare* name — can only ever reference its own file's
//! domain. A qualified 3-part `FACT other.a rel b` is the one way to feed
//! another domain's relation.

use elenchus_compiler::{CompileError, Compiled, MemoryResolver, compile};

/// Compile a two-file graph: `entry.vrf` (the root) importing `tmpl.vrf`.
fn compile_pair(template: &str, entry: &str) -> Result<Compiled, CompileError> {
    let mut r = MemoryResolver::new();
    r.add("tmpl.vrf", template);
    r.add("entry.vrf", entry);
    compile("entry.vrf", &r)
}

#[test]
fn same_predicate_in_two_domains_does_not_leak() {
    // The imported file declares a `linked` pair in ITS domain; the entry file
    // quantifies over ITS OWN `linked`, which has no pairs — the quantifier
    // must ground to nothing rather than borrow the foreign pairs (the
    // pre-domain-keying registry leaked them by bare predicate name).
    let template = "DOMAIN tmpl\nFACT p linked q\n";
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
PREMISE no_mutual FOR EACH x linked y:
    FORBIDS
        x hot on
        y hot on
"#;
    let c = compile_pair(template, entry).unwrap();
    assert_eq!(
        c.clauses.len(),
        0,
        "foreign pairs must not ground the FOR EACH"
    );
    assert!(c.consumed.is_empty(), "no edge may be marked consumed");
}

#[test]
fn a_set_is_not_visible_across_domains() {
    // A SET name is a bare identifier: it cannot be qualified, so it is only
    // visible inside its own domain. Referencing an imported file's set is an
    // UnknownSet error, and the "did you mean" candidates exclude foreign sets.
    let template = "DOMAIN tmpl\nSET machines\n    m1\n    m2\n";
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
PREMISE p FOR EACH m IN machines:
    ONEOF
        m state on
        m state off
"#;
    let CompileError::UnknownSet {
        set, suggestion, ..
    } = compile_pair(template, entry).unwrap_err()
    else {
        panic!("expected UnknownSet");
    };
    assert_eq!(set, "machines");
    assert_eq!(
        suggestion, "",
        "a foreign domain's set must not be suggested"
    );
}

#[test]
fn set_suggestions_stay_within_the_own_domain() {
    // The entry file's own (misspelled) set is still suggested — domain
    // filtering must not silence legitimate same-domain candidates.
    let template = "DOMAIN tmpl\nSET tasks\n    t1\n";
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
SET tasks
    a
PREMISE p FOR EACH t IN taske:
    ONEOF
        t s x
        t s y
"#;
    let CompileError::UnknownSet { suggestion, .. } = compile_pair(template, entry).unwrap_err()
    else {
        panic!("expected UnknownSet");
    };
    assert_eq!(suggestion, " — did you mean `tasks`?");
}
