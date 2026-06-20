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

use elenchus_compiler::{AtomId, AtomKey, Clause, Compiled, Lit, Origin, Value};
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
    /// When `ASSUME` hypotheses are what break an otherwise-consistent program,
    /// the minimal set of assumptions that cannot all hold *together with the
    /// (consistent) facts, premises and rules* — dropping any one restores
    /// consistency. Only ever lists `ASSUME` constructs: facts and premises are
    /// never blamed. Empty whenever the facts/premises are themselves to blame
    /// (a hard contradiction) or there is no conflict at all. The verdict stays
    /// `CONFLICT` (exit code 2); this field only says *which dial to turn*.
    pub retract: Vec<CoreItem>,
    /// Advisory near-duplicate atom-name hints (possible typos). Never affects
    /// [`Report::status`] or [`Report::exit_code`] — purely informational.
    pub hints: Vec<SimilarAtoms>,
}

/// An advisory hint that two atom names look like the same atom typed two
/// different ways (e.g. `is_rolled_back` vs `is rolled_back`). **Purely a
/// suggestion** — it never changes the verdict, the warning pool, or the exit
/// code. It exists to catch the silent-typo trap where a misspelling creates a
/// new UNKNOWN atom that quietly never links to the rest of the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimilarAtoms {
    /// One atom's human label (`subject predicate [object]`).
    pub a: String,
    /// The other atom's human label.
    pub b: String,
    /// Why the pair was flagged (a short, fixed explanation).
    pub reason: &'static str,
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
        s.push_str("],\"retract\":[");
        for (i, it) in self.retract.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            json_str(&it.label, &mut s);
            s.push('}');
        }
        s.push_str("],\"hints\":[");
        for (i, h) in self.hints.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"a\":");
            json_str(&h.a, &mut s);
            s.push_str(",\"b\":");
            json_str(&h.b, &mut s);
            s.push_str(",\"reason\":");
            json_str(h.reason, &mut s);
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
            retract: Vec::new(), // filled by `solve` when assumptions are to blame
            hints: Vec::new(),   // filled by `solve` (advisory, post-verdict)
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
    let mut report = e.finish();
    // If the program is a CONFLICT but the facts/premises are consistent on their
    // own, the `ASSUME` hypotheses are what break it: name which to retract. The
    // verdict stays CONFLICT — this only adds the "drop one of these" hint and,
    // when it applies, supersedes the raw conflict/unsat-core pools (which would
    // otherwise point at the assumption clause itself).
    if report.status == Status::Conflict {
        let retract = retract_assumptions(c);
        if !retract.is_empty() {
            report.unsat_core = Vec::new();
            report.retract = retract;
        }
    }
    // Advisory only: surface likely atom-name typos. Computed after the verdict
    // so it can never influence status/exit code.
    report.hints = similar_atom_pairs(c);
    report
}

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
fn retract_assumptions(c: &Compiled) -> Vec<CoreItem> {
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

/// Detect pairs of distinct atoms whose names look like the same atom typed two
/// ways. Two deliberately conservative signals (keep false positives minimal):
///
/// - **A — fold-equal:** identical after lowercasing and treating `_`/whitespace
///   as one separator (`Has_fuel`/`has_fuel`, `is_rolled_back`/`is rolled_back`).
///   Distinct atoms that fold to the same string are almost always one typo.
/// - **B — near edit:** *same subject*, an *alphabetic (cased)* script, and a
///   Levenshtein distance of exactly 1 over the folded form (names ≥ 5 chars).
///   Distance 1 only — distance 2 flags real antonyms (mortal/immortal) far too
///   often. Edit distance is a typo signal only where a word spans many
///   characters; in caseless scripts (CJK / kana / hangul) one character is a
///   whole word, so a one-character change is normally a *different* word — those
///   are skipped by the "cased letters only" test (no hard-coded Unicode ranges).
///
/// Signal A is fully script-agnostic; signal B is the script-sensitive one.
/// `O(n²)` over the (typically small) atom set, with a length-difference quick
/// reject. Deterministic: atoms are already canonically sorted in `Compiled`.
fn similar_atom_pairs(c: &Compiled) -> Vec<SimilarAtoms> {
    let folded: Vec<Vec<char>> = c.atoms.iter().map(fold_atom).collect();
    let cased: Vec<bool> = folded.iter().map(|f| is_cased_alphabetic(f)).collect();
    let mut out = Vec::new();
    for i in 0..c.atoms.len() {
        for j in (i + 1)..c.atoms.len() {
            if let Some(reason) = atoms_look_similar(
                &c.atoms[i],
                &folded[i],
                cased[i],
                &c.atoms[j],
                &folded[j],
                cased[j],
            ) {
                out.push(SimilarAtoms {
                    a: label(c, i as AtomId),
                    b: label(c, j as AtomId),
                    reason,
                });
            }
        }
    }
    out
}

/// Fold an atom to its comparison form: `subject predicate [object]` lowercased,
/// every `_`/whitespace run collapsed to a single space. So `_` vs space vs case
/// can never distinguish two names.
fn fold_atom(k: &AtomKey) -> Vec<char> {
    let mut raw = String::new();
    raw.push_str(&k.subject);
    raw.push(' ');
    raw.push_str(&k.predicate);
    if let Some(o) = &k.object {
        raw.push(' ');
        raw.push_str(o);
    }
    let mut out: Vec<char> = Vec::new();
    let mut prev_space = false;
    for ch in raw.chars() {
        if ch == '_' || ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        }
    }
    if out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// Whether every character of a folded name is a space or a *cased* letter — the
/// script-agnostic gate for edit-distance (signal B). Cased scripts (Latin,
/// Cyrillic, Greek, …) span many characters per word, so a one-character edit is
/// a plausible typo. Caseless scripts (CJK / kana / hangul, where one character
/// is a whole word) and digits report `is_lowercase() == false` after folding, so
/// they fall out here without enumerating any Unicode ranges.
fn is_cased_alphabetic(folded: &[char]) -> bool {
    folded.iter().all(|&c| c == ' ' || c.is_lowercase())
}

/// The two-signal similarity test (see [`similar_atom_pairs`]). Returns the
/// reason string when the pair looks like a typo, else `None`.
fn atoms_look_similar(
    ka: &AtomKey,
    fa: &[char],
    cased_a: bool,
    kb: &AtomKey,
    fb: &[char],
    cased_b: bool,
) -> Option<&'static str> {
    // A — same folded form (the AtomKeys differ, so the raw spelling differs).
    if fa == fb {
        return Some("same name up to case, '_', or spaces");
    }
    // B — same subject, an alphabetic (cased) script, a single-character slip.
    // Only distance 1: distance 2 flags real antonyms (mortal/immortal) and word
    // pairs far too often — genuine typos are almost always a one-character edit,
    // and the underscore/case case is already covered by signal A.
    if !cased_a || !cased_b || ka.subject != kb.subject {
        return None;
    }
    if fa.len().abs_diff(fb.len()) > 1 {
        return None; // edit distance >= length difference, so it can't be 1
    }
    let min_len = fa.len().min(fb.len());
    if min_len >= 5 && levenshtein(fa, fb) == 1 {
        return Some("looks like a one-character typo of each other");
    }
    None
}

/// Plain Levenshtein edit distance over char slices (rolling two-row DP).
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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

/// A fast sufficient core via one assumption-solve: each construct gets a fresh
/// selector variable `s_k`, every clause becomes `(¬s_k ∨ clause)`, and we solve
/// asserting all selectors true. The SAT core (a subset of the selectors) names a
/// sufficient set of constructs in a single solve — versus O(n) deletion solves.
/// Returns an `active` mask over `all`.
fn candidate_via_assumptions(c: &Compiled, all: &[Construct]) -> Vec<bool> {
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
fn minimal_unsat_core(c: &Compiled) -> Vec<CoreItem> {
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

/// Indentation levels for the human report. This module is the **single** place
/// leading whitespace is defined: every line is emitted through
/// [`ReportWriter::line`] with one of these as the `indent` argument, so no
/// format string ever carries leading spaces. To restyle the report, change a
/// number here — not spaces scattered across `write!` calls.
mod indent {
    /// `RESULT:` / `SUMMARY:` / `EXIT_CODE:` — flush left.
    pub const ROOT: usize = 0;
    /// A section header: `CONFLICT` / `WARNING` / `CORE` / `RETRACT` / `DERIVED`
    /// / `HINT` / `UNDERDETERMINED`.
    pub const SECTION: usize = 2;
    /// A line belonging to a section (conflict atoms, `blocked by:`, an `ASSUME`).
    pub const ITEM: usize = 6;
    /// A line nested under an item (a `why:` trace step, a `CORE` member).
    pub const NESTED: usize = 8;
}

/// The human report's one output primitive. It owns the indentation rule so
/// callers pass a semantic [`indent`] level and the text — never raw spaces.
struct ReportWriter<'a, 'b> {
    f: &'a mut fmt::Formatter<'b>,
}

impl ReportWriter<'_, '_> {
    /// Write `indent` leading spaces, the formatted text, then a newline.
    fn line(&mut self, indent: usize, args: fmt::Arguments<'_>) -> fmt::Result {
        write!(self.f, "{:width$}{}", "", args, width = indent)?;
        self.f.write_str("\n")
    }

    /// Like [`line`](Self::line) but without the trailing newline — for the final
    /// `EXIT_CODE` line, so the report ends exactly as it always has.
    fn tail(&mut self, indent: usize, args: fmt::Arguments<'_>) -> fmt::Result {
        write!(self.f, "{:width$}{}", "", args, width = indent)
    }
}

/// `emit!(out, LEVEL, "fmt", args…)` — one indented report line. A thin wrapper
/// over [`ReportWriter::line`] so call sites read `emit!(out, SECTION, …)` with
/// the indent as an explicit parameter and zero leading spaces in the string.
macro_rules! emit {
    ($out:expr, $indent:expr, $($arg:tt)*) => {
        $out.line($indent, format_args!($($arg)*))
    };
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use indent::{ITEM, NESTED, ROOT, SECTION};
        let mut out = ReportWriter { f };

        emit!(out, ROOT, "RESULT: {}", self.status)?;

        // A pure assumption clash: lead with the one action a small model needs
        // and suppress the raw conflict / CORE pools (they would only echo the
        // ASSUME clause). The verdict is still CONFLICT (exit code 2).
        if !self.retract.is_empty() {
            emit!(
                out,
                SECTION,
                "RETRACT  drop or flip ONE of these ASSUME guesses (your FACTs/PREMISEs are fine):"
            )?;
            for it in &self.retract {
                emit!(
                    out,
                    ITEM,
                    "ASSUME {}   [{}:{}]",
                    it.label,
                    it.origin.source,
                    it.origin.line
                )?;
            }
        } else {
            for c in &self.conflicts {
                emit!(out, SECTION, "CONFLICT  {}", premise_tag(&c.origin))?;
                for a in &c.atoms {
                    emit!(out, ITEM, "{}", a)?;
                }
                if !c.trace.is_empty() {
                    emit!(out, ITEM, "why:")?;
                    for step in &c.trace {
                        emit!(out, NESTED, "{}", trace_line(step))?;
                    }
                }
            }
            if !self.unsat_core.is_empty() {
                emit!(
                    out,
                    SECTION,
                    "CORE  smallest jointly-unsatisfiable set ({}):",
                    self.unsat_core.len()
                )?;
                for it in &self.unsat_core {
                    let name = if it.label.is_empty() { "-" } else { &it.label };
                    emit!(
                        out,
                        NESTED,
                        "{} ({})  [{}:{}]",
                        name,
                        it.origin.kind,
                        it.origin.source,
                        it.origin.line
                    )?;
                }
            }
        }

        for w in &self.warnings {
            emit!(out, SECTION, "WARNING   {}", premise_tag(&w.origin))?;
            emit!(out, ITEM, "blocked by: {}", w.blocked_by.join(", "))?;
        }
        if let Some(atom) = &self.underdetermined {
            emit!(out, SECTION, "UNDERDETERMINED  an alternative model exists")?;
            emit!(out, ITEM, "pin it down: add  FACT {atom}  or  NOT {atom}")?;
        }
        for d in &self.derived {
            let v = match d.value {
                Value::True => "TRUE",
                Value::False => "FALSE",
            };
            emit!(
                out,
                SECTION,
                "DERIVED   {} = {}   from {}",
                d.atom,
                v,
                premise_tag(&d.origin)
            )?;
        }
        for h in &self.hints {
            emit!(
                out,
                SECTION,
                "HINT      possible typo — '{}' and '{}' look like the same atom ({})",
                h.a,
                h.b,
                h.reason
            )?;
        }

        let underdetermined = usize::from(self.status == Status::Underdetermined);
        emit!(
            out,
            ROOT,
            "SUMMARY: {} conflicts, {} underdetermined, {} warnings, {} derived",
            self.conflicts.len(),
            underdetermined,
            self.warnings.len(),
            self.derived.len()
        )?;
        out.tail(ROOT, format_args!("EXIT_CODE: {}", self.exit_code()))
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
            r#"
        FACT A has flying
        PREMISE w:
            WHEN A has flying
            THEN A has wing
        "#,
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
            r#"
        FACT A has flying
        RULE o:
            WHEN A has flying
            THEN A needs oxygen
        "#,
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
            r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#,
        )
        .unwrap();
        assert_eq!(r.status, Status::Underdetermined);
    }

    #[test]
    fn fact_pins_unique_model_consistent() {
        // Same premise, but FACT x a forces b false → the only model → CONSISTENT.
        let r = verify_source(
            "<t>",
            r#"
        FACT x a
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x BIDIRECTIONAL
        "#,
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn no_bidirectional_skips_backward_pass() {
        // Plain CHECK: alternatives are not searched → stays CONSISTENT, not UNDERDETERMINED.
        let r = verify_source(
            "<t>",
            r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#,
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
            r#"
        FACT x a
        FACT x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        CHECK x
        "#,
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
        let src = r#"
        PREMISE one:
            ONEOF
                x a
                x b
        PREMISE ac:
            WHEN x a
            THEN x c
        PREMISE bc:
            WHEN x b
            THEN x c
        NOT x c
        CHECK x BIDIRECTIONAL
        "#;
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
        let src = r#"
        PREMISE one:
            ONEOF
                x a
                x b
        PREMISE ac:
            WHEN x a
            THEN x c
        PREMISE bc:
            WHEN x b
            THEN x c
        NOT x c
        FACT z here
        PREMISE noise:
            EXCLUSIVE
                z here
                z gone
        CHECK x BIDIRECTIONAL
        "#;
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

    // --- ASSUME: soft (retractable) hypotheses -----------------------------

    #[test]
    fn compatible_assumptions_behave_like_facts() {
        // ASSUME that does not clash with anything → ordinary CONSISTENT, and the
        // assumption participates like a fact (no retract, no conflict).
        let r = verify_source("<t>", "ASSUME rel in_prod\nFACT rel reviewed\nCHECK rel\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.retract.is_empty());
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn assume_drives_a_rule_like_a_fact() {
        // A soft assumption fires forward chaining just like a hard fact.
        let r = verify_source(
            "<t>",
            "ASSUME A has flying\nRULE o:\n    WHEN A has flying\n    THEN A needs oxygen\nCHECK A\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert_eq!(r.derived.len(), 1);
        assert_eq!(r.derived[0].atom, "A needs oxygen");
    }

    #[test]
    fn clashing_assumptions_yield_conflict_with_a_retract_set() {
        // in_prod needs a rollback OR a feature flag; assuming in_prod plus
        // neither makes the premise unsatisfiable — but only via the guesses.
        let src = r#"
        FACT rel reviewed
        PREMISE prod_needs_safety:
            WHEN rel in_prod
            THEN rel has_rollback
            OR   rel has_feature_flag
        ASSUME rel in_prod
        ASSUME NOT rel has_rollback
        ASSUME NOT rel has_feature_flag
        CHECK rel
        "#;
        let r = verify_source("<t>", src).unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.exit_code(), 2);
        // All three guesses are jointly to blame: dropping any one fixes it.
        assert_eq!(r.retract.len(), 3, "{:?}", r.retract);
        let labels: Vec<&str> = r.retract.iter().map(|it| it.label.as_str()).collect();
        assert!(labels.contains(&"rel in_prod"));
        assert!(labels.contains(&"NOT rel has_rollback"));
        assert!(labels.contains(&"NOT rel has_feature_flag"));
        // Every retract item is an ASSUME — a FACT/PREMISE is never blamed.
        assert!(r.retract.iter().all(|it| it.origin.kind == "ASSUME"));
        // The human report leads with RETRACT and hides the raw conflict pool.
        let shown = alloc::format!("{r}");
        assert!(shown.contains("RETRACT"), "{shown}");
        assert!(!shown.contains("CONFLICT  "), "{shown}");
    }

    #[test]
    fn assume_vs_fact_retracts_only_the_assumption() {
        // FACT x a is ground truth; ASSUME NOT x a is the only removable thing.
        let r = verify_source("<t>", "FACT x a\nASSUME NOT x a\nCHECK x\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.retract.len(), 1);
        assert_eq!(r.retract[0].label, "NOT x a");
        assert_eq!(r.retract[0].origin.kind, "ASSUME");
    }

    #[test]
    fn hard_conflict_is_not_blamed_on_assumptions() {
        // The FACTs themselves contradict; an unrelated ASSUME must NOT appear in
        // a retract set (the hard program is already broken).
        let r = verify_source("<t>", "FACT x a\nNOT x a\nASSUME y b\nCHECK x\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert!(r.retract.is_empty(), "{:?}", r.retract);
    }

    #[test]
    fn two_assumptions_directly_contradict() {
        let r = verify_source("<t>", "ASSUME x a\nASSUME NOT x a\nCHECK x\n").unwrap();
        assert_eq!(r.status, Status::Conflict);
        assert_eq!(r.retract.len(), 2, "{:?}", r.retract);
    }

    #[test]
    fn assume_retract_is_in_json() {
        let r = verify_source("<t>", "FACT x a\nASSUME NOT x a\nCHECK x\n").unwrap();
        let j = r.to_json();
        assert!(j.contains("\"retract\":["), "{j}");
        assert!(j.contains("\"kind\":\"ASSUME\""), "{j}");
        assert!(j.contains("NOT x a"), "{j}");
    }

    // --- near-duplicate atom hints (advisory typo detector) ----------------

    #[test]
    fn hint_flags_underscore_vs_space_and_is_advisory_only() {
        // The real trap: `is rolled_back` (obj) vs `is_rolled_back` (pred) are
        // DIFFERENT atoms — no contradiction, so the verdict stays CONSISTENT —
        // but the hint warns they were probably meant to be one atom.
        let r = verify_source(
            "<t>",
            r#"
        FACT auth is rolled_back
        NOT auth is_rolled_back
        CHECK
        "#,
        )
        .unwrap();
        assert_eq!(
            r.status,
            Status::Consistent,
            "hint must not change the verdict"
        );
        assert_eq!(r.exit_code(), 0, "hint must not change the exit code");
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
        assert!(r.hints[0].reason.contains('_') || r.hints[0].reason.contains("case"));
    }

    #[test]
    fn hint_flags_case_only_difference() {
        let r = verify_source("<t>", "FACT Engine has_fuel\nNOT Engine Has_fuel\nCHECK\n").unwrap();
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    }

    #[test]
    fn hint_flags_single_char_typo_same_subject() {
        // alphabetic, same subject, edit distance 1, len >= 5 → signal B.
        let r = verify_source("<t>", "FACT svc deployed\nNOT svc deployd\nCHECK\n").unwrap();
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    }

    #[test]
    fn no_hint_for_short_distinct_atoms() {
        // `x a` vs `x b`: distance 1 but intentionally different — must NOT flag.
        let r = verify_source("<t>", "FACT x a\nNOT x b\nCHECK\n").unwrap();
        assert!(r.hints.is_empty(), "{:?}", r.hints);
    }

    #[test]
    fn no_hint_for_distinct_words() {
        let r = verify_source("<t>", "FACT p is lead\nNOT p is dev\nNOT p is qa\nCHECK\n").unwrap();
        assert!(r.hints.is_empty(), "{:?}", r.hints);
    }

    #[test]
    fn russian_case_typo_is_flagged() {
        // Signal A is script-agnostic: lowercasing works for Cyrillic too.
        let r = verify_source("<t>", "FACT кот спит\nNOT Кот спит\nCHECK\n").unwrap();
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    }

    #[test]
    fn russian_single_char_typo_is_flagged() {
        let r = verify_source("<t>", "FACT кот пушистый\nNOT кот пушстый\nCHECK\n").unwrap();
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    }

    #[test]
    fn cjk_one_char_difference_is_not_flagged() {
        // Caseless script: a one-character change is a different word, not a typo,
        // so the edit-distance signal is skipped (only exact fold-equality fires).
        let r = verify_source("<t>", "FACT a 是黑\nNOT a 是白\nCHECK\n").unwrap();
        assert!(r.hints.is_empty(), "{:?}", r.hints);
    }

    #[test]
    fn cjk_underscore_vs_space_is_flagged() {
        // Signal A still applies to any script: `a 猫_黑` (pred) vs `a 猫 黑`
        // (pred+obj) fold to the same name.
        let r = verify_source("<t>", "FACT a 猫_黑\nNOT a 猫 黑\nCHECK\n").unwrap();
        assert_eq!(r.hints.len(), 1, "{:?}", r.hints);
    }
}
