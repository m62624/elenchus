//! elenchus-solver — the inference interpreter (forward pass).
//!
//! Consumes the [`Compiled`] IR from `elenchus-compiler` and evaluates it under
//! three-valued Kleene logic (TRUE / FALSE / UNKNOWN, where UNKNOWN ≠ FALSE):
//!
//! 1. seed a model from confident `FACT`/`NOT` facts (and report `FACT X` + `NOT X`);
//! 2. forward-chain `RULE`s to a fixpoint, deriving facts (a derived value that
//!    contradicts an existing one is a CONFLICT);
//! 3. evaluate every `Impossible` clause (the desugared `PREMISE`s):
//!    - all literals forced TRUE → **CONFLICT** (the constraint is violated);
//!    - some literal FALSE → satisfied → CONSISTENT;
//!    - otherwise (no FALSE, an UNKNOWN remains): for implication premises this is a
//!      **WARNING** (blocked by missing data), for list premises (EXCLUSIVE/FORBIDS/
//!      ONEOF/ATLEAST) it is CONSISTENT (UNKNOWN means "no conflict yet").
//!
//! On `CHECK ... BIDIRECTIONAL` a **backward pass** also runs: the premises, rules
//! and confident facts are encoded as CNF and handed to a small in-crate CDCL SAT
//! core ([`sat`], replicating varisat's algorithm) to count models — 0 means the
//! system is jointly unsatisfiable (a CONFLICT the forward pass may miss), ≥2
//! means an alternative model exists (`UNDERDETERMINED`).
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod sat;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use elenchus_compiler::{AtomId, Clause, Compiled, Lit, Origin, Value};
pub use elenchus_compiler::{CompileError, MemoryResolver, Resolver, compile, compile_source};

/// Three-valued truth (Kleene). UNKNOWN is a first-class value, not hidden FALSE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V3 {
    /// Known true.
    True,
    /// Known false.
    False,
    /// Not asserted and not derivable — the absence of information, not falsity.
    Unknown,
}

impl V3 {
    /// Kleene negation: TRUE↔FALSE, and UNKNOWN stays UNKNOWN.
    fn not(self) -> V3 {
        match self {
            V3::True => V3::False,
            V3::False => V3::True,
            V3::Unknown => V3::Unknown,
        }
    }
}

/// Overall verdict for the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No contradictions, and (when checked) the model is pinned down.
    Consistent,
    /// The constraints are satisfiable but do not pin a unique assignment — an
    /// alternative model exists (found by the backward pass on `BIDIRECTIONAL`).
    Underdetermined,
    /// A premise could not be checked because a needed atom is UNKNOWN.
    Warning,
    /// A premise is violated, or the premises + facts are jointly unsatisfiable.
    Conflict,
}

/// A violated constraint (or a fact-level contradiction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Provenance of the violated constraint (source, line, premise name, kind).
    pub origin: Origin,
    /// Human labels of the atoms participating in the contradiction.
    pub atoms: Vec<String>,
    /// The derivation chain that forced the participating atoms to the values
    /// which made the constraint fire — supporting facts first, then each rule
    /// built on them, ending at the conflict. This is the answer to "CONFLICT,
    /// but *why*?". Empty for a direct `FACT X` + `NOT X` contradiction and for
    /// the `<system>` joint-unsatisfiability conflict (neither has a chain).
    pub trace: Vec<TraceStep>,
}

/// One link in a [`Conflict`]'s derivation chain: an atom, the value it was
/// forced to, and why it holds that value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStep {
    /// Human label of the atom (`subject predicate [object]`).
    pub atom: String,
    /// The confident value the atom was forced to (TRUE or FALSE).
    pub value: Value,
    /// Why the atom holds that value.
    pub reason: TraceReason,
}

/// Why a [`TraceStep`] atom holds its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceReason {
    /// Asserted directly by a `FACT` / `NOT`.
    Asserted(Origin),
    /// Derived by a `RULE` whose antecedent atoms all held.
    Derived {
        /// Provenance of the firing rule.
        origin: Origin,
        /// Human labels of the antecedent atoms that supported the derivation.
        from: Vec<String>,
    },
}

/// A constraint that could not be checked because a needed atom is UNKNOWN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Provenance of the constraint that could not be checked.
    pub origin: Origin,
    /// Human labels of the UNKNOWN atoms blocking the check.
    pub blocked_by: Vec<String>,
}

/// A fact produced by a `RULE` during forward chaining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    /// Human label of the atom whose value was derived.
    pub atom: String,
    /// The value the rule assigned (TRUE, or FALSE for a `THEN NOT …`).
    pub value: Value,
    /// Provenance of the `RULE` that produced it.
    pub origin: Origin,
}

/// The result of solving, self-contained (atom ids already resolved to labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The overall verdict.
    pub status: Status,
    /// Every violated constraint / fact contradiction (sorted by source+line).
    pub conflicts: Vec<Conflict>,
    /// Every premise blocked by an UNKNOWN atom (sorted by source+line).
    pub warnings: Vec<Warning>,
    /// Facts produced by forward-chaining `RULE`s.
    pub derived: Vec<Derived>,
    /// When `UNDERDETERMINED`, the label of an atom left free by the constraints
    /// (asserting it would pin the model down).
    pub underdetermined: Option<String>,
    /// When the system is jointly unsatisfiable but the forward pass found no
    /// single violated constraint, the minimal set of constructs (facts /
    /// premises / rules) whose removal restores satisfiability — i.e. the
    /// smallest group jointly to blame. Empty in every other case.
    pub unsat_core: Vec<CoreItem>,
}

/// One construct named in an [`Report::unsat_core`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreItem {
    /// Provenance of the construct (source, line, kind, premise name if any).
    pub origin: Origin,
    /// A human label: the premise/rule name, or the atom for a bare `FACT`/`NOT`.
    pub label: String,
}

impl Report {
    /// CLI-style exit code: 0 = consistent, 1 = underdetermined/warnings, 2 = conflicts.
    pub fn exit_code(&self) -> i32 {
        match self.status {
            Status::Conflict => 2,
            Status::Underdetermined | Status::Warning => 1,
            Status::Consistent => 0,
        }
    }

    /// Render the report as a single-line JSON object (for tooling / MCP).
    /// Hand-written so the crate stays dependency-free and `no_std`.
    pub fn to_json(&self) -> String {
        use core::fmt::Write as _;
        let mut s = String::new();
        let _ = write!(s, "{{\"status\":");
        json_str(status_name(self.status), &mut s);
        let _ = write!(s, ",\"exit_code\":{}", self.exit_code());

        s.push_str(",\"conflicts\":[");
        for (i, c) in self.conflicts.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&c.origin, &mut s);
            s.push_str(",\"atoms\":");
            json_array(&c.atoms, &mut s);
            s.push_str(",\"trace\":[");
            for (j, step) in c.trace.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                json_trace_step(step, &mut s);
            }
            s.push_str("]}");
        }
        s.push_str("],\"warnings\":[");
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&w.origin, &mut s);
            s.push_str(",\"blocked_by\":");
            json_array(&w.blocked_by, &mut s);
            s.push('}');
        }
        s.push_str("],\"derived\":[");
        for (i, d) in self.derived.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            json_origin_fields(&d.origin, &mut s);
            s.push_str(",\"atom\":");
            json_str(&d.atom, &mut s);
            let _ = write!(s, ",\"value\":{}", matches!(d.value, Value::True));
            s.push('}');
        }
        s.push_str("],\"underdetermined\":");
        match &self.underdetermined {
            Some(atom) => json_str(atom, &mut s),
            None => s.push_str("null"),
        }
        s.push_str(",\"unsat_core\":[");
        for (i, it) in self.unsat_core.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            json_str(&it.label, &mut s);
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

/// Push one derivation-trace step as a JSON object.
fn json_trace_step(step: &TraceStep, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("{\"atom\":");
    json_str(&step.atom, out);
    let _ = write!(out, ",\"value\":{}", matches!(step.value, Value::True));
    match &step.reason {
        TraceReason::Asserted(o) => {
            out.push_str(",\"how\":\"asserted\",");
            json_origin_fields(o, out);
            out.push_str(",\"from\":[]");
        }
        TraceReason::Derived { origin, from } => {
            out.push_str(",\"how\":\"derived\",");
            json_origin_fields(origin, out);
            out.push_str(",\"from\":");
            json_array(from, out);
        }
    }
    out.push('}');
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Consistent => "CONSISTENT",
        Status::Underdetermined => "UNDERDETERMINED",
        Status::Warning => "WARNING",
        Status::Conflict => "CONFLICT",
    }
}

/// Push a JSON-escaped string literal (including the surrounding quotes).
fn json_str(value: &str, out: &mut String) {
    use core::fmt::Write as _;
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Push a JSON array of escaped strings.
fn json_array(items: &[String], out: &mut String) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_str(item, out);
    }
    out.push(']');
}

/// `"premise":..,"kind":..,"source":..,"line":..` (no braces).
fn json_origin_fields(o: &Origin, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("\"premise\":");
    match &o.premise {
        Some(name) => json_str(name, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"kind\":");
    json_str(o.kind, out);
    out.push_str(",\"source\":");
    json_str(&o.source, out);
    let _ = write!(out, ",\"line\":{}", o.line);
}

/// Open an object `{` and write the origin fields.
fn json_origin(o: &Origin, out: &mut String) {
    out.push('{');
    json_origin_fields(o, out);
}

/// Render atom `a` as the human string `subject predicate [object]`.
fn label(c: &Compiled, a: AtomId) -> String {
    let k = &c.atoms[a as usize];
    match &k.object {
        Some(o) => alloc::format!("{} {} {}", k.subject, k.predicate, o),
        None => alloc::format!("{} {}", k.subject, k.predicate),
    }
}

/// The three-valued value of a literal: the atom's value, flipped if negated.
fn lit_value(model: &[V3], l: &Lit) -> V3 {
    let v = model[l.atom as usize];
    if l.negated { v.not() } else { v }
}

/// Kleene AND over a conjunction of literals (a rule antecedent / clause prefix).
fn conjunction(model: &[V3], lits: &[Lit]) -> V3 {
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
enum ClauseEval {
    /// Every literal is forced TRUE → the constraint is violated.
    Violated,
    /// Some literal is FALSE → the literals cannot all hold → satisfied.
    Satisfied,
    /// No FALSE literal, but an UNKNOWN remains: the check is blocked on these atoms.
    Blocked(Vec<AtomId>),
}

/// Classify one `Impossible` clause under the model: all-true is a violation,
/// any-false satisfies it, otherwise it is blocked on the remaining UNKNOWNs.
fn eval_clause(model: &[V3], clause: &Clause) -> ClauseEval {
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
enum AtomReason {
    /// Set directly by a `FACT` / `NOT`.
    Asserted(Origin),
    /// Derived by a firing `RULE` from the listed antecedent atoms.
    Derived { origin: Origin, from: Vec<AtomId> },
}

/// An internal conflict before labels and trace are materialized. `atoms` are the
/// exact display strings; `cause` are the atoms whose forcing chains explain it
/// (empty when there is no chain to show, e.g. a direct fact contradiction).
struct RawConflict {
    origin: Origin,
    atoms: Vec<String>,
    cause: Vec<AtomId>,
}

/// Working state of the forward + backward evaluation, evaluated as a pipeline.
struct Eval<'a> {
    c: &'a Compiled,
    model: Vec<V3>,
    /// Per-atom provenance, filled by `seed_facts` and `saturate_rules`; read by
    /// `build_trace` once the model is final.
    reason: Vec<Option<AtomReason>>,
    conflicts: Vec<RawConflict>,
    warnings: Vec<Warning>,
    derived: Vec<Derived>,
    /// Minimal set of constructs to blame when the backward pass finds UNSAT.
    unsat_core: Vec<CoreItem>,
}

impl<'a> Eval<'a> {
    fn new(c: &'a Compiled) -> Self {
        Eval {
            c,
            model: vec![V3::Unknown; c.atoms.len()],
            reason: vec![None; c.atoms.len()],
            conflicts: Vec::new(),
            warnings: Vec::new(),
            derived: Vec::new(),
            unsat_core: Vec::new(),
        }
    }

    fn label(&self, a: AtomId) -> String {
        label(self.c, a)
    }

    /// 1. Seed the model from confident facts; `FACT X` + `NOT X` is a CONFLICT.
    fn seed_facts(&mut self) {
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
    fn saturate_rules(&mut self) {
        let c = self.c;
        loop {
            let mut changed = false;
            for r in &c.rules {
                if conjunction(&self.model, &r.antecedent) != V3::True {
                    continue; // rule does not fire (FALSE, or blocked by UNKNOWN)
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

    /// 3. Evaluate every `Impossible` clause against the model.
    fn check_premises(&mut self) {
        let c = self.c;
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
                ClauseEval::Blocked(unknowns) if clause.origin.kind == "PREMISE" => {
                    self.warnings.push(Warning {
                        origin: clause.origin.clone(),
                        blocked_by: unknowns.iter().map(|a| self.label(*a)).collect(),
                    });
                }
                ClauseEval::Blocked(_) => {}
            }
        }
    }

    /// Build the derivation chain that explains why the `causes` atoms hold their
    /// current values: a post-order walk of the reason graph so each atom's
    /// supports appear before it (facts first, the conflict atoms last), with
    /// every atom emitted once. Atoms with no recorded reason (UNKNOWN) are
    /// skipped — a forced atom always has one.
    fn build_trace(&self, causes: &[AtomId]) -> Vec<TraceStep> {
        let mut visited = vec![false; self.c.atoms.len()];
        let mut out = Vec::new();
        for &a in causes {
            self.trace_dfs(a, &mut visited, &mut out);
        }
        out
    }

    fn trace_dfs(&self, a: AtomId, visited: &mut [bool], out: &mut Vec<TraceStep>) {
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
    fn backward_pass(&mut self) -> Option<String> {
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
                        kind: "UNSAT",
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

    /// Run the backward pass, sort deterministically, and assemble the report.
    fn finish(mut self) -> Report {
        let underdetermined = self.backward_pass();
        self.conflicts.sort_by_key(|c| key(&c.origin));
        self.warnings.sort_by_key(|w| key(&w.origin));
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
        let conflicts: Vec<Conflict> = self
            .conflicts
            .iter()
            .map(|rc| Conflict {
                origin: rc.origin.clone(),
                atoms: rc.atoms.clone(),
                trace: self.build_trace(&rc.cause),
            })
            .collect();
        Report {
            status,
            conflicts,
            warnings: self.warnings,
            derived: self.derived,
            underdetermined,
            unsat_core: self.unsat_core,
        }
    }
}

/// Evaluate a compiled program: the three-valued forward pass, then the backward
/// pass on `BIDIRECTIONAL`.
pub fn solve(c: &Compiled) -> Report {
    let mut e = Eval::new(c);
    e.seed_facts();
    e.saturate_rules();
    e.check_premises();
    e.finish()
}

/// Encode the premises (`Impossible` clauses), rules (as implications), and
/// confident facts (as unit clauses) into CNF for the backward pass. Also
/// returns the constrained atoms (those appearing in a clause or rule) to
/// project model counting onto.
fn build_cnf(c: &Compiled) -> (sat::Cnf, Vec<sat::Var>) {
    use sat::SatLit;
    let mut cnf = sat::Cnf::new(c.atoms.len());
    let mut constrained = vec![false; c.atoms.len()];
    let mark = |a: AtomId, constrained: &mut [bool]| constrained[a as usize] = true;

    // Impossible([L1..Ln]) == (¬L1 ∨ … ∨ ¬Ln); ¬L of (atom, negated) is (atom, negated).
    for clause in &c.clauses {
        let lits = clause
            .lits
            .iter()
            .map(|l| {
                mark(l.atom, &mut constrained);
                SatLit::new(l.atom, l.negated)
            })
            .collect();
        cnf.add_clause(lits);
    }
    // RULE WHEN A.. THEN C.. == for each C: (¬A1 ∨ … ∨ C).
    for r in &c.rules {
        for cons in &r.consequent {
            let mut lits: Vec<SatLit> = r
                .antecedent
                .iter()
                .map(|a| {
                    mark(a.atom, &mut constrained);
                    SatLit::new(a.atom, a.negated)
                })
                .collect();
            mark(cons.atom, &mut constrained);
            lits.push(SatLit::new(cons.atom, !cons.negated));
            cnf.add_clause(lits);
        }
    }
    // Confident facts as unit clauses.
    for f in &c.facts {
        let lit = match f.value {
            Value::True => SatLit::positive(f.atom),
            Value::False => SatLit::negative(f.atom),
        };
        cnf.add_clause(vec![lit]);
    }

    let project = (0..c.atoms.len() as AtomId)
        .filter(|&a| constrained[a as usize])
        .collect();
    (cnf, project)
}

/// Convert a three-valued model entry to a confident [`Value`] (UNKNOWN → `None`).
fn v3_to_value(v: V3) -> Option<Value> {
    match v {
        V3::True => Some(Value::True),
        V3::False => Some(Value::False),
        V3::Unknown => None,
    }
}

/// A removable source construct (one fact, one premise, or one rule) and the CNF
/// clauses it contributes — the unit of an unsat-core explanation.
struct Construct {
    origin: Origin,
    label: String,
    clauses: Vec<Vec<sat::SatLit>>,
}

/// Two origins refer to the same source construct.
fn same_origin(a: &Origin, b: &Origin) -> bool {
    a.source == b.source && a.line == b.line && a.premise == b.premise && a.kind == b.kind
}

/// Split the program into removable constructs. A premise that desugared into
/// several clauses (e.g. an `EXCLUSIVE` over n atoms) is grouped back into one
/// construct by origin, so the core blames whole premises, not clause shards.
fn constructs(c: &Compiled) -> Vec<Construct> {
    use sat::SatLit;
    let mut out: Vec<Construct> = Vec::new();

    for f in &c.facts {
        let lit = match f.value {
            Value::True => SatLit::positive(f.atom),
            Value::False => SatLit::negative(f.atom),
        };
        out.push(Construct {
            origin: f.origin.clone(),
            label: label(c, f.atom),
            clauses: vec![vec![lit]],
        });
    }

    let mut premises: Vec<Construct> = Vec::new();
    for clause in &c.clauses {
        let lits: Vec<SatLit> = clause
            .lits
            .iter()
            .map(|l| SatLit::new(l.atom, l.negated))
            .collect();
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
            .map(|cons| {
                let mut lits: Vec<SatLit> = r
                    .antecedent
                    .iter()
                    .map(|a| SatLit::new(a.atom, a.negated))
                    .collect();
                lits.push(SatLit::new(cons.atom, !cons.negated));
                lits
            })
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
fn subset_is_sat(num_vars: usize, all: &[Construct], active: &[bool]) -> bool {
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

/// A minimal (1-minimal) unsat core via deletion-based minimization: starting
/// from the whole (unsatisfiable) program, drop each construct in turn; if the
/// rest is still unsatisfiable the construct was not needed. What survives is an
/// irreducible set jointly to blame. Called only when the full system is UNSAT.
fn minimal_unsat_core(c: &Compiled) -> Vec<CoreItem> {
    let all = constructs(c);
    let mut active = vec![true; all.len()];
    for i in 0..all.len() {
        active[i] = false;
        if subset_is_sat(c.atoms.len(), &all, &active) {
            active[i] = true; // removing it restored SAT → it is part of the core
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
fn key(o: &Origin) -> (String, u32) {
    (o.source.clone(), o.line)
}

/// Parse → compile → solve a single source.
pub fn verify_source(name: &str, src: &str) -> Result<Report, CompileError> {
    Ok(solve(&compile_source(name, src)?))
}

/// Parse → compile (resolving imports) → solve, given a [`Resolver`].
pub fn verify<R: Resolver>(root: &str, resolver: &R) -> Result<Report, CompileError> {
    Ok(solve(&compile(root, resolver)?))
}

// --- human-readable report -------------------------------------------------

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Consistent => "CONSISTENT",
            Status::Underdetermined => "UNDERDETERMINED",
            Status::Warning => "WARNING",
            Status::Conflict => "CONFLICT",
        })
    }
}

/// Format provenance as `name (KIND)  [source:line]` for the human report.
fn premise_tag(o: &Origin) -> String {
    let name = o.premise.as_deref().unwrap_or("-");
    alloc::format!("{} ({})  [{}:{}]", name, o.kind, o.source, o.line)
}

/// One derivation-trace line for the human report.
fn trace_line(step: &TraceStep) -> String {
    let v = match step.value {
        Value::True => "TRUE",
        Value::False => "FALSE",
    };
    match &step.reason {
        TraceReason::Asserted(o) => {
            alloc::format!(
                "{} = {}   [{} {}:{}]",
                step.atom,
                v,
                o.kind,
                o.source,
                o.line
            )
        }
        TraceReason::Derived { origin, from } => alloc::format!(
            "{} = {}   from {} ({})  [{}:{}]  <= {}",
            step.atom,
            v,
            origin.premise.as_deref().unwrap_or("-"),
            origin.kind,
            origin.source,
            origin.line,
            from.join(", ")
        ),
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "RESULT: {}", self.status)?;
        for c in &self.conflicts {
            writeln!(f, "  CONFLICT  {}", premise_tag(&c.origin))?;
            for a in &c.atoms {
                writeln!(f, "      {}", a)?;
            }
            if !c.trace.is_empty() {
                writeln!(f, "      why:")?;
                for step in &c.trace {
                    writeln!(f, "        {}", trace_line(step))?;
                }
            }
        }
        if !self.unsat_core.is_empty() {
            writeln!(
                f,
                "  CORE  smallest jointly-unsatisfiable set ({}):",
                self.unsat_core.len()
            )?;
            for it in &self.unsat_core {
                let name = if it.label.is_empty() { "-" } else { &it.label };
                writeln!(
                    f,
                    "        {} ({})  [{}:{}]",
                    name, it.origin.kind, it.origin.source, it.origin.line
                )?;
            }
        }
        for w in &self.warnings {
            writeln!(f, "  WARNING   {}", premise_tag(&w.origin))?;
            writeln!(f, "      blocked by: {}", w.blocked_by.join(", "))?;
        }
        if let Some(atom) = &self.underdetermined {
            writeln!(f, "  UNDERDETERMINED  an alternative model exists")?;
            writeln!(f, "      pin it down: add  FACT {atom}  or  NOT {atom}")?;
        }
        for d in &self.derived {
            let v = match d.value {
                Value::True => "TRUE",
                Value::False => "FALSE",
            };
            writeln!(
                f,
                "  DERIVED   {} = {}   from {}",
                d.atom,
                v,
                premise_tag(&d.origin)
            )?;
        }
        let underdetermined = usize::from(self.status == Status::Underdetermined);
        writeln!(
            f,
            "SUMMARY: {} conflicts, {} underdetermined, {} warnings, {} derived",
            self.conflicts.len(),
            underdetermined,
            self.warnings.len(),
            self.derived.len()
        )?;
        write!(f, "EXIT_CODE: {}", self.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_consistent() {
        let r = verify_source("<t>", "FACT x a\nCHECK x\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.conflicts.is_empty() && r.warnings.is_empty());
    }

    #[test]
    fn fact_contradiction_is_conflict() {
        let r = verify_source("<t>", "FACT x a\nNOT x a\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.conflicts.len(), 1);
    }

    #[test]
    fn exclusive_violation_is_conflict() {
        let src = include_str!("../../../docs/examples/conflict.vrf");
        let r = verify_source("conflict.vrf", src).unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(
            r.conflicts[0].origin.premise.as_deref(),
            Some("fly_xor_swim")
        );
        assert_eq!(r.conflicts[0].atoms.len(), 2);
    }

    #[test]
    fn exclusive_with_unknown_is_consistent_not_warning() {
        // flying TRUE, swimming UNKNOWN — at most one can hold, no conflict, no warning.
        let r = verify_source("<t>", "FACT A has flying\nPREMISE e:\n    EXCLUSIVE\n        A has flying\n        A has swimming\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn implication_missing_consequent_is_warning() {
        // WHEN flying THEN wing: flying TRUE, wing UNKNOWN → blocked → WARNING.
        let r = verify_source(
            "<t>",
            "FACT A has flying\nPREMISE w:\n    WHEN A has flying\n    THEN A has wing\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Warning);
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].blocked_by, vec![String::from("A has wing")]);
    }

    #[test]
    fn implication_satisfied_is_consistent() {
        let r = verify_source("<t>", "FACT A has flying\nFACT A has wing\nPREMISE w:\n    WHEN A has flying\n    THEN A has wing\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn implication_violated_is_conflict() {
        // antecedent TRUE, consequent FALSE → CONFLICT.
        let r = verify_source("<t>", "FACT A has flying\nNOT A has wing\nPREMISE w:\n    WHEN A has flying\n    THEN A has wing\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
    }

    #[test]
    fn rule_derives_fact() {
        let r = verify_source(
            "<t>",
            "FACT A has flying\nRULE o:\n    WHEN A has flying\n    THEN A needs oxygen\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert_eq!(r.derived.len(), 1);
        assert_eq!(r.derived[0].atom, "A needs oxygen");
    }

    #[test]
    fn rule_derivation_contradiction_is_conflict() {
        // rule derives `A needs oxygen` TRUE, but it's asserted FALSE.
        let r = verify_source("<t>", "FACT A has flying\nNOT A needs oxygen\nRULE o:\n    WHEN A has flying\n    THEN A needs oxygen\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
    }

    #[test]
    fn bidirectional_finds_alternative_model_underdetermined() {
        // EXCLUSIVE(a,b) with no facts: {FF, TF, FT} all satisfy → not unique.
        let r = verify_source(
            "<t>",
            "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Underdetermined);
    }

    #[test]
    fn fact_pins_unique_model_consistent() {
        // Same premise, but FACT x a forces b false → the only model → CONSISTENT.
        let r = verify_source(
            "<t>",
            "FACT x a\nPREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn no_bidirectional_skips_backward_pass() {
        // Plain CHECK: alternatives are not searched → stays CONSISTENT, not UNDERDETERMINED.
        let r = verify_source(
            "<t>",
            "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn creature_example_forward_pass() {
        let src = include_str!("../../../docs/examples/creature.vrf");
        let r = verify_source("creature.vrf", src).unwrap();
        // fly_xor_swim & no_dual_temp consistent; wings_need_bone → 2 warnings
        // (wing, bone); needs_oxygen derived; no conflicts.
        assert_eq!(r.status, Status::Warning);
        assert!(r.conflicts.is_empty());
        assert_eq!(r.warnings.len(), 2);
        assert_eq!(r.derived.len(), 1);
        assert_eq!(r.derived[0].atom, "Creature.A needs oxygen");
    }

    #[test]
    fn roles_puzzle_is_uniquely_solved() {
        // 3 people × 3 roles, ONEOF per person and per role, two clues. The
        // backward (SAT) pass must find the assignment satisfiable AND unique —
        // i.e. CONSISTENT, not UNDERDETERMINED.
        let src = include_str!("../../../docs/examples/roles-puzzle.vrf");
        let r = verify_source("roles-puzzle.vrf", src).unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.conflicts.is_empty());
        assert!(r.underdetermined.is_none());
    }

    #[test]
    fn roles_puzzle_underdetermined_without_a_clue() {
        // Drop the `NOT bob is qa` clue and the solution is no longer unique
        // (bob/carol can swap dev/qa) — the SAT pass reports UNDERDETERMINED.
        // Normalize CRLF first: on a Windows checkout include_str! embeds the file
        // with \r\n, so a literal "...\n" match would otherwise miss the line.
        let src = include_str!("../../../docs/examples/roles-puzzle.vrf")
            .replace("\r\n", "\n")
            .replace("NOT  bob is qa\n", "");
        let r = verify_source("roles-puzzle.vrf", &src).unwrap();
        assert_eq!(r.status, Status::Underdetermined);
    }

    #[test]
    fn socrates_chain_is_a_conflict() {
        // human → animal → living → mortal (3 derivations), then mortal EXCLUSIVE
        // immortal with `immortal` asserted → CONFLICT on the exclusivity premise.
        let src = include_str!("../../../docs/examples/socrates.vrf");
        let r = verify_source("socrates.vrf", src).unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(
            r.conflicts[0].origin.premise.as_deref(),
            Some("mortal_xor_immortal")
        );
        assert_eq!(r.derived.len(), 3); // animal, living, mortal
    }

    // --- conflict explainability: derivation trace + minimal unsat core ------

    #[test]
    fn forward_conflict_carries_a_trace_of_its_facts() {
        let r = verify_source(
            "<t>",
            "FACT x a\nFACT x b\nPREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Conflict);
        let t = &r.conflicts[0].trace;
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].atom, "x a");
        assert_eq!(t[0].value, Value::True);
        assert!(matches!(t[0].reason, TraceReason::Asserted(_)));
        assert!(r.unsat_core.is_empty());
    }

    #[test]
    fn derivation_chain_is_traced_back_to_the_root_fact() {
        // human → animal → living → mortal, then mortal XOR immortal (immortal asserted).
        let src = include_str!("../../../docs/examples/socrates.vrf");
        let r = verify_source("socrates.vrf", src).unwrap();
        let t = &r.conflicts[0].trace;
        // human (fact) + animal, living, mortal (derived) + immortal (fact) = 5 steps,
        // supports before dependents.
        assert_eq!(t.len(), 5);
        assert_eq!(t[0].atom, "socrates is human");
        assert!(matches!(t[0].reason, TraceReason::Asserted(_)));
        let mortal = t.iter().find(|s| s.atom == "socrates is mortal").unwrap();
        match &mortal.reason {
            TraceReason::Derived { from, .. } => {
                assert_eq!(from, &vec![String::from("socrates is living")]);
            }
            _ => panic!("mortal should be derived, not asserted"),
        }
    }

    #[test]
    fn direct_fact_contradiction_has_no_trace() {
        let r = verify_source("<t>", "FACT x a\nNOT x a\nCHECK x\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert!(r.conflicts[0].trace.is_empty());
    }

    #[test]
    fn jointly_unsatisfiable_reports_a_minimal_core() {
        // ONEOF(a,b); a→c; b→c; NOT c. Unsat only via case-split, so the forward
        // pass misses it and the backward pass produces the core.
        let src = "PREMISE one:\n    ONEOF\n        x a\n        x b\nPREMISE ac:\n    WHEN x a\n    THEN x c\nPREMISE bc:\n    WHEN x b\n    THEN x c\nNOT x c\nCHECK x BIDIRECTIONAL\n";
        let r = verify_source("<t>", src).unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.conflicts[0].origin.kind, "UNSAT");
        assert_eq!(r.unsat_core.len(), 4);
        let labels: Vec<&str> = r.unsat_core.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"one"));
        assert!(labels.contains(&"x c")); // the bare NOT fact is labelled by its atom
    }

    #[test]
    fn unsat_core_excludes_irrelevant_constructs() {
        // The same unsat cluster, plus an unrelated fact + premise that must not
        // appear in the (irreducible) core.
        let src = "PREMISE one:\n    ONEOF\n        x a\n        x b\nPREMISE ac:\n    WHEN x a\n    THEN x c\nPREMISE bc:\n    WHEN x b\n    THEN x c\nNOT x c\nFACT z here\nPREMISE noise:\n    EXCLUSIVE\n        z here\n        z gone\nCHECK x BIDIRECTIONAL\n";
        let r = verify_source("<t>", src).unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.unsat_core.len(), 4);
        let labels: Vec<&str> = r.unsat_core.iter().map(|c| c.label.as_str()).collect();
        assert!(!labels.contains(&"noise"));
        assert!(!labels.iter().any(|l| l.contains("here")));
    }

    #[test]
    fn consistent_report_has_empty_core_and_no_trace() {
        let r = verify_source("<t>", "FACT x a\nCHECK x BIDIRECTIONAL\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.unsat_core.is_empty());
        assert!(r.conflicts.is_empty());
    }
}
