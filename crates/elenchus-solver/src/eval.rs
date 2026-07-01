//! The forward pass: seed facts, forward-chain rules to a fixpoint, evaluate
//! every `Impossible` clause, and collect the conflicts / warnings / derived facts.
use crate::cnf::build_cnf;
use crate::report::CoreItem;
use crate::report::{
    Conflict, Defeated, Derived, Report, Status, TraceReason, TraceStep, Warning, label,
};
use crate::sat;
use crate::unsat::{key, minimal_unsat_core};
use crate::v3::{V3, v3_to_value};
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use elenchus_compiler::{AtomId, Clause, Compiled, KIND_UNSAT, Lit, Origin, Value, kw};

/// The three-valued value of a literal: the atom's value, flipped if negated.
pub(crate) fn lit_value(model: &[V3], l: &Lit) -> V3 {
    let v = model[l.atom as usize];
    if l.negated { v.not() } else { v }
}

/// Kleene AND over a conjunction of literals (a rule antecedent / clause prefix).
pub(crate) fn conjunction(model: &[V3], lits: &[Lit]) -> V3 {
    let mut result = V3::True;
    for l in lits {
        match lit_value(model, l) {
            V3::False => return V3::False,
            V3::Unknown => result = V3::Unknown,
            V3::True => {}
        }
    }
    result
}

/// The status of one `Impossible` clause under the current model.
pub(crate) enum ClauseEval {
    /// Every literal is forced TRUE → the constraint is violated.
    Violated,
    /// Some literal is FALSE → the literals cannot all hold → satisfied.
    Satisfied,
    /// No FALSE literal, but an UNKNOWN remains: the check is blocked on these atoms.
    Blocked(Vec<AtomId>),
}

/// Classify one `Impossible` clause under the model: all-true is a violation,
/// any-false satisfies it, otherwise it is blocked on the remaining UNKNOWNs.
pub(crate) fn eval_clause(model: &[V3], clause: &Clause) -> ClauseEval {
    let mut any_false = false;
    let mut all_true = true;
    let mut blocked = Vec::new();
    for l in &clause.lits {
        match lit_value(model, l) {
            V3::False => {
                any_false = true;
                all_true = false;
            }
            V3::Unknown => {
                all_true = false;
                blocked.push(l.atom);
            }
            V3::True => {}
        }
    }
    if all_true {
        ClauseEval::Violated
    } else if any_false {
        ClauseEval::Satisfied
    } else {
        ClauseEval::Blocked(blocked)
    }
}

/// Why an atom holds its confident value — the forward pass records this so a
/// conflict can be traced back to the facts and rules that forced it.
#[derive(Clone)]
pub(crate) enum AtomReason {
    /// Set directly by a `FACT` / `NOT`.
    Asserted(Origin),
    /// Derived by a firing `RULE` from the listed antecedent atoms.
    Derived { origin: Origin, from: Vec<AtomId> },
}

/// An internal conflict before labels and trace are materialized. `atoms` are the
/// exact display strings; `cause` are the atoms whose forcing chains explain it
/// (empty when there is no chain to show, e.g. a direct fact contradiction).
pub(crate) struct RawConflict {
    origin: Origin,
    atoms: Vec<String>,
    cause: Vec<AtomId>,
}

/// Working state of the forward + backward evaluation, evaluated as a pipeline.
pub(crate) struct Eval<'a> {
    c: &'a Compiled,
    model: Vec<V3>,
    /// Per-atom provenance, filled by `seed_facts` and `saturate_rules`; read by
    /// `build_trace` once the model is final.
    reason: Vec<Option<AtomReason>>,
    conflicts: Vec<RawConflict>,
    warnings: Vec<Warning>,
    derived: Vec<Derived>,
    /// Informational notes: a defeasible `RULE` whose default was suppressed by an
    /// established `UNLESS` exception. Never affects the verdict.
    defeated: Vec<Defeated>,
    /// Minimal set of constructs to blame when the backward pass finds UNSAT.
    unsat_core: Vec<CoreItem>,
}

impl<'a> Eval<'a> {
    pub(crate) fn new(c: &'a Compiled) -> Self {
        Eval {
            c,
            model: vec![V3::Unknown; c.atoms.len()],
            reason: vec![None; c.atoms.len()],
            conflicts: Vec::new(),
            warnings: Vec::new(),
            derived: Vec::new(),
            defeated: Vec::new(),
            unsat_core: Vec::new(),
        }
    }

    pub(crate) fn label(&self, a: AtomId) -> String {
        label(self.c, a)
    }

    /// 1. Seed the model from confident facts; `FACT X` + `NOT X` is a CONFLICT.
    pub(crate) fn seed_facts(&mut self) {
        let c = self.c;
        let n = c.atoms.len();
        let mut t: Vec<Option<Origin>> = vec![None; n];
        let mut f: Vec<Option<Origin>> = vec![None; n];
        for fact in &c.facts {
            let slot = match fact.value {
                Value::True => &mut t,
                Value::False => &mut f,
            };
            if slot[fact.atom as usize].is_none() {
                slot[fact.atom as usize] = Some(fact.origin.clone());
            }
        }
        for a in 0..n {
            match (&t[a], &f[a]) {
                (Some(o), Some(_)) => {
                    self.model[a] = V3::True;
                    self.reason[a] = Some(AtomReason::Asserted(o.clone()));
                    self.conflicts.push(RawConflict {
                        origin: o.clone(),
                        atoms: vec![alloc::format!(
                            "{} (asserted both TRUE and FALSE)",
                            self.label(a as AtomId)
                        )],
                        cause: Vec::new(),
                    });
                }
                (Some(o), None) => {
                    self.model[a] = V3::True;
                    self.reason[a] = Some(AtomReason::Asserted(o.clone()));
                }
                (None, Some(o)) => {
                    self.model[a] = V3::False;
                    self.reason[a] = Some(AtomReason::Asserted(o.clone()));
                }
                (None, None) => {}
            }
        }
    }

    /// 2. Forward-chain RULEs to a fixpoint, deriving facts (Kleene antecedent).
    pub(crate) fn saturate_rules(&mut self) {
        let c = self.c;
        loop {
            let mut changed = false;
            for r in &c.rules {
                if conjunction(&self.model, &r.antecedent) != V3::True {
                    continue; // rule does not fire (FALSE, or blocked by UNKNOWN)
                }
                // A defeasible RULE is suppressed when any UNLESS exception is
                // *established* TRUE. FALSE or UNKNOWN exceptions do not defeat it
                // (assume-normal) — only a settled exception retracts the default.
                if r.exceptions
                    .iter()
                    .any(|ex| lit_value(&self.model, ex) == V3::True)
                {
                    continue;
                }
                for cl in &r.consequent {
                    let target = if cl.negated { V3::False } else { V3::True };
                    match self.model[cl.atom as usize] {
                        V3::Unknown => {
                            self.model[cl.atom as usize] = target;
                            self.reason[cl.atom as usize] = Some(AtomReason::Derived {
                                origin: r.origin.clone(),
                                from: r.antecedent.iter().map(|l| l.atom).collect(),
                            });
                            changed = true;
                            self.derived.push(Derived {
                                atom: self.label(cl.atom),
                                value: if cl.negated {
                                    Value::False
                                } else {
                                    Value::True
                                },
                                origin: r.origin.clone(),
                            });
                        }
                        v if v == target => {}
                        _ => {
                            // The rule wants the opposite of a value the atom already
                            // holds. Trace both sides: why the antecedent fired (its
                            // atoms) and how the atom got its existing value.
                            let mut cause: Vec<AtomId> =
                                r.antecedent.iter().map(|l| l.atom).collect();
                            cause.push(cl.atom);
                            self.conflicts.push(RawConflict {
                                origin: r.origin.clone(),
                                atoms: vec![alloc::format!(
                                    "{} (derived value contradicts a known fact)",
                                    self.label(cl.atom)
                                )],
                                cause,
                            });
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Record each defeasible `RULE` whose default was suppressed by an established
    /// exception: its antecedent holds, yet an `UNLESS` literal is TRUE, so it derived
    /// nothing. Read from the *settled* model (run after [`Eval::saturate_rules`]), so
    /// each defeat is noted once. Purely informational — it never changes the verdict
    /// (a defeated default is not a conflict), mirroring `derived`.
    pub(crate) fn flag_defeated_defaults(&mut self) {
        let c = self.c;
        for r in &c.rules {
            if r.exceptions.is_empty() || conjunction(&self.model, &r.antecedent) != V3::True {
                continue;
            }
            let blocked_by: Vec<String> = r
                .exceptions
                .iter()
                .filter(|ex| lit_value(&self.model, ex) == V3::True)
                .map(|ex| self.label(ex.atom))
                .collect();
            if blocked_by.is_empty() {
                continue; // fired normally, nothing suppressed
            }
            let consequent = r
                .consequent
                .iter()
                .map(|cl| self.label(cl.atom))
                .collect::<Vec<_>>()
                .join(", ");
            self.defeated.push(Defeated {
                origin: r.origin.clone(),
                consequent,
                blocked_by,
            });
        }
    }

    /// 3. Evaluate every `Impossible` clause against the model.
    pub(crate) fn check_premises(&mut self) {
        let c = self.c;
        // Atoms some RULE can derive (appear in a rule's consequent). The pivot for
        // the warning hint: a blocked atom in this set is a derivation waiting on
        // its rule's antecedent; one *not* in it can only be set by a FACT/NOT — or
        // by turning a PREMISE that means to establish it into a RULE.
        let derivable: BTreeSet<AtomId> = c
            .rules
            .iter()
            .flat_map(|r| r.consequent.iter().map(|l| l.atom))
            .collect();
        for clause in &c.clauses {
            match eval_clause(&self.model, clause) {
                ClauseEval::Violated => self.conflicts.push(RawConflict {
                    origin: clause.origin.clone(),
                    atoms: clause.lits.iter().map(|l| self.label(l.atom)).collect(),
                    cause: clause.lits.iter().map(|l| l.atom).collect(),
                }),
                ClauseEval::Satisfied => {}
                // Implication premises warn on missing data; list premises treat
                // UNKNOWN as "no conflict yet" and stay consistent.
                ClauseEval::Blocked(unknowns) if clause.origin.kind == kw::PREMISE => {
                    let hint = self.warning_hint(&unknowns, &derivable);
                    self.warnings.push(Warning {
                        origin: clause.origin.clone(),
                        blocked_by: unknowns.iter().map(|a| self.label(*a)).collect(),
                        hint,
                    });
                }
                ClauseEval::Blocked(_) => {}
            }
        }
    }

    /// Pick the most informative blocked atom and phrase a directed fix for it.
    /// Prefers a *free input* (nothing derives it) — the common "I used a PREMISE
    /// where I needed a RULE / forgot a FACT" trap — over an atom a RULE could
    /// still derive.
    pub(crate) fn warning_hint(
        &self,
        unknowns: &[AtomId],
        derivable: &BTreeSet<AtomId>,
    ) -> Option<String> {
        let free = unknowns.iter().find(|a| !derivable.contains(a));
        match free {
            Some(&a) => Some(alloc::format!(
                "nothing determines `{}` — add `FACT {}` (or `NOT …`), or if a PREMISE's \
                 THEN is meant to establish it, make that PREMISE a RULE so it derives the value",
                self.label(a),
                self.label(a),
            )),
            None => unknowns.first().map(|&a| {
                alloc::format!(
                    "`{}` is derived by a RULE that has not fired — assert that rule's antecedent",
                    self.label(a),
                )
            }),
        }
    }

    /// Build the derivation chain that explains why the `causes` atoms hold their
    /// current values: a post-order walk of the reason graph so each atom's
    /// supports appear before it (facts first, the conflict atoms last), with
    /// every atom emitted once. Atoms with no recorded reason (UNKNOWN) are
    /// skipped — a forced atom always has one. `visited` is a caller-owned scratch
    /// buffer (reset to all-false on entry) so [`Eval::finish`] can reuse one
    /// allocation across every conflict's trace instead of allocating fresh per call.
    pub(crate) fn build_trace(&self, causes: &[AtomId], visited: &mut [bool]) -> Vec<TraceStep> {
        visited.fill(false);
        let mut out = Vec::new();
        for &a in causes {
            self.trace_dfs(a, visited, &mut out);
        }
        out
    }

    pub(crate) fn trace_dfs(&self, a: AtomId, visited: &mut [bool], out: &mut Vec<TraceStep>) {
        if visited[a as usize] {
            return;
        }
        visited[a as usize] = true;
        let value = match v3_to_value(self.model[a as usize]) {
            Some(v) => v,
            None => return, // UNKNOWN: nothing forced it, nothing to explain
        };
        let reason = match &self.reason[a as usize] {
            Some(AtomReason::Asserted(o)) => TraceReason::Asserted(o.clone()),
            Some(AtomReason::Derived { origin, from }) => {
                for &f in from {
                    self.trace_dfs(f, visited, out); // supports first
                }
                TraceReason::Derived {
                    origin: origin.clone(),
                    from: from.iter().map(|&f| self.label(f)).collect(),
                }
            }
            None => return,
        };
        out.push(TraceStep {
            atom: self.label(a),
            value,
            reason,
        });
    }

    /// Backward pass (model finding), run only when a CHECK requests BIDIRECTIONAL.
    /// Encodes premises + rules + facts as CNF and asks the SAT core for models.
    /// No model means the system is jointly unsatisfiable (a CONFLICT the forward
    /// pass may have missed). Two or more models means an alternative exists; we
    /// return the UNDERDETERMINED witness — the first constrained atom the two
    /// models disagree on.
    pub(crate) fn backward_pass(&mut self) -> Option<String> {
        if !self.c.checks.iter().any(|ch| ch.bidirectional) {
            return None;
        }
        let (cnf, project) = build_cnf(self.c);
        let found = sat::models(&cnf, &project, 2);
        match found.len() {
            0 if self.conflicts.is_empty() => {
                self.unsat_core = minimal_unsat_core(self.c);
                self.conflicts.push(RawConflict {
                    origin: Origin {
                        source: String::from("<system>"),
                        line: 0,
                        premise: None,
                        kind: KIND_UNSAT,
                    },
                    atoms: vec![String::from(
                        "the premises and facts are jointly unsatisfiable",
                    )],
                    cause: Vec::new(),
                });
                None
            }
            n if n >= 2 => {
                let (m0, m1) = (&found[0], &found[1]);
                project
                    .iter()
                    .find(|&&v| m0[v as usize] != m1[v as usize])
                    .map(|&v| self.label(v))
                    .or_else(|| Some(String::from("a free atom")))
            }
            _ => None,
        }
    }

    /// Turn each unwitnessed `EXISTS` (an `ExistsDomain::Open` the compiler flagged)
    /// into a WARNING: the existential named no candidate, so it could not be
    /// checked. Must run *before* [`Eval::finish`] computes the verdict, so it can
    /// raise CONSISTENT → WARNING — a premise that could not be checked, exactly
    /// like an implication blocked by an UNKNOWN atom.
    pub(crate) fn flag_unwitnessed_exists(&mut self) {
        for u in &self.c.unwitnessed_exists {
            self.warnings.push(Warning {
                origin: u.origin.clone(),
                blocked_by: alloc::vec![u.condition.clone()],
                hint: Some(alloc::format!(
                    "name a witness: EXISTS {b} WITNESS <term>  (or a set: EXISTS {b} IN <set>)",
                    b = u.binder
                )),
            });
        }
    }

    /// Check each `FACT … BECAUSE <ground>` justification against the settled forward
    /// model — the L2 "how do you know?" layer. The ground's value decides the
    /// verdict: FALSE means the stated reason does not hold (**CONFLICT**), UNKNOWN
    /// means it is unestablished (**WARNING**), TRUE means the justification holds
    /// (silent). `BECAUSE` emits no clause (the check is evaluative, so an UNKNOWN
    /// ground is *reported* rather than forced true), hence this must run *before*
    /// [`Eval::finish`] so it can raise the verdict — like [`Eval::flag_unwitnessed_exists`].
    pub(crate) fn check_justifications(&mut self) {
        let c = self.c;
        for j in &c.justifications {
            match self.model[j.ground as usize] {
                V3::True => {}
                V3::False => self.conflicts.push(RawConflict {
                    origin: j.origin.clone(),
                    atoms: vec![alloc::format!(
                        "{} — its stated ground {} is FALSE",
                        self.label(j.belief),
                        self.label(j.ground)
                    )],
                    // Explain *why* the ground is false (its asserting FACT/NOT/rule).
                    cause: vec![j.ground],
                }),
                V3::Unknown => self.warnings.push(Warning {
                    origin: j.origin.clone(),
                    blocked_by: vec![self.label(j.ground)],
                    hint: Some(alloc::format!(
                        "establish the ground: FACT {g}  (or derive it with a RULE)",
                        g = self.label(j.ground)
                    )),
                }),
            }
        }
    }

    /// Run the backward pass, sort deterministically, and assemble the report.
    pub(crate) fn finish(mut self) -> Report {
        let underdetermined = self.backward_pass();
        self.conflicts.sort_by_key(|c| key(&c.origin));
        self.warnings.sort_by_key(|w| key(&w.origin));
        self.defeated.sort_by_key(|d| key(&d.origin));
        let status = if !self.conflicts.is_empty() {
            Status::Conflict
        } else if underdetermined.is_some() {
            Status::Underdetermined
        } else if !self.warnings.is_empty() {
            Status::Warning
        } else {
            Status::Consistent
        };
        // Materialize each raw conflict into its public form, attaching the
        // derivation chain (reasons are final once the forward pass is done).
        // One scratch buffer, reused (reset) across every conflict's trace.
        let mut visited = vec![false; self.c.atoms.len()];
        let conflicts: Vec<Conflict> = self
            .conflicts
            .iter()
            .map(|rc| Conflict {
                origin: rc.origin.clone(),
                atoms: rc.atoms.clone(),
                trace: self.build_trace(&rc.cause, &mut visited),
            })
            .collect();
        Report {
            status,
            conflicts,
            warnings: self.warnings,
            derived: self.derived,
            defeated: self.defeated,
            underdetermined,
            unsat_core: self.unsat_core,
            retract: Vec::new(), // filled by `solve` when assumptions are to blame
            hints: Vec::new(),   // filled by `solve` (advisory, post-verdict)
            orphans: Vec::new(), // filled by `solve` (advisory, post-verdict)
            unused_imports: Vec::new(), // copied from the IR by `solve` (advisory)
            placeholders: Vec::new(), // copied from the IR by `solve` (advisory)
        }
    }
}
