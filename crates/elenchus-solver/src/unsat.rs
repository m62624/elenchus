//! The minimal-unsat-core search: which named constructs / facts are jointly
//! responsible for an unsatisfiable system, via SAT under assumptions.
use crate::cnf::{build_cnf, clause_lit, fact_lit, rule_consequent_clause};
use crate::report::{CoreItem, Fix, FixKind, Tried, TryOutcome, label};
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

    // One shared incremental solver answers every satisfiability question below.
    // All of them are verdict-only, so heuristics are on from the start.
    let mut cs = ConstructSolver::new(c.atoms.len(), &all);
    cs.enable_turbo();

    // The hard program (drop every soft construct) must be consistent on its own,
    // else the facts/premises are to blame and we must not point at assumptions.
    let hard_only: Vec<bool> = is_soft.iter().map(|&s| !s).collect();
    if !cs.subset_is_sat(&hard_only) {
        return Vec::new();
    }
    // The full program must actually be UNSAT for there to be anything to drop.
    let mut active = vec![true; all.len()];
    if cs.subset_is_sat(&active) {
        return Vec::new();
    }
    // Deletion-minimize over the soft constructs only; hard ones stay pinned.
    for i in 0..all.len() {
        if active[i] && is_soft[i] {
            active[i] = false;
            if cs.subset_is_sat(&active) {
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
            let fixes = fixes_for(c, &mut cs, &all, &active, i, &label);
            CoreItem {
                origin: f.origin.clone(),
                label,
                fixes,
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

/// A persistent selector-guarded solver over the program's constructs — the shared
/// engine of every minimization loop. Construct `k`'s clauses are loaded once as
/// `(¬s_k ∨ clause)`; a query *assumes* `s_k` for each active construct. A selector
/// that is not assumed is free, and since selectors occur only negatively, a free
/// selector lets the solver switch that construct off — so each query is
/// equisatisfiable with the formula containing exactly the active constructs.
///
/// Incremental on purpose: a deletion-minimization loop asks O(n) closely related
/// SAT/UNSAT questions over one formula; sharing the clause database lets learned
/// clauses answer later queries instead of being re-derived from scratch each time.
/// Only *verdicts* are consumed by the loops (semantically unique, so the reported
/// cores/fixes are byte-identical to the scratch-per-query implementation this
/// replaces); the one query whose *contents* feed the report — the initial core
/// candidate — is the solver's first, which on a fresh database is the exact same
/// computation as a standalone `solve_assuming`.
pub(crate) struct ConstructSolver {
    inc: sat::Incremental,
    /// Selector variables start here (== the program's atom count).
    base: usize,
}

impl ConstructSolver {
    /// Load every construct's clauses, guarded by one selector each.
    pub(crate) fn new(num_vars: usize, all: &[Construct]) -> Self {
        let mut cnf = sat::Cnf::new(num_vars + all.len());
        for (i, k) in all.iter().enumerate() {
            let s_neg = sat::SatLit::negative((num_vars + i) as sat::Var);
            for cl in &k.clauses {
                let mut lits = Vec::with_capacity(cl.len() + 1);
                lits.push(s_neg);
                lits.extend_from_slice(cl);
                cnf.add_clause(lits);
            }
        }
        ConstructSolver {
            inc: sat::Incremental::new(&cnf),
            base: num_vars,
        }
    }

    /// The selector literal enabling construct `i`.
    fn selector(&self, i: usize) -> sat::SatLit {
        sat::SatLit::positive((self.base + i) as sat::Var)
    }

    /// Turn search heuristics on for the queries that follow. Sound only once no
    /// remaining query's *contents* reach the report — i.e. after the core
    /// candidate (whose literals do) has been solved on the reference profile;
    /// the deletion/flip queries consume bare SAT/UNSAT verdicts, which
    /// heuristics cannot change.
    fn enable_turbo(&mut self) {
        self.inc.set_config(sat::SolverConfig::TURBO);
    }

    /// Is the program satisfiable using only the constructs marked active?
    pub(crate) fn subset_is_sat(&mut self, active: &[bool]) -> bool {
        let asm: Vec<sat::SatLit> = active
            .iter()
            .enumerate()
            .filter(|&(_, &a)| a)
            .map(|(i, _)| self.selector(i))
            .collect();
        matches!(self.inc.solve(&asm), sat::Solved::Sat(_))
    }

    /// Like [`ConstructSolver::subset_is_sat`], but construct `i` is replaced by
    /// asserting the single literal `flipped` (its selector stays free = off).
    fn flip_is_sat(&mut self, active: &[bool], i: usize, flipped: sat::SatLit) -> bool {
        let mut asm: Vec<sat::SatLit> = active
            .iter()
            .enumerate()
            .filter(|&(k, &a)| a && k != i)
            .map(|(k, _)| self.selector(k))
            .collect();
        asm.push(flipped);
        matches!(self.inc.solve(&asm), sat::Solved::Sat(_))
    }
}

/// Would *flipping* construct `i` (a single-unit `FACT`/`ASSUME`) — keeping every
/// currently-active construct as-is, but asserting the opposite value for `i` —
/// restore satisfiability? Only unit constructs can be flipped; a premise/rule has
/// no single polarity to reverse, so this returns `false` for them (their only fix
/// is `Drop`). Tested against the same `active` mask the drop-minimization used, so
/// the flip advice shares the drop advice's frame of reference.
pub(crate) fn flip_restores_sat(
    cs: &mut ConstructSolver,
    all: &[Construct],
    active: &[bool],
    i: usize,
) -> bool {
    // A flippable construct is exactly one unit clause holding one literal.
    if all[i].clauses.len() != 1 || all[i].clauses[0].len() != 1 {
        return false;
    }
    cs.flip_is_sat(active, i, all[i].clauses[0][0].negate())
}

/// The engine-verified repairs for a retained construct `i`: always `Drop` (its
/// removal restores SAT — that is what put it in the minimal set), plus `Flip` **only
/// when** re-solving with the fact flipped is actually consistent. `Flip` is offered
/// only for a `FACT`/`ASSUME` (`i < c.facts.len()`, the 1:1 prefix in [`constructs`]);
/// its target is the literal the flip would assert (opposite of the fact's value).
pub(crate) fn fixes_for(
    c: &Compiled,
    cs: &mut ConstructSolver,
    all: &[Construct],
    active: &[bool],
    i: usize,
    drop_target: &str,
) -> Vec<Fix> {
    let mut fixes = vec![Fix {
        kind: FixKind::Drop,
        target: String::from(drop_target),
    }];
    if i < c.facts.len() && flip_restores_sat(cs, all, active, i) {
        let f = &c.facts[i];
        // The flip asserts the opposite of the fact's current value.
        let target = if matches!(f.value, Value::True) {
            alloc::format!("NOT {}", label(c, f.atom))
        } else {
            label(c, f.atom)
        };
        fixes.push(Fix {
            kind: FixKind::Flip,
            target,
        });
    }
    fixes
}

/// A fast sufficient core via one assumption-solve: solve asserting every selector
/// true; the SAT core (a subset of the selectors) names a sufficient set of
/// constructs in a single solve — versus O(n) deletion solves. Returns an `active`
/// mask over the constructs.
///
/// Must be the **first** query on `cs`: on a fresh clause database this is the
/// exact same computation as a standalone `solve_assuming` over the same CNF, so
/// the returned candidate — whose contents shape the reported core — is identical
/// to the pre-incremental implementation's.
pub(crate) fn candidate_via_assumptions(cs: &mut ConstructSolver, count: usize) -> Vec<bool> {
    let assumptions: Vec<sat::SatLit> = (0..count).map(|i| cs.selector(i)).collect();
    let base = cs.base;
    let mut active = vec![false; count];
    match cs.inc.solve(&assumptions) {
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
    // One incremental solver serves the candidate solve, the deletion loop, and
    // the flip checks; the candidate must come first (see its docs).
    let mut cs = ConstructSolver::new(c.atoms.len(), &all);
    let mut active = candidate_via_assumptions(&mut cs, all.len());
    // The content-bearing candidate query is done; everything after is verdict-only.
    cs.enable_turbo();
    for i in 0..all.len() {
        if active[i] {
            active[i] = false;
            if cs.subset_is_sat(&active) {
                active[i] = true; // removing it restored SAT → it is part of the core
            }
        }
    }
    let mut core: Vec<CoreItem> = all
        .iter()
        .enumerate()
        .filter(|&(i, _)| active[i])
        .map(|(i, k)| {
            let fixes = fixes_for(c, &mut cs, &all, &active, i, &k.label);
            CoreItem {
                origin: k.origin.clone(),
                label: k.label.clone(),
                fixes,
            }
        })
        .collect();
    core.sort_by_key(|it| key(&it.origin));
    core
}

/// Sort key giving conflicts/warnings a stable, source-then-line order.
pub(crate) fn key(o: &Origin) -> (String, u32) {
    (o.source.clone(), o.line)
}

/// The abduction (L5) side-check: for each `TRY <literal>` hypothesis, judge whether
/// asserting the supplied candidate would resolve the program's open model. Purely
/// advisory — the candidate is **never committed**; each verdict is one bounded
/// side-solve (the program CNF plus the single candidate literal), so cost grows with
/// the number of `TRY` lines, never with any search the engine invents (Law 5).
///
/// The base program's model multiplicity (counted up to two) sets the baseline; adding
/// the candidate either drops it to a unique model ([`TryOutcome::Closes`] — it pins the
/// gap), makes the program unsatisfiable ([`TryOutcome::Conflicts`] — it clashes with
/// what is established), or leaves more than one model ([`TryOutcome::StillOpen`] — it
/// does not pin it). On an already-unsatisfiable program every hypothesis reads as
/// `Conflicts` (adding a clause never clears a conflict — that is L4's job).
pub(crate) fn tried_hypotheses(c: &Compiled) -> Vec<Tried> {
    if c.hypotheses.is_empty() {
        return Vec::new();
    }
    let (mut cnf, project) = build_cnf(c);
    // One incremental solver counts models for the base program AND every
    // hypothesis, instead of cloning + re-solving the CNF per hypothesis. Each
    // counting session gets its own **guard variable**: its blocking clause is
    // `(¬guard ∨ ¬model-projection)`, active only while that session assumes its
    // guard — so blocked models never leak into another hypothesis's count. Counts
    // (up to 2, distinct on `project`) are semantic — independent of enumeration
    // order — so the reported outcomes are identical to the clone-per-hypothesis
    // implementation this replaces.
    let guard_base = cnf.num_vars;
    cnf.num_vars += 1 + c.hypotheses.len();
    // Counts are heuristic-invariant, so the turbo profile is sound throughout.
    let mut inc = sat::Incremental::with_config(&cnf, sat::SolverConfig::TURBO);
    // Count the models of (program ∧ assumptions) projected on `project`, up to 2.
    let count2 = |inc: &mut sat::Incremental, assume: &[sat::SatLit], guard: sat::Var| match inc
        .solve(assume)
    {
        sat::Solved::Unsat(_) => 0,
        sat::Solved::Sat(model) => {
            let mut block = Vec::with_capacity(project.len() + 1);
            block.push(sat::SatLit::negative(guard));
            block.extend(project.iter().map(|&v| {
                if model[v as usize] {
                    sat::SatLit::negative(v)
                } else {
                    sat::SatLit::positive(v)
                }
            }));
            inc.add_clause(&block);
            let mut asm = Vec::with_capacity(assume.len() + 1);
            asm.extend_from_slice(assume);
            asm.push(sat::SatLit::positive(guard));
            match inc.solve(&asm) {
                sat::Solved::Sat(_) => 2,
                sat::Solved::Unsat(_) => 1,
            }
        }
    };
    // A single base model over the constrained atoms means the program is already
    // pinned; two means it is open (the same measure the backward pass uses).
    let base_unique = count2(&mut inc, &[], guard_base as sat::Var) == 1;
    c.hypotheses
        .iter()
        .enumerate()
        .map(|(i, h)| {
            // Assume the candidate literal (positive unless written `TRY NOT …`).
            let lit = sat::SatLit::new(h.lit.atom, !h.lit.negated);
            let guard = (guard_base + 1 + i) as sat::Var;
            let outcome = match count2(&mut inc, &[lit], guard) {
                0 => TryOutcome::Conflicts,
                1 if !base_unique => TryOutcome::Closes,
                _ => TryOutcome::StillOpen,
            };
            let name = label(c, h.lit.atom);
            let text = if h.lit.negated {
                alloc::format!("NOT {name}")
            } else {
                name
            };
            Tried {
                origin: h.origin.clone(),
                label: text,
                outcome,
            }
        })
        .collect()
}
