//! The minimal-unsat-core search: which named constructs / facts are jointly
//! responsible for an unsatisfiable system, via SAT under assumptions.
use crate::cnf::{clause_lit, fact_lit, rule_consequent_clause};
use crate::report::{CoreItem, label};
use crate::sat;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use elenchus_compiler::{Compiled, Origin, Value};

/// The minimal set of `ASSUME` hypotheses to retract so an
/// otherwise-consistent program stops contradicting itself.
///
/// Returns empty unless **all three** hold: there is at least one soft fact; the
/// hard program (facts + premises + rules, no assumptions) is satisfiable on its
/// own; and the full program (with assumptions) is unsatisfiable. In that case
/// the assumptions are the cause, and we deletion-minimize **over the soft facts
/// only** — every hard construct stays active, so a `FACT`/`PREMISE` can never be
/// blamed. What survives is an irreducible set of assumptions that cannot all
/// hold together; dropping any one restores consistency.
///
/// Reuses the same CNF / SAT machinery as [`minimal_unsat_core`]
/// ([`constructs`], [`subset_is_sat`]); the only difference is that hard
/// constructs are pinned active. Labels carry polarity (`NOT …`) so a small
/// model sees exactly what it assumed.
pub(crate) fn retract_assumptions(c: &Compiled) -> Vec<CoreItem> {
    if !c.facts.iter().any(|f| f.soft) {
        return Vec::new();
    }
    let all = constructs(c);
    // The first `c.facts.len()` constructs mirror `c.facts` 1:1 (see `constructs`).
    let is_soft: Vec<bool> = (0..all.len())
        .map(|i| i < c.facts.len() && c.facts[i].soft)
        .collect();

    // The hard program (drop every soft construct) must be consistent on its own,
    // else the facts/premises are to blame and we must not point at assumptions.
    let hard_only: Vec<bool> = is_soft.iter().map(|&s| !s).collect();
    if !subset_is_sat(c.atoms.len(), &all, &hard_only) {
        return Vec::new();
    }
    // The full program must actually be UNSAT for there to be anything to drop.
    let mut active = vec![true; all.len()];
    if subset_is_sat(c.atoms.len(), &all, &active) {
        return Vec::new();
    }
    // Deletion-minimize over the soft constructs only; hard ones stay pinned.
    for i in 0..all.len() {
        if active[i] && is_soft[i] {
            active[i] = false;
            if subset_is_sat(c.atoms.len(), &all, &active) {
                active[i] = true; // still needed for the contradiction
            }
        }
    }
    let mut core: Vec<CoreItem> = (0..all.len())
        .filter(|&i| active[i] && is_soft[i])
        .map(|i| {
            let f = &c.facts[i];
            // Show the assumed polarity so `ASSUME NOT x` reads as `NOT x`.
            let label = if matches!(f.value, Value::False) {
                alloc::format!("NOT {}", label(c, f.atom))
            } else {
                label(c, f.atom)
            };
            CoreItem {
                origin: f.origin.clone(),
                label,
            }
        })
        .collect();
    core.sort_by_key(|it| key(&it.origin));
    core
}

// --- near-duplicate atom detection (advisory typo hints) -------------------

/// A removable source construct (one fact, one premise, or one rule) and the CNF
/// clauses it contributes — the unit of an unsat-core explanation.
pub(crate) struct Construct {
    origin: Origin,
    label: String,
    clauses: Vec<Vec<sat::SatLit>>,
}

/// Two origins refer to the same source construct.
pub(crate) fn same_origin(a: &Origin, b: &Origin) -> bool {
    a.source == b.source && a.line == b.line && a.premise == b.premise && a.kind == b.kind
}

/// Split the program into removable constructs. A premise that desugared into
/// several clauses (e.g. an `EXCLUSIVE` over n atoms) is grouped back into one
/// construct by origin, so the core blames whole premises, not clause shards.
pub(crate) fn constructs(c: &Compiled) -> Vec<Construct> {
    let mut out: Vec<Construct> = Vec::new();

    for f in &c.facts {
        out.push(Construct {
            origin: f.origin.clone(),
            label: label(c, f.atom),
            clauses: vec![vec![fact_lit(f)]],
        });
    }

    let mut premises: Vec<Construct> = Vec::new();
    for clause in &c.clauses {
        let lits: Vec<sat::SatLit> = clause.lits.iter().map(clause_lit).collect();
        match premises
            .iter_mut()
            .find(|k| same_origin(&k.origin, &clause.origin))
        {
            Some(k) => k.clauses.push(lits),
            None => premises.push(Construct {
                label: clause.origin.premise.clone().unwrap_or_default(),
                origin: clause.origin.clone(),
                clauses: vec![lits],
            }),
        }
    }
    out.extend(premises);

    for r in &c.rules {
        let clauses = r
            .consequent
            .iter()
            .map(|cons| rule_consequent_clause(r, cons))
            .collect();
        out.push(Construct {
            label: r.origin.premise.clone().unwrap_or_default(),
            origin: r.origin.clone(),
            clauses,
        });
    }
    out
}

/// Is the program satisfiable using only the constructs marked active?
pub(crate) fn subset_is_sat(num_vars: usize, all: &[Construct], active: &[bool]) -> bool {
    let mut cnf = sat::Cnf::new(num_vars);
    for (k, &keep) in all.iter().zip(active) {
        if keep {
            for cl in &k.clauses {
                cnf.add_clause(cl.clone());
            }
        }
    }
    sat::solve(&cnf).is_some()
}

/// A fast sufficient core via one assumption-solve: each construct gets a fresh
/// selector variable `s_k`, every clause becomes `(¬s_k ∨ clause)`, and we solve
/// asserting all selectors true. The SAT core (a subset of the selectors) names a
/// sufficient set of constructs in a single solve — versus O(n) deletion solves.
/// Returns an `active` mask over `all`.
pub(crate) fn candidate_via_assumptions(c: &Compiled, all: &[Construct]) -> Vec<bool> {
    let base = c.atoms.len();
    let mut cnf = sat::Cnf::new(base + all.len());
    let sel = |i: usize| (base + i) as sat::Var;
    for (i, k) in all.iter().enumerate() {
        let s_neg = sat::SatLit::negative(sel(i));
        for cl in &k.clauses {
            let mut lits = Vec::with_capacity(cl.len() + 1);
            lits.push(s_neg);
            lits.extend_from_slice(cl);
            cnf.add_clause(lits);
        }
    }
    let assumptions: Vec<sat::SatLit> = (0..all.len())
        .map(|i| sat::SatLit::positive(sel(i)))
        .collect();
    let mut active = vec![false; all.len()];
    match sat::solve_assuming(&cnf, &assumptions) {
        sat::Solved::Unsat(core) => {
            for lit in core {
                let v = lit.var() as usize;
                if v >= base {
                    active[v - base] = true;
                }
            }
        }
        // The caller only calls this when the system is UNSAT, so this is
        // unreachable; fall back to all-active so the deletion pass below is still
        // correct (just slower).
        sat::Solved::Sat(_) => active.iter_mut().for_each(|a| *a = true),
    }
    active
}

/// A 1-minimal unsat core. First an assumption-solve narrows the program to a
/// sufficient candidate ([`candidate_via_assumptions`]); then deletion-based
/// minimization over *that candidate only* drops each construct in turn — if the
/// rest is still unsatisfiable it was not needed — leaving an irreducible set
/// jointly to blame. Called only when the full system is UNSAT.
pub(crate) fn minimal_unsat_core(c: &Compiled) -> Vec<CoreItem> {
    let all = constructs(c);
    let mut active = candidate_via_assumptions(c, &all);
    for i in 0..all.len() {
        if active[i] {
            active[i] = false;
            if subset_is_sat(c.atoms.len(), &all, &active) {
                active[i] = true; // removing it restored SAT → it is part of the core
            }
        }
    }
    let mut core: Vec<CoreItem> = all
        .iter()
        .zip(&active)
        .filter(|&(_, &keep)| keep)
        .map(|(k, _)| CoreItem {
            origin: k.origin.clone(),
            label: k.label.clone(),
        })
        .collect();
    core.sort_by_key(|it| key(&it.origin));
    core
}

/// Sort key giving conflicts/warnings a stable, source-then-line order.
pub(crate) fn key(o: &Origin) -> (String, u32) {
    (o.source.clone(), o.line)
}
