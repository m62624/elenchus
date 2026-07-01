//! Grounding substitution: replace `FOR EACH` binders inside atoms / literals /
//! bodies, and collect the domain prefixes a statement uses.
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use elenchus_parser::{Atom, Body, Literal, Located, Statement};

/// A list of binder substitutions `(name, value)` applied during grounding: one
/// entry for an `IN <set>` quantifier, two for a `<a> <rel> <b>` relation.
pub(crate) type Subs<'s> = [(&'s str, &'s str)];

/// Replace any binder with its value in one identifier; non-matching pass through.
pub(crate) fn subst_ident<'s>(s: &'s str, subs: &Subs<'s>) -> &'s str {
    subs.iter()
        .find_map(|&(b, v)| (s == b).then_some(v))
        .unwrap_or(s)
}

/// Replace the binders in an atom (subject, predicate, and object positions).
pub(crate) fn subst_atom<'s>(a: &Atom<'s>, subs: &Subs<'s>) -> Atom<'s> {
    Atom {
        domain: a.domain,
        subject: subst_ident(a.subject, subs),
        predicate: a.predicate.map(|p| subst_ident(p, subs)),
        object: a.object.map(|o| subst_ident(o, subs)),
    }
}

/// Replace the binders in one located literal (preserving its span and `NOT`).
pub(crate) fn subst_lit<'s>(
    ll: &Located<'s, Literal<'s>>,
    subs: &Subs<'s>,
) -> Located<'s, Literal<'s>> {
    Located {
        data: Literal {
            negated: ll.data.negated,
            atom: subst_atom(&ll.data.atom, subs),
        },
        span: ll.span,
    }
}

/// Build a grounded copy of a body with the `FOR EACH` binders substituted.
/// Spans are preserved so any error still points at the original source line. The
/// result borrows from the original body and from the substitution values, so it
/// is consumed immediately (its keys interned) and never stored.
pub(crate) fn subst_body<'s>(body: &Body<'s>, subs: &Subs<'s>) -> Body<'s> {
    match body {
        Body::List { op, atoms } => Body::List {
            op: *op,
            atoms: atoms
                .iter()
                .map(|la| Located {
                    data: subst_atom(&la.data, subs),
                    span: la.span,
                })
                .collect(),
        },
        Body::Impl {
            antecedent,
            ante_conn,
            consequent,
            cons_conn,
        } => Body::Impl {
            antecedent: antecedent.iter().map(|l| subst_lit(l, subs)).collect(),
            ante_conn: *ante_conn,
            consequent: consequent.iter().map(|l| subst_lit(l, subs)).collect(),
            cons_conn: *cons_conn,
        },
        Body::Exists {
            binder,
            domain,
            atom,
        } => Body::Exists {
            binder: binder.clone(),
            domain: domain.clone(),
            atom: Located {
                data: subst_atom(&atom.data, subs),
                span: atom.span,
            },
        },
    }
}

/// Collect the domain prefixes used by a statement's atoms into `out` (`None` for
/// a bare atom, `Some(p)` for a `p.`-qualified one) — feeds the unused-import lint.
pub(crate) fn collect_prefixes(stmt: &Statement, out: &mut BTreeSet<Option<String>>) {
    let mut add = |a: &Atom| {
        out.insert(a.domain.map(|d| d.to_string()));
    };
    match stmt {
        Statement::Fact(a) | Statement::Negation(a) => add(&a.data),
        Statement::Assume(l) => add(&l.data.atom),
        Statement::Premise { body, .. } | Statement::Rule { body, .. } => match body {
            Body::List { atoms, .. } => atoms.iter().for_each(|a| add(&a.data)),
            Body::Impl {
                antecedent,
                consequent,
                ..
            } => antecedent
                .iter()
                .chain(consequent)
                .for_each(|l| add(&l.data.atom)),
            Body::Exists { atom, .. } => add(&atom.data),
        },
        Statement::Domain(_)
        | Statement::Import { .. }
        | Statement::Check { .. }
        | Statement::Set { .. }
        | Statement::Close { .. }
        | Statement::Var { .. }
        | Statement::Provide { .. } => {}
    }
}
