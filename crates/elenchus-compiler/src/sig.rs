//! Content-address signatures: stable, order-independent strings for clauses and
//! named bodies (the dedup / redefinition keys), plus the pre-interning raw types.
use crate::domain::DomainCtx;
use crate::error::CompileError;
use crate::ir::{AtomKey, Origin, Value};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use elenchus_parser::{Body, Conn, ExistsDomain, ListOp, Literal, Quant, kw};

/// A literal keyed by atom identity (pre-interning counterpart of [`Lit`]).
#[derive(Clone)]
pub(crate) struct RawLit {
    pub(crate) key: AtomKey,
    pub(crate) negated: bool,
}

/// A fact keyed by atom identity (pre-interning counterpart of [`Fact`]).
pub(crate) struct RawFact {
    pub(crate) key: AtomKey,
    pub(crate) value: Value,
    pub(crate) origin: Origin,
    pub(crate) soft: bool,
}

/// A `FACT … BECAUSE` justification keyed by atom identity (pre-interning
/// counterpart of [`crate::ir::Justification`]).
pub(crate) struct RawJustification {
    pub(crate) belief: AtomKey,
    pub(crate) ground: AtomKey,
    pub(crate) origin: Origin,
}

/// A clause keyed by atom identity (pre-interning counterpart of [`Clause`]).
pub(crate) struct RawClause {
    pub(crate) lits: Vec<RawLit>,
    pub(crate) origin: Origin,
}

/// A rule keyed by atom identity (pre-interning counterpart of [`Rule`]).
pub(crate) struct RawRule {
    pub(crate) antecedent: Vec<RawLit>,
    pub(crate) consequent: Vec<RawLit>,
    /// `UNLESS` exceptions — the rule is defeasible when non-empty.
    pub(crate) exceptions: Vec<RawLit>,
    pub(crate) origin: Origin,
}

/// A canonical signature of a `FOR EACH` quantifier, appended to the body hash so
/// two same-named premises that differ only in their quantifier still count as a
/// redefinition.
pub(crate) fn quant_sig(q: &Quant) -> String {
    match q {
        Quant::InSet { binder, set } => alloc::format!("|FOREACH {} IN {}", binder.data, set.data),
        Quant::Relation {
            left,
            predicate,
            right,
        } => alloc::format!("|FOREACH {} {} {}", left.data, predicate.data, right.data),
    }
}

/// Lower parsed, located literals to key-based [`RawLit`]s (drops spans),
/// resolving each atom's domain through `ctx`.
pub(crate) fn raw_lits(
    lits: &[elenchus_parser::Located<Literal>],
    ctx: &DomainCtx,
) -> Result<Vec<RawLit>, CompileError> {
    lits.iter()
        .map(|l| {
            Ok(RawLit {
                key: ctx.key(&l.data.atom)?,
                negated: l.data.negated,
            })
        })
        .collect()
}

/// The surface keyword for a list op, used as [`Origin::kind`] in the report.
pub(crate) fn list_kind(op: ListOp) -> &'static str {
    match op {
        ListOp::Exclusive => kw::EXCLUSIVE,
        ListOp::Forbids => kw::FORBIDS,
        ListOp::OneOf => kw::ONEOF,
        ListOp::AtLeast => kw::ATLEAST,
    }
}

/// Stable `domain|subject|predicate|object` string for an atom key (the unit from
/// which clause/fact/body signatures are built). Includes the domain so atoms in
/// different domains never share a signature.
pub(crate) fn key_sig(k: &AtomKey) -> String {
    alloc::format!(
        "{}|{}|{}|{}",
        k.domain,
        k.subject,
        k.predicate.as_deref().unwrap_or(""),
        k.object.as_deref().unwrap_or("")
    )
}

/// Sort signature-string parts in place. Every caller's list is a `key|negated`
/// (or key-only) string where equal `Ord` values are byte-identical, so an
/// unstable sort (no stable sort's temp buffer) can never reorder anything
/// observably differently from a stable one.
fn sort_sig_parts(parts: &mut [String]) {
    parts.sort_unstable();
}

/// Canonical, order-independent signature of a clause's literals (for dedup).
pub(crate) fn clause_sig(lits: &[RawLit]) -> String {
    let mut parts: Vec<String> = lits
        .iter()
        .map(|l| alloc::format!("{}|{}", key_sig(&l.key), l.negated as u8))
        .collect();
    sort_sig_parts(&mut parts);
    parts.dedup();
    parts.join(";")
}

/// Canonical body string for a named construct, hashed for redefinition checks.
/// Resolves atom domains through `ctx` so the signature keys on resolved identity.
pub(crate) fn canonical_body(
    name: &str,
    body: &Body,
    is_rule: bool,
    ctx: &DomainCtx,
) -> Result<String, CompileError> {
    let mut s = String::new();
    let _ = write!(
        s,
        "{}|{}|",
        if is_rule { kw::RULE } else { kw::PREMISE },
        name
    );
    match body {
        Body::List { op, atoms } => {
            let _ = write!(s, "LIST|{}|", list_kind(*op));
            let mut keys: Vec<String> = atoms
                .iter()
                .map(|a| Ok(key_sig(&ctx.key(&a.data)?)))
                .collect::<Result<_, CompileError>>()?;
            // Same reasoning as `clause_sig`: equal strings are indistinguishable,
            // so `sort_unstable` is a free win here.
            keys.sort_unstable();
            s.push_str(&keys.join(";"));
        }
        Body::Impl {
            antecedent,
            ante_conn,
            consequent,
            cons_conn,
            exceptions,
        } => {
            let conn = |c: &Conn| if *c == Conn::Or { "OR" } else { "AND" };
            s.push_str("IMPL|ANTE|");
            s.push_str(conn(ante_conn));
            s.push('|');
            s.push_str(&lit_sigs(antecedent, ctx)?);
            s.push_str("|CONS|");
            s.push_str(conn(cons_conn));
            s.push('|');
            s.push_str(&lit_sigs(consequent, ctx)?);
            // Exceptions are part of a rule's identity: two same-named rules that
            // differ only in an UNLESS must not dedup-collapse.
            s.push_str("|EXC|");
            s.push_str(&lit_sigs(exceptions, ctx)?);
        }
        Body::Exists {
            binder,
            domain,
            atom,
        } => {
            match domain {
                // Frozen: the `IN <set>` signature must stay byte-identical (it is
                // a content-address / dedup key). A witness gets its own form.
                ExistsDomain::InSet(set) => {
                    let _ = write!(s, "EXISTS|{}|{}|", binder.data, set.data);
                }
                ExistsDomain::Witness(w) => {
                    let _ = write!(s, "EXISTS|{}|WITNESS {}|", binder.data, w.data);
                }
                ExistsDomain::Open => {
                    let _ = write!(s, "EXISTS|{}|OPEN|", binder.data);
                }
            }
            s.push_str(&key_sig(&ctx.key(&atom.data)?));
        }
    }
    Ok(s)
}

/// Sorted `key|negated` signature of a literal list (order-independent), used
/// inside [`canonical_body`] so reordering a body does not look like a redefinition.
pub(crate) fn lit_sigs(
    lits: &[elenchus_parser::Located<Literal>],
    ctx: &DomainCtx,
) -> Result<String, CompileError> {
    let mut parts: Vec<String> = lits
        .iter()
        .map(|l| {
            Ok(alloc::format!(
                "{}|{}",
                key_sig(&ctx.key(&l.data.atom)?),
                l.data.negated as u8
            ))
        })
        .collect::<Result<_, CompileError>>()?;
    // Equal strings are indistinguishable, so `sort_unstable` is a free win here too.
    parts.sort_unstable();
    Ok(parts.join(";"))
}
