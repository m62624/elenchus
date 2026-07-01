//! Property-based round-trip: generate a random *valid* program, render it to
//! `.vrf` text, parse it back, and assert the AST matches what we generated.
//! Any rendering the grammar accepts but the parser mis-reads is a bug.

use elenchus_parser::{Body, CloseKind, ExistsDomain, ListOp, Statement, parse};
use proptest::prelude::*;

/// `(subject, predicate, object)`. `predicate == None` is a **bare atom** (a
/// single-word proposition, e.g. a `VAR` port used in a body); `object` is only
/// ever `Some` when `predicate` is too (the grammar gates it).
type Atom3 = (String, Option<String>, Option<String>);
type Lit3 = (bool, Atom3);

#[derive(Clone, Debug)]
enum Stmt {
    Fact {
        atom: Atom3,
        /// `Some` → render a `BECAUSE <ground>` justification tail.
        because: Option<Atom3>,
    },
    Not(Atom3),
    Import(String),
    Var {
        name: String,
        default: Option<bool>,
    },
    Provide {
        atom: Atom3,
        value: bool,
    },
    PremiseList {
        name: String,
        op: &'static str,
        atoms: Vec<Atom3>,
    },
    Impl {
        rule: bool,
        name: String,
        ante: Vec<Lit3>,
        cons: Vec<Lit3>,
        /// `UNLESS` exceptions (only ever generated for a RULE).
        exc: Vec<Lit3>,
    },
    Check {
        subj: Option<String>,
        bidir: bool,
    },
    Close {
        relation: String,
        kind: &'static str,
    },
    ExistsPremise {
        name: String,
        binder: String,
        set: String,
        /// `true` → render `WITNESS <set>`; `false` → render `IN <set>`.
        witness: bool,
        atom: Atom3,
    },
}

// --- rendering (AST spec -> .vrf text) -------------------------------------

fn render_atom(a: &Atom3) -> String {
    match (&a.1, &a.2) {
        (Some(p), Some(o)) => format!("{} {} {}", a.0, p, o),
        (Some(p), None) => format!("{} {}", a.0, p),
        // A bare atom: a single word (a `VAR`-style proposition).
        (None, _) => a.0.clone(),
    }
}

fn render_bool(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn render_lit(l: &Lit3) -> String {
    let atom = render_atom(&l.1);
    if l.0 { format!("NOT {atom}") } else { atom }
}

fn render(stmts: &[Stmt]) -> String {
    let mut s = String::new();
    for st in stmts {
        match st {
            Stmt::Fact { atom, because } => {
                s.push_str(&format!("FACT {}", render_atom(atom)));
                if let Some(g) = because {
                    s.push_str(&format!(" BECAUSE {}", render_atom(g)));
                }
                s.push('\n');
            }
            Stmt::Not(a) => s.push_str(&format!("NOT {}\n", render_atom(a))),
            Stmt::Import(p) => s.push_str(&format!("IMPORT \"{p}\"\n")),
            Stmt::Var { name, default } => {
                s.push_str(&format!("VAR {name}"));
                if let Some(d) = default {
                    s.push_str(&format!(" DEFAULT {}", render_bool(*d)));
                }
                s.push('\n');
            }
            Stmt::Provide { atom, value } => {
                s.push_str(&format!(
                    "PROVIDE {}: {}\n",
                    render_atom(atom),
                    render_bool(*value)
                ));
            }
            Stmt::PremiseList { name, op, atoms } => {
                s.push_str(&format!("PREMISE {name}:\n    {op}\n"));
                for a in atoms {
                    s.push_str(&format!("        {}\n", render_atom(a)));
                }
            }
            Stmt::Impl {
                rule,
                name,
                ante,
                cons,
                exc,
            } => {
                let kw = if *rule { "RULE" } else { "PREMISE" };
                s.push_str(&format!("{kw} {name}:\n"));
                s.push_str(&format!("    WHEN {}\n", render_lit(&ante[0])));
                for l in &ante[1..] {
                    s.push_str(&format!("    AND {}\n", render_lit(l)));
                }
                s.push_str(&format!("    THEN {}\n", render_lit(&cons[0])));
                for l in &cons[1..] {
                    s.push_str(&format!("    AND {}\n", render_lit(l)));
                }
                for l in exc {
                    s.push_str(&format!("    UNLESS {}\n", render_lit(l)));
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
            Stmt::Close { relation, kind } => s.push_str(&format!("CLOSE {relation} {kind}\n")),
            Stmt::ExistsPremise {
                name,
                binder,
                set,
                witness,
                atom,
            } => {
                let dom = if *witness {
                    format!("WITNESS {set}")
                } else {
                    format!("IN {set}")
                };
                s.push_str(&format!(
                    "PREMISE {name}:\n    EXISTS {binder} {dom}\n        {}\n",
                    render_atom(atom)
                ));
            }
        }
    }
    s
}

// --- comparison (parsed AST vs spec) ---------------------------------------

fn atom_eq(p: &elenchus_parser::Atom, s: &Atom3) -> bool {
    // `predicate` and `object` are both optional: a `None` predicate is a bare
    // atom (single-word proposition), which the grammar must reparse intact.
    p.subject == s.0 && p.predicate == s.1.as_deref() && p.object == s.2.as_deref()
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

fn close_kind_eq(p: CloseKind, s: &str) -> bool {
    matches!(
        (p, s),
        (CloseKind::Transitive, "TRANSITIVE")
            | (CloseKind::Symmetric, "SYMMETRIC")
            | (CloseKind::Reflexive, "REFLEXIVE")
            | (CloseKind::Equivalence, "EQUIVALENCE")
            | (CloseKind::Scc, "SCC")
    )
}

fn stmt_eq(p: &Statement, s: &Stmt) -> bool {
    match (p, s) {
        (
            Statement::Fact {
                atom: a,
                because: pb,
            },
            Stmt::Fact {
                atom: b,
                because: sb,
            },
        ) => {
            atom_eq(&a.data, b)
                && match (pb, sb) {
                    (Some(pg), Some(sg)) => atom_eq(&pg.data, sg),
                    (None, None) => true,
                    _ => false,
                }
        }
        (Statement::Negation(a), Stmt::Not(b)) => atom_eq(&a.data, b),
        (Statement::Import { path: a, .. }, Stmt::Import(b)) => a.data == b,
        (
            Statement::Var { name, default },
            Stmt::Var {
                name: n,
                default: d,
            },
        ) => name.data == n && default == d,
        (Statement::Provide { atom, value }, Stmt::Provide { atom: a, value: v }) => {
            atom_eq(&atom.data, a) && value == v
        }
        (
            Statement::Premise {
                name,
                body: Body::List { op, atoms },
                ..
            },
            Stmt::PremiseList {
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
            Statement::Premise {
                name,
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        exceptions,
                        ..
                    },
                ..
            },
            Stmt::Impl {
                rule: false,
                name: n,
                ante,
                cons,
                exc,
            },
        )
        | (
            Statement::Rule {
                name,
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        exceptions,
                        ..
                    },
                ..
            },
            Stmt::Impl {
                rule: true,
                name: n,
                ante,
                cons,
                exc,
            },
        ) => {
            name.data == n
                && lits_eq(antecedent, ante)
                && lits_eq(consequent, cons)
                && lits_eq(exceptions, exc)
        }
        (
            Statement::Check {
                subject,
                bidirectional,
            },
            Stmt::Check { subj, bidir },
        ) => subject.as_ref().map(|x| x.data) == subj.as_deref() && bidirectional == bidir,
        (
            Statement::Close { relation, kind },
            Stmt::Close {
                relation: r,
                kind: k,
            },
        ) => relation.data == r && close_kind_eq(*kind, k),
        (
            Statement::Premise {
                name,
                body:
                    Body::Exists {
                        binder,
                        domain,
                        atom,
                    },
                ..
            },
            Stmt::ExistsPremise {
                name: n,
                binder: b,
                set: st,
                witness: w,
                atom: a,
            },
        ) => {
            let dom_eq = match domain {
                ExistsDomain::InSet(s) => !*w && s.data == st,
                ExistsDomain::Witness(t) => *w && t.data == st,
                ExistsDomain::Open => false, // proptest never generates the open form
            };
            name.data == n && binder.data == b && dom_eq && atom_eq(&atom.data, a)
        }
        _ => false,
    }
}

// --- generators ------------------------------------------------------------

fn ident() -> impl Strategy<Value = String> {
    // lowercase identifiers can never collide with the CAPS reserved words.
    "[a-z][a-z0-9_]{0,4}"
}

fn atom() -> impl Strategy<Value = Atom3> {
    // Three shapes, all legal: bare (`s`), two-word (`s p`), three-word (`s p o`).
    // An object only appears with a predicate, mirroring the grammar's gating.
    (
        ident(),
        prop::option::of((ident(), prop::option::of(ident()))),
    )
        .prop_map(|(s, rest)| match rest {
            Some((p, o)) => (s, Some(p), o),
            None => (s, None, None),
        })
}

fn lit() -> impl Strategy<Value = Lit3> {
    (any::<bool>(), atom())
}

fn stmt() -> impl Strategy<Value = Stmt> {
    prop_oneof![
        (atom(), prop::option::of(atom())).prop_map(|(atom, because)| Stmt::Fact { atom, because }),
        atom().prop_map(Stmt::Not),
        "[a-z][a-z0-9_.]{0,8}".prop_map(Stmt::Import),
        (ident(), prop::option::of(any::<bool>()))
            .prop_map(|(name, default)| Stmt::Var { name, default }),
        (atom(), any::<bool>()).prop_map(|(atom, value)| Stmt::Provide { atom, value }),
        (
            ident(),
            prop::sample::select(vec!["EXCLUSIVE", "FORBIDS", "ONEOF", "ATLEAST"]),
            prop::collection::vec(atom(), 2..5),
        )
            .prop_map(|(name, op, atoms)| Stmt::PremiseList { name, op, atoms }),
        (
            any::<bool>(),
            ident(),
            prop::collection::vec(lit(), 1..4),
            prop::collection::vec(lit(), 1..4),
            prop::collection::vec(lit(), 0..3),
        )
            .prop_map(|(rule, name, ante, cons, exc)| Stmt::Impl {
                rule,
                name,
                ante,
                cons,
                // UNLESS is RULE-only; never attach exceptions to a PREMISE.
                exc: if rule { exc } else { Vec::new() },
            }),
        (prop::option::of(ident()), any::<bool>())
            .prop_map(|(subj, bidir)| Stmt::Check { subj, bidir }),
        (
            ident(),
            prop::sample::select(vec![
                "TRANSITIVE",
                "SYMMETRIC",
                "REFLEXIVE",
                "EQUIVALENCE",
                "SCC",
            ]),
        )
            .prop_map(|(relation, kind)| Stmt::Close { relation, kind }),
        (ident(), ident(), ident(), any::<bool>(), atom()).prop_map(
            |(name, binder, set, witness, atom)| Stmt::ExistsPremise {
                name,
                binder,
                set,
                witness,
                atom,
            },
        ),
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
