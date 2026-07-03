//! Cross-domain scoping of the quantifier grounding sources (`SET` names and
//! relation pairs), driven through the public multi-file `compile` path with a
//! `MemoryResolver`.
//!
//! The registries are keyed by `(domain, name)`: a set or a relation belongs to
//! the domain it is declared in, and a `FOR EACH`/`CLOSE`/`EXISTS … IN` — whose
//! grammar only takes a *bare* name — can only ever reference its own file's
//! domain. A qualified 3-part `FACT other.a rel b` is the one way to feed
//! another domain's relation: declarations are collected for the *whole* import
//! graph before any file grounds (the two-phase compile), so an importing file's
//! qualified facts reach an imported template's `FOR EACH`/`CLOSE`.

use elenchus_compiler::{AtomKey, CompileError, Compiled, MemoryResolver, compile};

/// Compile a two-file graph: `entry.vrf` (the root) importing `tmpl.vrf`.
fn compile_pair(template: &str, entry: &str) -> Result<Compiled, CompileError> {
    let mut r = MemoryResolver::new();
    r.add("tmpl.vrf", template);
    r.add("entry.vrf", entry);
    compile("entry.vrf", &r)
}

/// An atom key in an explicit domain.
fn key_in(domain: &str, subject: &str, predicate: &str, object: &str) -> AtomKey {
    AtomKey {
        domain: domain.to_string(),
        subject: subject.to_string(),
        predicate: Some(predicate.to_string()),
        object: Some(object.to_string()),
    }
}

/// True when the compiled graph marks the atom as consumed by a quantifier.
fn consumed(c: &Compiled, k: &AtomKey) -> bool {
    let Some(id) = c.atoms.iter().position(|a| a == k) else {
        return false;
    };
    c.consumed.contains(&(id as u32))
}

/// The template used by the feeding tests: one seed edge and a premise
/// quantified over the `linked` relation (one FORBIDS clause per pair).
const LINKED_TEMPLATE: &str = r"DOMAIN tmpl
FACT a linked b
PREMISE no_mutual FOR EACH x linked y:
    FORBIDS
        x hot on
        y hot on
";

#[test]
fn qualified_fact_feeds_an_imported_relation_for_each() {
    // The entry file appends an edge to the TEMPLATE's relation by qualifying
    // the fact into the template's domain. The template's FOR EACH grounds over
    // both pairs, and the entry-fed edge is consumed (not an idle fact).
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
FACT tmpl.b linked c
"#;
    let c = compile_pair(LINKED_TEMPLATE, entry).unwrap();
    assert_eq!(c.clauses.len(), 2, "one grounded clause per pair");
    assert!(consumed(&c, &key_in("tmpl", "a", "linked", "b")));
    assert!(
        consumed(&c, &key_in("tmpl", "b", "linked", "c")),
        "the entry-fed edge must be consumed by the template's quantifier"
    );
    assert!(
        c.unused_imports.is_empty(),
        "a qualified fact is a real reference to the imported domain"
    );
}

#[test]
fn close_transitive_runs_over_template_and_entry_pairs() {
    // tmpl: a->b, CLOSE TRANSITIVE; entry adds b->c. The closure must run over
    // the union — a->b, b->c and the derived a->c — so the FOR EACH grounds
    // three clauses.
    let template = r"DOMAIN tmpl
FACT a dep b
CLOSE dep TRANSITIVE
PREMISE p FOR EACH x dep y:
    FORBIDS
        x hot on
        y hot on
";
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
FACT tmpl.b dep c
"#;
    let c = compile_pair(template, entry).unwrap();
    assert_eq!(c.clauses.len(), 3, "a->b, b->c and the derived a->c");
    assert!(consumed(&c, &key_in("tmpl", "b", "dep", "c")));
}

#[test]
fn an_entry_supplied_cycle_fails_the_template_close() {
    // The entry file's edge closes a cycle through the template's DAG-only
    // CLOSE TRANSITIVE — the existing CyclicRelation error fires, naming the
    // relation.
    let template = r"DOMAIN tmpl
FACT a dep b
CLOSE dep TRANSITIVE
";
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
FACT tmpl.b dep a
"#;
    let CompileError::CyclicRelation { relation, .. } = compile_pair(template, entry).unwrap_err()
    else {
        panic!("expected CyclicRelation");
    };
    assert_eq!(relation, "dep");
}

#[test]
fn a_bare_entry_fact_stays_in_the_entry_domain() {
    // Without the domain prefix the fact lands in the ENTRY domain — sharing is
    // explicit, so it must not feed the template's relation.
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf"
FACT b linked c
"#;
    let c = compile_pair(LINKED_TEMPLATE, entry).unwrap();
    assert_eq!(c.clauses.len(), 1, "only the template's own pair grounds");
    assert!(!consumed(&c, &key_in("e", "b", "linked", "c")));
}

#[test]
fn an_import_alias_feeds_the_same_relation() {
    // `IMPORT … AS t` + `FACT t.b linked c`: the alias resolves to the
    // canonical domain, so the pair feeds the same relation.
    let entry = r#"DOMAIN e
IMPORT "tmpl.vrf" AS t
FACT t.b linked c
"#;
    let c = compile_pair(LINKED_TEMPLATE, entry).unwrap();
    assert_eq!(c.clauses.len(), 2);
    assert!(consumed(&c, &key_in("tmpl", "b", "linked", "c")));
}

#[test]
fn diamond_feeders_merge_and_deduplicate() {
    // Two files import the same template and both feed it; one edge is fed
    // twice. The template is compiled once, the duplicate pair grounds a
    // duplicate clause that the signature dedup absorbs.
    let mid1 = r#"DOMAIN m1
IMPORT "tmpl.vrf"
FACT tmpl.b linked c
"#;
    let mid2 = r#"DOMAIN m2
IMPORT "tmpl.vrf"
FACT tmpl.b linked c
FACT tmpl.c linked d
"#;
    let entry = r#"DOMAIN e
IMPORT "m1.vrf"
IMPORT "m2.vrf"
"#;
    let mut r = MemoryResolver::new();
    r.add("tmpl.vrf", LINKED_TEMPLATE);
    r.add("m1.vrf", mid1);
    r.add("m2.vrf", mid2);
    r.add("entry.vrf", entry);
    let c = compile("entry.vrf", &r).unwrap();
    // Pairs: a->b (template), b->c (fed twice, deduped), c->d — 3 clauses.
    assert_eq!(c.clauses.len(), 3);
    assert!(consumed(&c, &key_in("tmpl", "c", "linked", "d")));
}

#[test]
fn same_domain_files_pool_their_pairs() {
    // Two files that declare the SAME domain share it nominally (per SPEC), so
    // their bare pairs pool into one relation.
    let library = r"DOMAIN shared
FACT a linked b
PREMISE p FOR EACH x linked y:
    FORBIDS
        x hot on
        y hot on
";
    let entry = r#"DOMAIN shared
IMPORT "tmpl.vrf"
FACT b linked c
"#;
    let c = compile_pair(library, entry).unwrap();
    assert_eq!(
        c.clauses.len(),
        2,
        "both files' pairs feed the shared domain"
    );
    assert!(consumed(&c, &key_in("shared", "b", "linked", "c")));
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
