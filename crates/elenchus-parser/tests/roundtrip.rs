//! Property-based round-trip: generate a random *valid* program, render it to
//! `.vrf` text, parse it back, and assert the AST matches what we generated.
//! Any rendering the grammar accepts but the parser mis-reads is a bug.

use elenchus_parser::{Body, ListOp, Statement, parse};
use proptest::prelude::*;

type Atom3 = (String, String, Option<String>);
type Lit3 = (bool, Atom3);

#[derive(Clone, Debug)]
enum Stmt {
    Fact(Atom3),
    Not(Atom3),
    Import(String),
    AxiomList {
        name: String,
        op: &'static str,
        atoms: Vec<Atom3>,
    },
    Impl {
        rule: bool,
        name: String,
        ante: Vec<Lit3>,
        cons: Vec<Lit3>,
    },
    Check {
        subj: Option<String>,
        bidir: bool,
    },
}

// --- rendering (AST spec -> .vrf text) -------------------------------------

fn render_atom(a: &Atom3) -> String {
    match &a.2 {
        Some(o) => format!("{} {} {}", a.0, a.1, o),
        None => format!("{} {}", a.0, a.1),
    }
}

fn render_lit(l: &Lit3) -> String {
    let atom = render_atom(&l.1);
    if l.0 { format!("NOT {atom}") } else { atom }
}

fn render(stmts: &[Stmt]) -> String {
    let mut s = String::new();
    for st in stmts {
        match st {
            Stmt::Fact(a) => s.push_str(&format!("FACT {}\n", render_atom(a))),
            Stmt::Not(a) => s.push_str(&format!("NOT {}\n", render_atom(a))),
            Stmt::Import(p) => s.push_str(&format!("IMPORT \"{p}\"\n")),
            Stmt::AxiomList { name, op, atoms } => {
                s.push_str(&format!("AXIOM {name}:\n    {op}\n"));
                for a in atoms {
                    s.push_str(&format!("        {}\n", render_atom(a)));
                }
            }
            Stmt::Impl {
                rule,
                name,
                ante,
                cons,
            } => {
                let kw = if *rule { "RULE" } else { "AXIOM" };
                s.push_str(&format!("{kw} {name}:\n"));
                s.push_str(&format!("    WHEN {}\n", render_lit(&ante[0])));
                for l in &ante[1..] {
                    s.push_str(&format!("    AND {}\n", render_lit(l)));
                }
                s.push_str(&format!("    THEN {}\n", render_lit(&cons[0])));
                for l in &cons[1..] {
                    s.push_str(&format!("    AND {}\n", render_lit(l)));
                }
            }
            Stmt::Check { subj, bidir } => {
                s.push_str("CHECK");
                if let Some(x) = subj {
                    s.push_str(&format!(" {x}"));
                }
                if *bidir {
                    s.push_str(" BIDIRECTIONAL");
                }
                s.push('\n');
            }
        }
    }
    s
}

// --- comparison (parsed AST vs spec) ---------------------------------------

fn atom_eq(p: &elenchus_parser::Atom, s: &Atom3) -> bool {
    p.subject == s.0 && p.predicate == s.1 && p.object == s.2.as_deref()
}

fn lits_eq(p: &[elenchus_parser::Located<elenchus_parser::Literal>], s: &[Lit3]) -> bool {
    p.len() == s.len()
        && p.iter()
            .zip(s)
            .all(|(pl, sl)| pl.data.negated == sl.0 && atom_eq(&pl.data.atom, &sl.1))
}

fn op_eq(p: ListOp, s: &str) -> bool {
    matches!(
        (p, s),
        (ListOp::Exclusive, "EXCLUSIVE")
            | (ListOp::Forbids, "FORBIDS")
            | (ListOp::OneOf, "ONEOF")
            | (ListOp::AtLeast, "ATLEAST")
    )
}

fn stmt_eq(p: &Statement, s: &Stmt) -> bool {
    match (p, s) {
        (Statement::Fact(a), Stmt::Fact(b)) => atom_eq(&a.data, b),
        (Statement::Negation(a), Stmt::Not(b)) => atom_eq(&a.data, b),
        (Statement::Import(a), Stmt::Import(b)) => a.data == b,
        (
            Statement::Axiom {
                name,
                body: Body::List { op, atoms },
            },
            Stmt::AxiomList {
                name: n,
                op: o,
                atoms: ats,
            },
        ) => {
            name.data == n
                && op_eq(*op, o)
                && atoms.len() == ats.len()
                && atoms.iter().zip(ats).all(|(p, s)| atom_eq(&p.data, s))
        }
        (
            Statement::Axiom {
                name,
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                    },
            },
            Stmt::Impl {
                rule: false,
                name: n,
                ante,
                cons,
            },
        )
        | (
            Statement::Rule {
                name,
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                    },
            },
            Stmt::Impl {
                rule: true,
                name: n,
                ante,
                cons,
            },
        ) => name.data == n && lits_eq(antecedent, ante) && lits_eq(consequent, cons),
        (
            Statement::Check {
                subject,
                bidirectional,
            },
            Stmt::Check { subj, bidir },
        ) => subject.as_ref().map(|x| x.data) == subj.as_deref() && bidirectional == bidir,
        _ => false,
    }
}

// --- generators ------------------------------------------------------------

fn ident() -> impl Strategy<Value = String> {
    // lowercase identifiers can never collide with the CAPS reserved words.
    "[a-z][a-z0-9_]{0,4}"
}

fn atom() -> impl Strategy<Value = Atom3> {
    (ident(), ident(), prop::option::of(ident()))
}

fn lit() -> impl Strategy<Value = Lit3> {
    (any::<bool>(), atom())
}

fn stmt() -> impl Strategy<Value = Stmt> {
    prop_oneof![
        atom().prop_map(Stmt::Fact),
        atom().prop_map(Stmt::Not),
        "[a-z][a-z0-9_.]{0,8}".prop_map(Stmt::Import),
        (
            ident(),
            prop::sample::select(vec!["EXCLUSIVE", "FORBIDS", "ONEOF", "ATLEAST"]),
            prop::collection::vec(atom(), 2..5),
        )
            .prop_map(|(name, op, atoms)| Stmt::AxiomList { name, op, atoms }),
        (
            any::<bool>(),
            ident(),
            prop::collection::vec(lit(), 1..4),
            prop::collection::vec(lit(), 1..4),
        )
            .prop_map(|(rule, name, ante, cons)| Stmt::Impl {
                rule,
                name,
                ante,
                cons,
            }),
        (prop::option::of(ident()), any::<bool>())
            .prop_map(|(subj, bidir)| Stmt::Check { subj, bidir }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// Generated AST → text → parse → same AST.
    #[test]
    fn ast_survives_render_and_reparse(spec in prop::collection::vec(stmt(), 1..8)) {
        let text = render(&spec);
        let parsed = parse(&text).map_err(|e| TestCaseError::fail(format!("{e}\n--- source ---\n{text}")))?;
        prop_assert_eq!(parsed.statements.len(), spec.len());
        for (p, s) in parsed.statements.iter().zip(&spec) {
            prop_assert!(stmt_eq(p, s), "mismatch:\n  parsed: {:?}\n  spec:   {:?}\n  text:\n{}", p, s, text);
        }
    }
}
