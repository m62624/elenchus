//! elenchus-parser — parses the English-like elenchus DSL into an AST.
//!
//! Style mirrors `vsm-parser`: zero-copy over `&str`, `nom` + `nom_locate`
//! for line/column tracking, and human-friendly syntax diagnostics. Syntax is
//! line/keyword-oriented (not S-expressions) so small models cannot trip on
//! parentheses or indentation.
//!
//! Grammar (see docs/SPEC.md, "Grammar (EBNF)"):
//! - statements are newline-terminated; indentation is cosmetic, not significant;
//! - keywords are ALWAYS CAPS (ASCII); identifiers are content (case-sensitive,
//!   verbatim, any-script letters — e.g. `условие`, `名前`);
//! - block boundaries (PREMISE/RULE bodies) are found by keywords, never by indent.
//!
//! On error, [`parse`] returns [`Diagnostics`]: *every* syntax error from one
//! pass, each rendered as a caret block with the keyword's correct syntax (see
//! [`diag`] and [`syntax`]).
//!
//! The crate is split into focused modules — [`ast`] (the tree), [`keywords`]
//! (the single keyword table: spellings, roles, syntax cards), [`diag`] (error
//! rendering), and `grammar` (the nom parser + recovering driver) — re-exported
//! here as a flat public surface.
//!
//! # Example
//!
//! ```
//! use elenchus_parser::{Statement, parse};
//!
//! // One statement per line; the result is a flat list of `Statement`s.
//! let program = parse("FACT socrates is human\nCHECK socrates\n").unwrap();
//! assert_eq!(program.statements.len(), 2);
//! assert!(matches!(program.statements[0], Statement::Fact(_)));
//! ```
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

pub mod ast;
pub mod diag;
mod grammar;
pub mod keywords;

pub use ast::{Atom, Body, Conn, ListOp, Literal, Located, Program, Quant, Span, Statement};
pub use diag::{Diagnostic, Diagnostics};
pub use grammar::parse;
pub use keywords::{Card, KEYWORDS, Keyword, card_for, is_reserved, kw};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn prog(src: &str) -> Program<'_> {
        parse(src).expect("should parse")
    }

    /// `(subject, predicate, object?)` of one atom, borrowed from the source.
    type AtomShape<'a> = (&'a str, &'a str, Option<&'a str>);
    /// A list premise flattened to `(operator, its atoms)`.
    type ListShape<'a> = (ListOp, Vec<AtomShape<'a>>);

    use alloc::vec::Vec;

    /// Atom data flattened to owned tuples — span-independent, for structural
    /// comparison (spans differ by offset, which is exactly what "cosmetic" means).
    fn atom_shapes<'a>(p: &Program<'a>) -> Vec<ListShape<'a>> {
        p.statements
            .iter()
            .filter_map(|s| match s {
                Statement::Premise {
                    body: Body::List { op, atoms },
                    ..
                } => Some((
                    *op,
                    atoms
                        .iter()
                        .map(|a| (a.data.subject, a.data.predicate, a.data.object))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_fact_and_negation() {
        let p = prog(
            r#"
        FACT Creature_A has flying
        NOT Creature_A has cold_blood
        "#,
        );
        assert_eq!(p.statements.len(), 2);
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "Creature_A");
                assert_eq!(a.data.predicate, "has");
                assert_eq!(a.data.object, Some("flying"));
            }
            other => panic!("expected fact, got {:?}", other),
        }
        match &p.statements[1] {
            Statement::Negation(a) => {
                assert_eq!(a.data.object, Some("cold_blood"));
            }
            other => panic!("expected negation, got {:?}", other),
        }
    }

    #[test]
    fn fact_without_object() {
        let p = prog("FACT Motor over_100\n");
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "Motor");
                assert_eq!(a.data.predicate, "over_100");
                assert_eq!(a.data.object, None);
            }
            other => panic!("expected fact, got {:?}", other),
        }
    }

    #[test]
    fn parses_assume_positive_and_negated() {
        let p = prog(
            r#"
        ASSUME rel in_prod
        ASSUME NOT rel has_rollback
        "#,
        );
        assert_eq!(p.statements.len(), 2);
        match &p.statements[0] {
            Statement::Assume(l) => {
                assert!(!l.data.negated);
                assert_eq!(l.data.atom.subject, "rel");
                assert_eq!(l.data.atom.predicate, "in_prod");
                assert_eq!(l.data.atom.object, None);
            }
            other => panic!("expected assume, got {:?}", other),
        }
        match &p.statements[1] {
            Statement::Assume(l) => {
                assert!(l.data.negated);
                assert_eq!(l.data.atom.predicate, "has_rollback");
            }
            other => panic!("expected negated assume, got {:?}", other),
        }
    }

    #[test]
    fn assume_is_a_reserved_word() {
        assert!(parse("FACT ASSUME has x\n").is_err());
    }

    #[test]
    fn parses_import() {
        let p = prog("IMPORT \"physics.vrf\"\n");
        match &p.statements[0] {
            Statement::Import { path, alias } => {
                assert_eq!(path.data, "physics.vrf");
                assert!(alias.is_none());
            }
            other => panic!("expected import, got {:?}", other),
        }
    }

    #[test]
    fn parses_import_with_alias() {
        let p = prog("IMPORT \"physics.vrf\" AS phys\n");
        match &p.statements[0] {
            Statement::Import { path, alias } => {
                assert_eq!(path.data, "physics.vrf");
                assert_eq!(alias.as_ref().unwrap().data, "phys");
            }
            other => panic!("expected import, got {:?}", other),
        }
    }

    #[test]
    fn parses_domain_declaration() {
        let p = prog("DOMAIN physics\n");
        match &p.statements[0] {
            Statement::Domain(name) => assert_eq!(name.data, "physics"),
            other => panic!("expected domain, got {:?}", other),
        }
    }

    #[test]
    fn parses_domain_qualified_atom() {
        // `physics.Motor over_200` → domain prefix split from the subject.
        let p = prog("FACT physics.Motor over_200\n");
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.domain, Some("physics"));
                assert_eq!(a.data.subject, "Motor");
                assert_eq!(a.data.predicate, "over_200");
            }
            other => panic!("expected fact, got {:?}", other),
        }
    }

    #[test]
    fn bare_atom_has_no_domain() {
        let p = prog("FACT engine has_fuel\n");
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.domain, None);
                assert_eq!(a.data.subject, "engine");
            }
            other => panic!("expected fact, got {:?}", other),
        }
    }

    #[test]
    fn domain_is_a_reserved_word() {
        assert!(parse("FACT DOMAIN has x\n").is_err());
        assert!(parse("FACT AS has x\n").is_err());
    }

    #[test]
    fn parses_exclusive_premise() {
        let src = r#"
        PREMISE fly_xor_swim:
            EXCLUSIVE
                Creature_A has flying
                Creature_A has swimming
        "#;
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise { name, body, .. } => {
                assert_eq!(name.data, "fly_xor_swim");
                match body {
                    Body::List { op, atoms } => {
                        assert_eq!(*op, ListOp::Exclusive);
                        assert_eq!(atoms.len(), 2);
                        assert_eq!(atoms[1].data.object, Some("swimming"));
                    }
                    other => panic!("expected list body, got {:?}", other),
                }
            }
            other => panic!("expected premise, got {:?}", other),
        }
    }

    #[test]
    fn parses_implication_premise_with_and() {
        let src = r#"
        PREMISE wings_need_bone:
            WHEN Creature_A has flying
            THEN Creature_A has wing
            AND  Creature_A has bone
        "#;
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 1);
                assert_eq!(antecedent[0].data.atom.object, Some("flying"));
                assert_eq!(consequent.len(), 2);
                assert_eq!(consequent[0].data.atom.object, Some("wing"));
                assert_eq!(consequent[1].data.atom.object, Some("bone"));
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn antecedent_and_goes_before_then() {
        let src = r#"
        PREMISE deploy:
            WHEN s tested
            AND s reviewed
            THEN s can_deploy
        "#;
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 2);
                assert_eq!(consequent.len(), 1);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn when_or_sets_disjunctive_antecedent() {
        let src = r#"
        PREMISE p:
            WHEN x a
            OR x b
            THEN x c
        "#;
        match &prog(src).statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        ante_conn,
                        consequent,
                        cons_conn,
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 2);
                assert_eq!(*ante_conn, Conn::Or);
                assert_eq!(consequent.len(), 1);
                assert_eq!(*cons_conn, Conn::And); // single consequent → AND
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn then_or_sets_disjunctive_consequent() {
        let src = r#"
        PREMISE p:
            WHEN x a
            THEN x b
            OR x c
        "#;
        match &prog(src).statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        consequent,
                        cons_conn,
                        ..
                    },
                ..
            } => {
                assert_eq!(consequent.len(), 2);
                assert_eq!(*cons_conn, Conn::Or);
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn mixing_and_or_in_one_group_is_an_error() {
        let mixed_when = r#"
        PREMISE p:
            WHEN x a
            AND x b
            OR x c
            THEN x d
        "#;
        let mixed_then = r#"
        PREMISE p:
            WHEN x a
            THEN x b
            AND x c
            OR x d
        "#;
        assert!(parse(mixed_when).is_err());
        assert!(parse(mixed_then).is_err());
    }

    #[test]
    fn or_is_a_reserved_word() {
        assert!(parse("FACT OR has x\n").is_err());
    }

    #[test]
    fn parses_negated_literal_in_rule() {
        let src = r#"
        RULE pick_slow:
            WHEN NOT Motor over_100
            THEN Motor uses slow_path
        "#;
        let p = prog(src);
        match &p.statements[0] {
            Statement::Rule {
                body: Body::Impl { antecedent, .. },
                ..
            } => {
                assert!(antecedent[0].data.negated);
                assert_eq!(antecedent[0].data.atom.predicate, "over_100");
            }
            other => panic!("expected rule, got {:?}", other),
        }
    }

    #[test]
    fn parses_check_variants() {
        let p = prog("CHECK Creature_A BIDIRECTIONAL\n");
        match &p.statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert_eq!(subject.as_ref().unwrap().data, "Creature_A");
                assert!(bidirectional);
            }
            other => panic!("expected check, got {:?}", other),
        }

        let p = prog("CHECK\n");
        match &p.statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert!(subject.is_none());
                assert!(!bidirectional);
            }
            other => panic!("expected check, got {:?}", other),
        }
    }

    #[test]
    fn comments_and_blanks_are_ignored() {
        let src = "// header\n\nFACT a b   // trailing comment\n\n// tail\n";
        let p = prog(src);
        assert_eq!(p.statements.len(), 1);
    }

    #[test]
    fn indentation_is_cosmetic() {
        let flat = r#"
        PREMISE x:
        EXCLUSIVE
        a b
        a c
        "#;
        let indented = r#"
        PREMISE x:
                EXCLUSIVE
          a b
                    a c
        "#;
        // Spans differ by offset (cosmetic); the parsed structure must be identical.
        assert_eq!(atom_shapes(&prog(flat)), atom_shapes(&prog(indented)));
    }

    #[test]
    fn top_level_statements_may_be_indented() {
        // Leading indentation on the FACT/PREMISE/CHECK lines themselves is also
        // cosmetic (so a whole program can be pasted indented inside a here-doc).
        let flat = r#"
        FACT x a
        NOT x b
        CHECK x
        "#;
        let indented = r#"
            FACT x a
                NOT x b
            CHECK x
        "#;
        assert_eq!(atom_shapes(&prog(flat)), atom_shapes(&prog(indented)));
        assert_eq!(prog(indented).statements.len(), 3);
    }

    #[test]
    fn full_creature_example_parses() {
        let src = include_str!("../../../docs/examples/creature.vrf");
        let p = prog(src);
        // 1 DOMAIN + 2 FACT + 3 PREMISE + 1 RULE + 1 CHECK = 8
        assert_eq!(p.statements.len(), 8);
    }

    #[test]
    fn import_demo_example_parses() {
        let src = include_str!("../../../docs/examples/import-demo.vrf");
        let p = prog(src);
        assert!(matches!(p.statements[0], Statement::Domain(_)));
        assert!(matches!(p.statements[1], Statement::Import { .. }));
    }

    #[test]
    fn unicode_identifiers_any_script() {
        // Cyrillic subject/predicate/object, mixed with `_` and digits (not first).
        let p = prog(
            r#"
        FACT кот пушистый2
        NOT собака has крылья
        "#,
        );
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "кот");
                assert_eq!(a.data.predicate, "пушистый2");
                assert_eq!(a.data.object, None);
            }
            other => panic!("expected fact, got {:?}", other),
        }
        match &p.statements[1] {
            Statement::Negation(a) => {
                assert_eq!(a.data.subject, "собака");
                assert_eq!(a.data.object, Some("крылья"));
            }
            other => panic!("expected negation, got {:?}", other),
        }
    }

    #[test]
    fn unicode_premise_name_and_body() {
        let src = r#"
        PREMISE правило_лая:
            WHEN собака has хвост
            THEN собака умеет_лаять
        "#;
        match &prog(src).statements[0] {
            Statement::Premise { name, body, .. } => {
                assert_eq!(name.data, "правило_лая");
                match body {
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    } => {
                        assert_eq!(antecedent[0].data.atom.subject, "собака");
                        assert_eq!(consequent[0].data.atom.subject, "собака");
                        assert_eq!(consequent[0].data.atom.predicate, "умеет_лаять");
                    }
                    other => panic!("expected impl body, got {:?}", other),
                }
            }
            other => panic!("expected premise, got {:?}", other),
        }
    }

    #[test]
    fn identifier_cannot_start_with_digit() {
        // `2cats` is not a valid subject — first char must be a letter.
        assert!(parse("FACT 2cats has fur\n").is_err());
    }

    #[test]
    fn punctuation_is_rejected_in_identifier() {
        // `!` and other symbols are not identifier characters.
        assert!(parse("FACT cat! has fur\n").is_err());
    }

    #[test]
    fn reserved_word_cannot_be_identifier() {
        // `WHEN` as a subject is illegal.
        assert!(parse("FACT WHEN has x\n").is_err());
    }

    #[test]
    fn pretty_error_points_at_offending_line() {
        let src = r#"FACT a b
!garbage here
FACT c d
"#;
        let err = parse(src).expect_err("should fail");
        let shown = format!("{}", err);
        // The new diagnostic format: a RESULT header, the line number, the
        // verbatim offending line, and a caret pointing at it.
        assert!(shown.contains("RESULT: 1 syntax error"));
        assert!(shown.contains("line 2"));
        assert!(shown.contains("!garbage here"));
        assert!(shown.contains('^'));
    }

    #[test]
    fn collects_every_error_in_one_pass() {
        // Three broken top-level lines among valid ones → exactly three errors,
        // no cascade from recovery.
        let src = "FACT lonely\nFACT a b\nNOT also_lonely\nCHECK\nIMPORT nothx\n";
        let diags = parse(src).expect_err("should fail");
        assert_eq!(diags.len(), 3);
    }

    #[test]
    fn crlf_line_endings() {
        let p = prog(
            r#"
        FACT a b
        CHECK a
        "#,
        );
        assert_eq!(p.statements.len(), 2);
    }

    #[test]
    fn tabs_as_indentation() {
        let p = prog(
            r#"
        PREMISE e:
	EXCLUSIVE
		x a
		x b
        "#,
        );
        assert!(matches!(
            p.statements[0],
            Statement::Premise {
                body: Body::List {
                    op: ListOp::Exclusive,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_all_list_ops() {
        for (kw, want) in [
            ("EXCLUSIVE", ListOp::Exclusive),
            ("FORBIDS", ListOp::Forbids),
            ("ONEOF", ListOp::OneOf),
            ("ATLEAST", ListOp::AtLeast),
        ] {
            let src = alloc::format!("PREMISE a:\n    {kw}\n        x a\n        x b\n");
            match &prog(&src).statements[0] {
                Statement::Premise {
                    body: Body::List { op, .. },
                    ..
                } => assert_eq!(*op, want),
                other => panic!("{kw}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn check_bidirectional_without_subject() {
        match &prog("CHECK BIDIRECTIONAL\n").statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert!(subject.is_none());
                assert!(bidirectional);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_and_comment_only_input_yield_no_statements() {
        assert_eq!(prog("").statements.len(), 0);
        assert_eq!(prog("// just a comment\n\n   \n").statements.len(), 0);
    }

    #[test]
    fn negation_with_object() {
        match &prog("NOT Creature_A has wing\n").statements[0] {
            Statement::Negation(a) => {
                assert_eq!(a.data.subject, "Creature_A");
                assert_eq!(a.data.object, Some("wing"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn negated_consequent_then_not() {
        let src = r#"
        PREMISE a:
            WHEN x on
            THEN NOT x off
        "#;
        match &prog(src).statements[0] {
            Statement::Premise {
                body: Body::Impl { consequent, .. },
                ..
            } => {
                assert!(consequent[0].data.negated);
                assert_eq!(consequent[0].data.atom.predicate, "off");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn multiple_imports_then_facts() {
        let p = prog(
            r#"
        IMPORT "a.vrf"
        IMPORT "b.vrf"
        FACT x y
        "#,
        );
        assert!(matches!(p.statements[0], Statement::Import { .. }));
        assert!(matches!(p.statements[1], Statement::Import { .. }));
        assert!(matches!(p.statements[2], Statement::Fact(_)));
    }

    #[test]
    fn trailing_comment_without_final_newline() {
        let p = prog("FACT a b\n// trailing, no newline");
        assert_eq!(p.statements.len(), 1);
    }

    #[test]
    fn set_declaration_parses() {
        let p = prog("SET tasks\n    deploy\n    backup\n");
        match &p.statements[0] {
            Statement::Set { name, elements } => {
                assert_eq!(name.data, "tasks");
                let got: Vec<&str> = elements.iter().map(|e| e.data).collect();
                assert_eq!(got, ["deploy", "backup"]);
            }
            other => panic!("expected a SET, got {other:?}"),
        }
    }

    #[test]
    fn for_each_header_parses_into_a_quant() {
        let p = prog("PREMISE p FOR EACH t IN tasks:\n    ONEOF\n        t s a\n        t s b\n");
        match &p.statements[0] {
            Statement::Premise {
                quant: Some(Quant::InSet { binder, set }),
                ..
            } => {
                assert_eq!(binder.data, "t");
                assert_eq!(set.data, "tasks");
            }
            other => panic!("expected a quantified premise, got {other:?}"),
        }
    }

    #[test]
    fn an_unquantified_premise_has_no_quant() {
        let p = prog("PREMISE p:\n    ONEOF\n        x s a\n        x s b\n");
        assert!(matches!(
            &p.statements[0],
            Statement::Premise { quant: None, .. }
        ));
    }

    #[test]
    fn a_malformed_for_each_is_a_committed_error() {
        // FOR without EACH commits (FOR is reserved, only a quantifier here).
        assert!(
            parse("PREMISE p FOR t IN tasks:\n    ONEOF\n        t s a\n        t s b\n").is_err()
        );
    }
}
