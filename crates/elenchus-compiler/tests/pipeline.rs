//! Pipeline (black-box) tests for the compiler facade: every case here drives
//! the public parse → compile path, so it lives as an integration test that can
//! only see the crate's public API.
use elenchus_compiler::*;

/// Compile a single inline source under a default `DOMAIN t`, so test
/// programs need not repeat the declaration. Atoms land in domain `t`.
fn cs(src: &str) -> Result<Compiled, CompileError> {
    compile_source("<t>", &format!("DOMAIN t\n{src}"))
}

/// An atom key in the default test domain `t`.
fn key(subject: &str, predicate: &str, object: Option<&str>) -> AtomKey {
    key_in("t", subject, predicate, object)
}

/// An atom key in an explicit domain.
fn key_in(domain: &str, subject: &str, predicate: &str, object: Option<&str>) -> AtomKey {
    AtomKey {
        domain: domain.to_string(),
        subject: subject.to_string(),
        predicate: Some(predicate.to_string()),
        object: object.map(|o| o.to_string()),
    }
}

fn id(c: &Compiled, k: &AtomKey) -> AtomId {
    c.atoms.iter().position(|a| a == k).unwrap() as AtomId
}

#[test]
fn exclusive_unfolds_pairwise() {
    let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
                x c
        "#;
    let c = cs(src).unwrap();
    // C(3,2) = 3 clauses, each of 2 positive literals.
    assert_eq!(c.clauses.len(), 3);
    for cl in &c.clauses {
        assert_eq!(cl.lits.len(), 2);
        assert!(cl.lits.iter().all(|l| !l.negated));
    }
}

#[test]
fn implication_negates_consequent() {
    // WHEN x a THEN x b  ==  Impossible([x a, NOT x b])
    let src = r#"
        PREMISE r:
            WHEN x a
            THEN x b
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
    let cl = &c.clauses[0];
    assert_eq!(cl.lits.len(), 2);
    let a = id(&c, &key("x", "a", None));
    let b = id(&c, &key("x", "b", None));
    assert!(cl.lits.contains(&Lit {
        atom: a,
        negated: false
    }));
    assert!(cl.lits.contains(&Lit {
        atom: b,
        negated: true
    }));
}

#[test]
fn negated_consequent_flips_to_positive() {
    // THEN NOT x b  →  NOT(NOT x b) = x b positive inside Impossible
    let src = r#"
        PREMISE r:
            WHEN x a
            THEN NOT x b
        "#;
    let c = cs(src).unwrap();
    let b = id(&c, &key("x", "b", None));
    assert!(c.clauses[0].lits.contains(&Lit {
        atom: b,
        negated: false
    }));
}

#[test]
fn consequent_or_is_one_clause_with_all_negated() {
    // WHEN x p THEN x a OR x b  ==  Impossible([x p, NOT x a, NOT x b])
    let src = r#"
        PREMISE r:
            WHEN x p
            THEN x a
            OR x b
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
    let cl = &c.clauses[0];
    assert_eq!(cl.lits.len(), 3);
    let p = id(&c, &key("x", "p", None));
    let a = id(&c, &key("x", "a", None));
    let b = id(&c, &key("x", "b", None));
    assert!(cl.lits.contains(&Lit {
        atom: p,
        negated: false
    }));
    assert!(cl.lits.contains(&Lit {
        atom: a,
        negated: true
    }));
    assert!(cl.lits.contains(&Lit {
        atom: b,
        negated: true
    }));
}

#[test]
fn antecedent_or_is_one_clause_per_disjunct() {
    // WHEN x a OR x b THEN x c
    //   == Impossible([x a, NOT x c]) ∧ Impossible([x b, NOT x c])
    let src = r#"
        PREMISE r:
            WHEN x a
            OR x b
            THEN x c
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 2);
    let a = id(&c, &key("x", "a", None));
    let b = id(&c, &key("x", "b", None));
    let cc = id(&c, &key("x", "c", None));
    // every clause has exactly two lits and carries NOT c
    for cl in &c.clauses {
        assert_eq!(cl.lits.len(), 2);
        assert!(cl.lits.contains(&Lit {
            atom: cc,
            negated: true
        }));
    }
    let has = |atom| {
        c.clauses.iter().any(|cl| {
            cl.lits.contains(&Lit {
                atom,
                negated: false,
            })
        })
    };
    assert!(has(a) && has(b));
}

#[test]
fn antecedent_or_with_consequent_or_distributes() {
    // (a ∨ b) → (c ∨ d): Impossible([a,¬c,¬d]) ∧ Impossible([b,¬c,¬d])
    let src = r#"
        PREMISE r:
            WHEN x a
            OR x b
            THEN x c
            OR x d
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 2);
    for cl in &c.clauses {
        assert_eq!(cl.lits.len(), 3);
    }
}

#[test]
fn rule_with_or_antecedent_splits_into_two_rules() {
    // (a ∨ b) → c derives c whenever either fires: two single-antecedent rules.
    let src = r#"
        RULE r:
            WHEN x a
            OR x b
            THEN x c
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.rules.len(), 2);
    assert!(
        c.rules
            .iter()
            .all(|r| r.antecedent.len() == 1 && r.consequent.len() == 1)
    );
}

#[test]
fn rule_with_or_consequent_is_rejected() {
    // A rule cannot derive a disjunction — must be a PREMISE.
    let src = r#"
        RULE r:
            WHEN x a
            THEN x b
            OR x c
        "#;
    let err = cs(src).unwrap_err();
    assert!(matches!(
        err,
        CompileError::RuleDisjunctiveConsequent { .. }
    ));
}

#[test]
fn oneof_is_pairwise_plus_at_least_one() {
    let src = r#"
        PREMISE o:
            ONEOF
                x a
                x b
        "#;
    let c = cs(src).unwrap();
    // pairwise C(2,2)=1 + 1 at-least-one = 2 clauses
    assert_eq!(c.clauses.len(), 2);
    // the at-least-one clause is the all-negated one
    assert!(c.clauses.iter().any(|cl| cl.lits.iter().all(|l| l.negated)));
}

#[test]
fn atleast_is_one_negated_clause() {
    let src = r#"
        PREMISE a:
            ATLEAST
                x a
                x b
                x c
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
    assert_eq!(c.clauses[0].lits.len(), 3);
    assert!(c.clauses[0].lits.iter().all(|l| l.negated));
}

#[test]
fn rules_are_separate_from_clauses() {
    let src = r#"
        RULE needs:
            WHEN x a
            THEN x b
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 0);
    assert_eq!(c.rules.len(), 1);
    assert_eq!(c.rules[0].antecedent.len(), 1);
    assert_eq!(c.rules[0].consequent.len(), 1);
}

#[test]
fn atoms_are_canonically_sorted() {
    let src = r#"
        FACT z z
        FACT a a
        FACT m m
        "#;
    let c = cs(src).unwrap();
    let mut sorted = c.atoms.clone();
    sorted.sort();
    assert_eq!(c.atoms, sorted);
}

#[test]
fn duplicate_premise_is_idempotent() {
    let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
}

#[test]
fn redefinition_with_different_body_errors() {
    let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x c
        "#;
    let err = cs(src).unwrap_err();
    assert_eq!(
        err,
        CompileError::PremiseRedefinition {
            name: "e".to_string()
        }
    );
}

#[test]
fn duplicate_fact_is_idempotent() {
    let c = cs("FACT x a\nFACT x a\n").unwrap();
    assert_eq!(c.facts.len(), 1);
}

#[test]
fn conflicting_facts_are_both_kept() {
    // FACT X + NOT X is a CONFLICT for the solver, not a compile error.
    let c = cs("FACT x a\nNOT x a\n").unwrap();
    assert_eq!(c.facts.len(), 2);
}

#[test]
fn import_is_recorded_pending() {
    let c = cs("IMPORT \"physics.vrf\"\nFACT x a\n").unwrap();
    assert_eq!(c.pending_imports, vec!["physics.vrf".to_string()]);
}

#[test]
fn qualified_fact_lands_in_the_imported_domain() {
    // The library's premise is about `physics.Engine_X has fuel`; the main file
    // asserts a fact qualified INTO that domain, so the two share one atom id.
    let mut r = MemoryResolver::new();
    r.add(
        "lib.vrf",
        r#"
        DOMAIN physics
        PREMISE needs_fuel:
            WHEN Engine_X has engine
            THEN Engine_X has fuel
        "#,
    );
    r.add(
        "main.vrf",
        r#"
        DOMAIN main
        IMPORT "lib.vrf"
        FACT physics.Engine_X has engine
        FACT physics.Engine_X has fuel
        "#,
    );
    let c = compile("main.vrf", &r).unwrap();
    assert!(c.pending_imports.is_empty());
    assert_eq!(c.clauses.len(), 1); // the imported premise
    assert_eq!(c.facts.len(), 2);

    // `physics.Engine_X has fuel` from the FACT and the imported premise share an id.
    let fuel = key_in("physics", "Engine_X", "has", Some("fuel"));
    let fuel_id = id(&c, &fuel);
    assert!(c.facts.iter().any(|f| f.atom == fuel_id));
    assert!(c.clauses[0].lits.iter().any(|l| l.atom == fuel_id));
}

#[test]
fn same_triple_in_different_domains_does_not_unify() {
    // Without a domain prefix the fact lands in `main`, NOT `physics`, so it is
    // a distinct atom from the library's `physics.Engine_X has fuel`.
    let mut r = MemoryResolver::new();
    r.add("lib.vrf", "DOMAIN physics\nFACT Engine_X has fuel\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"lib.vrf\"\nFACT Engine_X has fuel\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    // Two distinct atoms: physics.Engine_X has fuel and main.Engine_X has fuel.
    assert!(c.atoms.iter().any(|a| a.domain == "physics"));
    assert!(c.atoms.iter().any(|a| a.domain == "main"));
    assert_eq!(
        c.atoms
            .iter()
            .filter(|a| a.subject == "Engine_X" && a.predicate.as_deref() == Some("has"))
            .count(),
        2
    );
}

#[test]
fn import_alias_binds_a_local_domain_name() {
    // `AS phys` lets the consumer reference the imported domain by a local name.
    let mut r = MemoryResolver::new();
    r.add("lib.vrf", "DOMAIN physics\nFACT Motor over_200\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"lib.vrf\" AS phys\nFACT phys.Motor over_100\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    // Both facts live in the physics domain (one via its own name, one via alias).
    assert_eq!(c.atoms.iter().filter(|a| a.domain == "physics").count(), 2);
}

#[test]
fn unknown_domain_reference_errors() {
    // Referencing a domain that is neither this file's nor imported here fails.
    let err = cs("FACT ghost.x a\n").unwrap_err();
    assert!(matches!(err, CompileError::UnknownDomain { .. }));
}

#[test]
fn imports_are_not_transitive_for_naming() {
    // main imports physics; physics imports math. main may NOT name math.
    let mut r = MemoryResolver::new();
    r.add("math.vrf", "DOMAIN math\nFACT foo bar\n");
    r.add(
        "physics.vrf",
        "DOMAIN physics\nIMPORT \"math.vrf\"\nFACT Motor over_100\n",
    );
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT math.foo bar\n",
    );
    let err = compile("main.vrf", &r).unwrap_err();
    assert!(matches!(err, CompileError::UnknownDomain { .. }));
}

#[test]
fn transitive_dependency_clauses_still_load() {
    // Even though main can't *name* math, math's clauses still participate.
    let mut r = MemoryResolver::new();
    r.add(
        "math.vrf",
        r"
        DOMAIN math
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        ",
    );
    r.add("physics.vrf", "DOMAIN physics\nIMPORT \"math.vrf\"\n");
    r.add("main.vrf", "DOMAIN main\nIMPORT \"physics.vrf\"\n");
    let c = compile("main.vrf", &r).unwrap();
    assert_eq!(c.clauses.len(), 1); // math's clause loaded transitively
    assert!(c.clauses.iter().all(|cl| cl.origin.source == "math.vrf"));
}

#[test]
fn missing_domain_errors() {
    let err = compile_source("nodomain.vrf", "FACT x a\n").unwrap_err();
    assert!(matches!(err, CompileError::MissingDomain { .. }));
}

#[test]
fn duplicate_domain_errors() {
    let err = compile_source("dup.vrf", "DOMAIN a\nDOMAIN b\nFACT x a\n").unwrap_err();
    assert!(matches!(err, CompileError::DuplicateDomain { .. }));
}

#[test]
fn alias_clash_when_one_local_name_binds_two_domains() {
    // The same local alias `x` bound to two genuinely different domains is a
    // clash: disambiguate with distinct aliases.
    let mut r = MemoryResolver::new();
    r.add("a.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add("b.vrf", "DOMAIN chemistry\nFACT atom reacts\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"a.vrf\" AS x\nIMPORT \"b.vrf\" AS x\n",
    );
    let err = compile("main.vrf", &r).unwrap_err();
    assert!(matches!(err, CompileError::DomainAliasClash { .. }));
}

#[test]
fn two_files_with_the_same_domain_name_merge() {
    // Nominal domains: two files both declaring DOMAIN physics share it (the
    // value of importing a premise library is exactly this unification).
    let mut r = MemoryResolver::new();
    r.add("a.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        "DOMAIN physics\nIMPORT \"a.vrf\"\nFACT Motor over_200\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    // Both motors live in the single shared `physics` domain.
    assert!(c.atoms.iter().all(|a| a.domain == "physics"));
    assert_eq!(c.atoms.len(), 2);
}

#[test]
fn diamond_import_is_deduped() {
    // main → a, c ; a → base ; c → base. base merged once.
    let mut r = MemoryResolver::new();
    r.add(
        "base.vrf",
        r#"
        DOMAIN base
        PREMISE b:
            EXCLUSIVE
                x a
                x b
        "#,
    );
    r.add("a.vrf", "DOMAIN a\nIMPORT \"base.vrf\"\n");
    r.add("c.vrf", "DOMAIN c\nIMPORT \"base.vrf\"\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"a.vrf\"\nIMPORT \"c.vrf\"\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    assert_eq!(c.clauses.len(), 1); // base's single clause, not two
}

#[test]
fn circular_import_errors() {
    let mut r = MemoryResolver::new();
    r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\n");
    r.add("b.vrf", "DOMAIN b\nIMPORT \"a.vrf\"\n");
    let err = compile("a.vrf", &r).unwrap_err();
    assert!(matches!(err, CompileError::CircularImport(_)));
}

#[test]
fn three_node_cycle_errors() {
    // a → b → c → a. The back-edge to the on-path ancestor is detected.
    let mut r = MemoryResolver::new();
    r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\n");
    r.add("b.vrf", "DOMAIN b\nIMPORT \"c.vrf\"\n");
    r.add("c.vrf", "DOMAIN c\nIMPORT \"a.vrf\"\n");
    let err = compile("a.vrf", &r).unwrap_err();
    assert!(matches!(err, CompileError::CircularImport(_)));
}

#[test]
fn shared_grandchild_diamond_loads_once() {
    // The user's case: a imports B and C; C ALSO imports B. B must be compiled
    // exactly once (its single clause is not duplicated by the two paths to it).
    let mut r = MemoryResolver::new();
    r.add(
        "b.vrf",
        r"
        DOMAIN b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        ",
    );
    r.add("c.vrf", "DOMAIN c\nIMPORT \"b.vrf\"\n");
    r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\nIMPORT \"c.vrf\"\n");
    let c = compile("a.vrf", &r).unwrap();
    assert_eq!(
        c.clauses.len(),
        1,
        "b.vrf's clause must appear exactly once"
    );
}

#[test]
fn exponential_fan_out_is_memoized_not_blown_up() {
    // f_k imports f_{k-1} TWICE. Without content-hash memoization the visit
    // count is 2^k (2^40 ≈ a trillion); with it, the work is linear, so this
    // finishes instantly. A guard against any combinatorial blow-up / DoS.
    let mut r = MemoryResolver::new();
    r.add("f0.vrf", "DOMAIN d0\nFACT x a\n");
    let n = 40;
    for k in 1..=n {
        r.add(
            &format!("f{k}.vrf"),
            &format!(
                "DOMAIN d{k}\nIMPORT \"f{}.vrf\"\nIMPORT \"f{}.vrf\"\n",
                k - 1,
                k - 1
            ),
        );
    }
    let c = compile(&format!("f{n}.vrf"), &r).unwrap();
    assert_eq!(c.facts.len(), 1); // the single fact from f0, reached once
}

#[test]
fn very_deep_linear_chain_does_not_overflow() {
    // A long non-cyclic chain. Resolution is iterative (explicit work stack),
    // so a depth that would overflow a recursive loader compiles cleanly.
    let mut r = MemoryResolver::new();
    r.add("f0.vrf", "DOMAIN d0\nFACT x a\n");
    let n = 10_000;
    for k in 1..=n {
        r.add(
            &format!("f{k}.vrf"),
            &format!("DOMAIN d{k}\nIMPORT \"f{}.vrf\"\n", k - 1),
        );
    }
    let c = compile(&format!("f{n}.vrf"), &r).unwrap();
    assert_eq!(c.facts.len(), 1);
}

#[test]
fn missing_import_errors() {
    let mut r = MemoryResolver::new();
    r.add("main.vrf", "DOMAIN main\nIMPORT \"ghost.vrf\"\n");
    let err = compile("main.vrf", &r).unwrap_err();
    assert!(matches!(err, CompileError::ImportNotFound(_)));
}

#[test]
fn unused_import_is_flagged() {
    // main imports physics but never writes a `physics.` atom → unused.
    let mut r = MemoryResolver::new();
    r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT x a\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    assert_eq!(c.unused_imports.len(), 1);
    assert_eq!(c.unused_imports[0].domain, "physics");
    assert_eq!(c.unused_imports[0].file, "main.vrf");
    assert_eq!(c.unused_imports[0].alias, None);
}

#[test]
fn referenced_import_is_not_unused() {
    // The same import, but now a `physics.` atom uses it → not flagged.
    let mut r = MemoryResolver::new();
    r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT physics.Motor over_200\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    assert!(c.unused_imports.is_empty(), "{:?}", c.unused_imports);
}

#[test]
fn unused_import_records_its_alias() {
    let mut r = MemoryResolver::new();
    r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\" AS phys\nFACT x a\n",
    );
    let c = compile("main.vrf", &r).unwrap();
    assert_eq!(c.unused_imports.len(), 1);
    assert_eq!(c.unused_imports[0].alias.as_deref(), Some("phys"));
}

#[test]
fn import_referenced_only_inside_a_premise_is_used() {
    // The reference can be anywhere — here inside a premise body, not a fact.
    let mut r = MemoryResolver::new();
    r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
    r.add(
        "main.vrf",
        r#"
        DOMAIN main
        IMPORT "physics.vrf"
        PREMISE p:
            WHEN physics.Motor over_100
            THEN x ok
        "#,
    );
    let c = compile("main.vrf", &r).unwrap();
    assert!(c.unused_imports.is_empty(), "{:?}", c.unused_imports);
}

#[test]
fn same_premise_name_across_files_coexists() {
    // Two files may legitimately reuse a premise NAME with different bodies.
    // Names are per-source labels — both premises apply, qualified by source.
    // NOT a redefinition error. (Atoms stay apart too: different domains.)
    let mut r = MemoryResolver::new();
    r.add(
        "physics.vrf",
        r#"
        DOMAIN physics
        PREMISE safety:
            EXCLUSIVE
                x a
                x b
        "#,
    );
    r.add(
        "main.vrf",
        r#"
        DOMAIN main
        IMPORT "physics.vrf"
        PREMISE safety:
            EXCLUSIVE
                x a
                x c
        "#,
    );
    let c = compile("main.vrf", &r).unwrap();
    assert_eq!(c.clauses.len(), 2); // a-b from physics, a-c from main
    assert!(c.clauses.iter().any(|cl| cl.origin.source == "physics.vrf"));
    assert!(c.clauses.iter().any(|cl| cl.origin.source == "main.vrf"));
}

#[test]
fn redefinition_within_one_source_still_errors() {
    // But reusing a name with a different body *inside one source* is a mistake.
    let src = r#"
        DOMAIN m
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x c
        "#;
    let err = compile_source("main.vrf", src).unwrap_err();
    assert_eq!(
        err,
        CompileError::PremiseRedefinition {
            name: "e".to_string()
        }
    );
}

#[test]
fn import_demo_examples_resolve() {
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
    assert!(c.pending_imports.is_empty());
    // physics.vrf: one_path (EXCLUSIVE, 1 clause) + speed_order (impl, 1 clause)
    assert_eq!(c.clauses.len(), 2);
    // The qualified facts (`physics.Motor …`) share ids with the imported premise.
    let over_100 = id(&c, &key_in("physics", "Motor", "over_100", None));
    assert!(c.facts.iter().any(|f| f.atom == over_100));
    assert!(
        c.clauses
            .iter()
            .any(|cl| cl.lits.iter().any(|l| l.atom == over_100))
    );
}

#[test]
fn creature_example_compiles() {
    let src = include_str!("../../../docs/examples/creature.vrf");
    let c = compile_source("creature.vrf", src).unwrap();
    assert_eq!(c.facts.len(), 2); // flying, warm_blood
    assert_eq!(c.rules.len(), 1); // needs_oxygen
    assert_eq!(c.checks.len(), 1);
    // fly_xor_swim (1) + wings_need_bone (THEN wing AND bone → 2) + no_dual_temp (1) = 4
    assert_eq!(c.clauses.len(), 4);
    assert_eq!(c.atoms.len(), 7);
}

#[test]
fn forbids_unfolds_pairwise() {
    let src = r#"
        PREMISE f:
            FORBIDS
                x a
                x b
                x c
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 3); // C(3,2), like EXCLUSIVE
    assert!(
        c.clauses
            .iter()
            .all(|cl| cl.lits.len() == 2 && cl.lits.iter().all(|l| !l.negated))
    );
}

#[test]
fn rule_with_multiple_consequents() {
    let src = r#"
        RULE r:
            WHEN x a
            THEN x b
            AND  x c
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.rules.len(), 1);
    assert_eq!(c.rules[0].consequent.len(), 2);
}

#[test]
fn negated_antecedent_literal_keeps_polarity() {
    // WHEN NOT x a THEN x b  ==  Impossible([NOT x a, NOT x b])
    let src = r#"
        PREMISE a:
            WHEN NOT x a
            THEN x b
        "#;
    let c = cs(src).unwrap();
    let xa = id(&c, &key("x", "a", None));
    assert!(c.clauses[0].lits.contains(&Lit {
        atom: xa,
        negated: true
    }));
}

#[test]
fn rule_keeps_consequent_negation() {
    let src = r#"
        RULE r:
            WHEN x a
            THEN NOT x b
        "#;
    let c = cs(src).unwrap();
    assert!(c.rules[0].consequent[0].negated);
}

#[test]
fn compilation_is_deterministic() {
    let src = r#"
        PREMISE e:
            EXCLUSIVE
                z z
                a a
                m m
        FACT q q
        "#;
    assert_eq!(cs(src).unwrap(), cs(src).unwrap());
}

#[test]
fn empty_program_compiles_to_empty_ir() {
    let c = cs("// nothing here\n").unwrap();
    assert!(c.atoms.is_empty() && c.clauses.is_empty() && c.facts.is_empty());
}

#[test]
fn same_clause_from_two_named_premises_is_deduped() {
    // Different names, identical logical content → one clause, no redefinition.
    let src = r#"
        PREMISE e1:
            EXCLUSIVE
                x a
                x b
        PREMISE e2:
            EXCLUSIVE
                x a
                x b
        "#;
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
}

#[test]
fn object_distinguishes_atom_identity() {
    // `x p a` and `x p b` differ only by object → two distinct atoms.
    let c = cs("FACT x p a\nFACT x p b\n").unwrap();
    assert_eq!(c.atoms.len(), 2);
}

// --- closed-world: ONEOF closes its variable's value set -----------------

/// A `ONEOF` body declaring three values of `resolved is …`. Flush-left so it
/// concatenates cleanly in front of an appended line (CAPSTONE-style const).
const ONEOF_RESOLVED: &str = r"PREMISE pick:
    ONEOF
        resolved is censored
        resolved is censored_mtp
        resolved is uncensored
";

#[test]
fn value_outside_oneof_is_rejected() {
    let src = format!("{ONEOF_RESOLVED}FACT resolved is censoredmtp\n");
    let err = cs(&src).unwrap_err();
    let CompileError::UnknownValue(e) = err else {
        panic!("expected UnknownValue, got {err:?}");
    };
    assert_eq!(e.value, "censoredmtp");
    assert_eq!(e.subject, "resolved");
    assert_eq!(e.predicate, "is");
    assert_eq!(e.declared, "censored, censored_mtp, uncensored");
}

#[test]
fn near_miss_value_suggests_the_intended_one() {
    let src = format!("{ONEOF_RESOLVED}FACT resolved is censoredmtp\n");
    let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
        panic!("expected UnknownValue");
    };
    assert_eq!(e.suggestion, " — did you mean `censored_mtp`?");
}

#[test]
fn far_off_value_offers_no_suggestion() {
    // `wildly_different` is past the edit-distance budget of every declared
    // value, so we reject it but do not guess.
    let src = format!("{ONEOF_RESOLVED}FACT resolved is wildly_different\n");
    let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
        panic!("expected UnknownValue");
    };
    assert_eq!(e.suggestion, "");
}

#[test]
fn declared_value_compiles_cleanly() {
    let src = format!("{ONEOF_RESOLVED}FACT resolved is censored_mtp\n");
    assert!(cs(&src).is_ok());
}

#[test]
fn oneof_declared_after_the_reference_still_catches_it() {
    // The check runs once every source is accumulated, so order is irrelevant.
    let src = format!("FACT resolved is censoredmtp\n{ONEOF_RESOLVED}");
    assert!(matches!(
        cs(&src).unwrap_err(),
        CompileError::UnknownValue(_)
    ));
}

#[test]
fn out_of_set_value_inside_a_premise_is_caught() {
    // Closed-world covers references anywhere — not just FACTs.
    let src = format!(
        r"{ONEOF_RESOLVED}
            PREMISE p:
                WHEN resolved is censoredmtp
                THEN x done
        "
    );
    assert!(matches!(
        cs(&src).unwrap_err(),
        CompileError::UnknownValue(_)
    ));
}

#[test]
fn out_of_set_value_inside_a_rule_is_caught() {
    let src = format!(
        r"{ONEOF_RESOLVED}
            RULE r:
                WHEN x go
                THEN resolved is censoredmtp
        "
    );
    assert!(matches!(
        cs(&src).unwrap_err(),
        CompileError::UnknownValue(_)
    ));
}

#[test]
fn binary_atoms_in_a_oneof_do_not_close_anything() {
    // `alice cooks` / `alice cleans` have no object slot, so there is no value
    // set to violate — a later `alice bakes` is just another atom, not an error.
    let src = r"
        PREMISE chores:
            ONEOF
                alice cooks
                alice cleans
        FACT alice bakes
        ";
    assert!(cs(src).is_ok());
}

#[test]
fn a_subject_without_a_oneof_stays_open() {
    // No ONEOF over `mood is …` → open world, any value is a fresh atom.
    let src = format!("{ONEOF_RESOLVED}FACT mood is anything_goes\n");
    assert!(cs(&src).is_ok());
}

#[test]
fn two_oneofs_union_their_declared_values() {
    // A value declared by either ONEOF for the same variable is legal.
    let src = r"
        PREMISE a:
            ONEOF
                v is one
                v is two
        PREMISE b:
            ONEOF
                v is two
                v is three
        FACT v is three
        ";
    assert!(cs(src).is_ok());
}

#[test]
fn earliest_offender_is_reported() {
    // Two violations; the diagnostic points at the first by line.
    let src = format!("{ONEOF_RESOLVED}FACT resolved is firstbad\nFACT resolved is secondbad\n");
    let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
        panic!("expected UnknownValue");
    };
    assert_eq!(e.value, "firstbad");
}

#[test]
fn closed_world_spans_imported_domains() {
    // physics closes `Motor speed …`; main, referencing it via the prefix with
    // a typo, is rejected — the value set is shared across the domain boundary.
    let mut r = MemoryResolver::new();
    r.add(
        "physics.vrf",
        r"
        DOMAIN physics
        PREMISE g:
            ONEOF
                Motor speed slow
                Motor speed fast
        ",
    );
    r.add(
        "main.vrf",
        "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT physics.Motor speed faast\n",
    );
    let CompileError::UnknownValue(e) = compile("main.vrf", &r).unwrap_err() else {
        panic!("expected UnknownValue");
    };
    assert_eq!(e.value, "faast");
    assert_eq!(e.suggestion, " — did you mean `fast`?");
}

#[test]
fn same_value_in_a_different_domain_does_not_clash() {
    // `state is open` is closed in domain a; domain b's own `state is shut`
    // (never declared in a) is fine — value sets are per-domain.
    let mut r = MemoryResolver::new();
    r.add(
        "a.vrf",
        r"
        DOMAIN a
        PREMISE s:
            ONEOF
                state is open
                state is closed
        ",
    );
    r.add("b.vrf", "DOMAIN b\nIMPORT \"a.vrf\"\nFACT state is shut\n");
    // `state is shut` is in domain b, which has no ONEOF → open, so it compiles.
    assert!(compile("b.vrf", &r).is_ok());
}

#[test]
fn short_value_is_still_rejected_just_without_a_guess() {
    // The closed-world error does not depend on the suggestion: an out-of-set
    // single-character value is rejected exactly, only the `did you mean` is
    // suppressed.
    let src = r"
        PREMISE pick:
            ONEOF
                roll is 一
                roll is 二
        FACT roll is 七
        ";
    let CompileError::UnknownValue(e) = cs(src).unwrap_err() else {
        panic!("expected UnknownValue");
    };
    assert_eq!(e.value, "七");
    assert_eq!(e.suggestion, "");
}

// --- FOR EACH / SET (bounded quantification, Phase 1) ------------------

#[test]
fn for_each_grounds_once_per_element() {
    // A ONEOF body over a 2-element set: each element yields one pairwise
    // clause + one at-least-one clause = 2 clauses; 2 elements → 4 clauses,
    // and 4 distinct grounded atoms (a/b × slot m/n).
    let src = r"
        SET xs
            a
            b
        PREMISE slot FOR EACH t IN xs:
            ONEOF
                t slot m
                t slot n
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 4);
    for s in ["a", "b"] {
        for o in ["m", "n"] {
            assert!(c.atoms.contains(&key(s, "slot", Some(o))));
        }
    }
}

#[test]
fn for_each_in_a_rule_derives_per_element() {
    // A quantified RULE grounds to one rule per element.
    let src = r"
        SET xs
            a
            b
        RULE r FOR EACH t IN xs:
            WHEN t on
            THEN t hot
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.rules.len(), 2);
}

#[test]
fn for_each_over_an_undeclared_set_is_rejected() {
    let src = r"
        SET tasks
            a
        PREMISE p FOR EACH t IN taske:
            ONEOF
                t s x
                t s y
        ";
    let CompileError::UnknownSet {
        set, suggestion, ..
    } = cs(src).unwrap_err()
    else {
        panic!("expected UnknownSet");
    };
    assert_eq!(set, "taske");
    assert_eq!(suggestion, " — did you mean `tasks`?");
}

#[test]
fn for_each_closes_each_grounded_variable() {
    // ONEOF inside FOR EACH closes the variable per element, so an out-of-set
    // value on a grounded subject is a hard error (closed-world after subst).
    let src = r"
        SET xs
            a
            b
        PREMISE c FOR EACH t IN xs:
            ONEOF
                t color red
                t color blue
        FACT a color gren
        ";
    let CompileError::UnknownValue(e) = cs(src).unwrap_err() else {
        panic!("expected UnknownValue from the grounded ONEOF");
    };
    assert_eq!(e.value, "gren");
    assert_eq!(e.subject, "a");
}

#[test]
fn nested_for_each_is_a_parse_error() {
    // The structural guarantee: a second FOR EACH is unrepresentable — the
    // header carries exactly one, so nesting fails to parse (no domain
    // product can ever be written).
    let src = r"
        SET xs
            a
        PREMISE p FOR EACH x IN xs FOR EACH y IN xs:
            ONEOF
                x r y
                x s y
        ";
    assert!(matches!(cs(src), Err(CompileError::Parse(_))));
}

#[test]
fn relation_for_each_grounds_per_fact_pair() {
    // Two declared edges → the body is instantiated once per edge (two
    // pairwise clauses), and both edge atoms are recorded as consumed so the
    // ORPHAN lint will not flag them.
    let src = r"
        FACT a linked b
        FACT b linked c
        PREMISE p FOR EACH x linked y:
            FORBIDS
                x hot on
                y hot on
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 2);
    assert_eq!(c.consumed.len(), 2);
    assert!(c.consumed.contains(&id(&c, &key("a", "linked", Some("b")))));
}

#[test]
fn relation_for_each_over_no_edges_is_inert() {
    // A relation with no matching facts grounds to nothing (vacuous), not an
    // error — unlike an undeclared SET.
    let src = r"
        PREMISE p FOR EACH x linked y:
            FORBIDS
                x hot on
                y hot on
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 0);
    assert!(c.consumed.is_empty());
}

#[test]
fn close_transitive_extends_the_relation() {
    // a->b, b->c; CLOSE adds a->c, so a relation FOR EACH grounds over all
    // three pairs (without CLOSE it would be two).
    let src = r"
        FACT a r b
        FACT b r c
        CLOSE r TRANSITIVE
        PREMISE p FOR EACH x r y:
            FORBIDS
                x hot on
                y hot on
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 3);
}

#[test]
fn close_transitive_rejects_a_cycle() {
    let src = r"
        FACT a r b
        FACT b r a
        CLOSE r TRANSITIVE
        ";
    let CompileError::CyclicRelation { relation, .. } = cs(src).unwrap_err() else {
        panic!("expected CyclicRelation");
    };
    assert_eq!(relation, "r");
}

/// Count the directed pairs a relation `r` holds after a CLOSE, by grounding a
/// relation `FOR EACH x r y` over it (one clause per pair). The body is an
/// *implication* (asymmetric in x and y) so (a,b) and (b,a) stay distinct — a
/// symmetric `FORBIDS` would dedup them and undercount.
fn pairs_after_close(close: &str) -> usize {
    let src = format!(
        "FACT a r b\nFACT b r c\n{close}\nPREMISE p FOR EACH x r y:\n    WHEN x src\n    THEN y dst\n"
    );
    cs(&src).unwrap().clauses.len()
}

#[test]
fn close_symmetric_adds_the_back_edge() {
    // a->b, b->c plus their reverses b->a, c->b = 4 pairs.
    assert_eq!(pairs_after_close("CLOSE r SYMMETRIC"), 4);
}

#[test]
fn close_reflexive_adds_self_pairs() {
    // a->b, b->c plus a->a, b->b, c->c (3 nodes) = 5 pairs.
    assert_eq!(pairs_after_close("CLOSE r REFLEXIVE"), 5);
}

#[test]
fn close_equivalence_groups_a_whole_component() {
    // a->b->c connects {a,b,c} into one class: every ordered pair incl. self =
    // 3 x 3 = 9. A cycle would be fine here (no DAG requirement), unlike TRANSITIVE.
    assert_eq!(pairs_after_close("CLOSE r EQUIVALENCE"), 9);
}

#[test]
fn close_equivalence_does_not_reject_a_cycle() {
    // EQUIVALENCE expects cycles (they are the classes), so a back-edge that
    // would make TRANSITIVE fail is accepted here.
    let src = "FACT a r b\nFACT b r a\nCLOSE r EQUIVALENCE\n";
    assert!(cs(src).is_ok());
}

#[test]
fn close_scc_isolates_a_directed_cycle() {
    // a<->b form a strongly-connected pair; c is reachable from b but does not
    // reach back, so it is its own singleton. SCC keeps the mutual pairs of
    // {a,b} (a-a,a-b,b-a,b-b) + c-c = 5, and does NOT error on the cycle.
    let src = "FACT a r b\nFACT b r a\nFACT b r c\nCLOSE r SCC\nPREMISE p FOR EACH x r y:\n    WHEN x src\n    THEN y dst\n";
    assert_eq!(cs(src).unwrap().clauses.len(), 5);
}

#[test]
fn exists_grounds_to_one_at_least_one_over_the_set() {
    // ∃ over a 2-element set = a single at-least-one clause over the two
    // instantiated atoms (exactly an ATLEAST whose atoms come from the set).
    let src = r"
        SET handlers
            a
            b
        PREMISE covered:
            EXISTS h IN handlers
                h handles request
        ";
    let c = cs(src).unwrap();
    assert_eq!(c.clauses.len(), 1);
    assert!(c.atoms.contains(&key("a", "handles", Some("request"))));
    assert!(c.atoms.contains(&key("b", "handles", Some("request"))));
}

#[test]
fn exists_matches_a_hand_written_atleast() {
    // Oracle: EXISTS over {a,b} produces the same clause set as a manual ATLEAST
    // of the two instantiated atoms.
    let via_exists =
        cs("SET hs\n    a\n    b\nPREMISE p:\n    EXISTS h IN hs\n        h does x\n").unwrap();
    let via_atleast = cs("PREMISE p:\n    ATLEAST\n        a does x\n        b does x\n").unwrap();
    assert_eq!(via_exists.clauses.len(), 1);
    assert_eq!(via_exists.clauses.len(), via_atleast.clauses.len());
}

#[test]
fn exists_over_an_undeclared_set_is_rejected() {
    let src = "SET handlers\n    a\nPREMISE p:\n    EXISTS h IN handlerz\n        h does x\n";
    let CompileError::UnknownSet { set, .. } = cs(src).unwrap_err() else {
        panic!("expected UnknownSet");
    };
    assert_eq!(set, "handlerz");
}

#[test]
fn grounding_count_is_linear_in_the_set() {
    // No domain product: N elements → exactly N groundings (here N clauses,
    // one at-least-one per element), never N².
    let elems: String = (0..20).map(|i| format!("    e{i}\n")).collect();
    let src = format!(
        "SET xs\n{elems}PREMISE p FOR EACH t IN xs:\n    ATLEAST\n        t a\n        t b\n"
    );
    let c = cs(&src).unwrap();
    assert_eq!(c.clauses.len(), 20);
}
