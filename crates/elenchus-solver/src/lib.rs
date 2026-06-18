//! elenchus-solver — the reasoning interpreter (forward pass).
//!
//! Consumes the [`Compiled`] IR from `elenchus-compiler` and evaluates it under
//! three-valued Kleene logic (TRUE / FALSE / UNKNOWN, where UNKNOWN ≠ FALSE):
//!
//! 1. seed a model from confident `FACT`/`NOT` facts (and report `FACT X` + `NOT X`);
//! 2. forward-chain `RULE`s to a fixpoint, deriving facts (a derived value that
//!    contradicts an existing one is a CONFLICT);
//! 3. evaluate every `Impossible` clause (the desugared `AXIOM`s):
//!    - all literals forced TRUE → **CONFLICT** (the constraint is violated);
//!    - some literal FALSE → satisfied → CONSISTENT;
//!    - otherwise (no FALSE, an UNKNOWN remains): for implication axioms this is a
//!      **WARNING** (blocked by missing data), for list axioms (EXCLUSIVE/FORBIDS/
//!      ONEOF/ATLEAST) it is CONSISTENT (UNKNOWN means "no conflict yet").
//!
//! On `CHECK ... BIDIRECTIONAL` a **backward pass** also runs: the axioms, rules
//! and confident facts are encoded as CNF and handed to a small in-crate CDCL SAT
//! core ([`sat`], replicating varisat's algorithm) to count models — 0 means the
//! system is jointly unsatisfiable (a CONFLICT the forward pass may miss), ≥2
//! means an alternative model exists (`UNDERDETERMINED`).
#![no_std]

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
    True,
    False,
    Unknown,
}

impl V3 {
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
    Consistent,
    /// The constraints are satisfiable but do not pin a unique assignment — an
    /// alternative model exists (found by the backward pass on `BIDIRECTIONAL`).
    Underdetermined,
    Warning,
    Conflict,
}

/// A violated constraint (or a fact-level contradiction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub origin: Origin,
    /// Human labels of the atoms participating in the contradiction.
    pub atoms: Vec<String>,
}

/// A constraint that could not be checked because a needed atom is UNKNOWN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub origin: Origin,
    /// Human labels of the UNKNOWN atoms blocking the check.
    pub blocked_by: Vec<String>,
}

/// A fact produced by a `RULE` during forward chaining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub atom: String,
    pub value: Value,
    pub origin: Origin,
}

/// The result of solving, self-contained (atom ids already resolved to labels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    pub conflicts: Vec<Conflict>,
    pub warnings: Vec<Warning>,
    pub derived: Vec<Derived>,
    /// When `UNDERDETERMINED`, the label of an atom left free by the constraints
    /// (asserting it would pin the model down).
    pub underdetermined: Option<String>,
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
            s.push('}');
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
        s.push('}');
        s
    }
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

/// `"axiom":..,"kind":..,"source":..,"line":..` (no braces).
fn json_origin_fields(o: &Origin, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("\"axiom\":");
    match &o.axiom {
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

fn label(c: &Compiled, a: AtomId) -> String {
    let k = &c.atoms[a as usize];
    match &k.object {
        Some(o) => alloc::format!("{} {} {}", k.subject, k.predicate, o),
        None => alloc::format!("{} {}", k.subject, k.predicate),
    }
}

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

/// Working state of the forward + backward evaluation, evaluated as a pipeline.
struct Eval<'a> {
    c: &'a Compiled,
    model: Vec<V3>,
    conflicts: Vec<Conflict>,
    warnings: Vec<Warning>,
    derived: Vec<Derived>,
}

impl<'a> Eval<'a> {
    fn new(c: &'a Compiled) -> Self {
        Eval {
            c,
            model: vec![V3::Unknown; c.atoms.len()],
            conflicts: Vec::new(),
            warnings: Vec::new(),
            derived: Vec::new(),
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
                    self.conflicts.push(Conflict {
                        origin: o.clone(),
                        atoms: vec![alloc::format!(
                            "{} (asserted both TRUE and FALSE)",
                            self.label(a as AtomId)
                        )],
                    });
                }
                (Some(_), None) => self.model[a] = V3::True,
                (None, Some(_)) => self.model[a] = V3::False,
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
                        _ => self.conflicts.push(Conflict {
                            origin: r.origin.clone(),
                            atoms: vec![alloc::format!(
                                "{} (derived value contradicts a known fact)",
                                self.label(cl.atom)
                            )],
                        }),
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// 3. Evaluate every `Impossible` clause against the model.
    fn check_axioms(&mut self) {
        let c = self.c;
        for clause in &c.clauses {
            match eval_clause(&self.model, clause) {
                ClauseEval::Violated => self.conflicts.push(Conflict {
                    origin: clause.origin.clone(),
                    atoms: clause.lits.iter().map(|l| self.label(l.atom)).collect(),
                }),
                ClauseEval::Satisfied => {}
                // Implication axioms warn on missing data; list axioms treat
                // UNKNOWN as "no conflict yet" and stay consistent.
                ClauseEval::Blocked(unknowns) if clause.origin.kind == "AXIOM" => {
                    self.warnings.push(Warning {
                        origin: clause.origin.clone(),
                        blocked_by: unknowns.iter().map(|a| self.label(*a)).collect(),
                    });
                }
                ClauseEval::Blocked(_) => {}
            }
        }
    }

    /// Backward pass (model finding), run only when a CHECK requests BIDIRECTIONAL.
    /// Encodes axioms + rules + facts as CNF and asks the SAT core for models.
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
                self.conflicts.push(Conflict {
                    origin: Origin {
                        source: String::from("<system>"),
                        line: 0,
                        axiom: None,
                        kind: "UNSAT",
                    },
                    atoms: vec![String::from(
                        "the axioms and facts are jointly unsatisfiable",
                    )],
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
        Report {
            status,
            conflicts: self.conflicts,
            warnings: self.warnings,
            derived: self.derived,
            underdetermined,
        }
    }
}

/// Evaluate a compiled program: the three-valued forward pass, then the backward
/// pass on `BIDIRECTIONAL`.
pub fn solve(c: &Compiled) -> Report {
    let mut e = Eval::new(c);
    e.seed_facts();
    e.saturate_rules();
    e.check_axioms();
    e.finish()
}

/// Encode the axioms (`Impossible` clauses), rules (as implications), and
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

fn axiom_tag(o: &Origin) -> String {
    let name = o.axiom.as_deref().unwrap_or("-");
    alloc::format!("{} ({})  [{}:{}]", name, o.kind, o.source, o.line)
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "RESULT: {}", self.status)?;
        for c in &self.conflicts {
            writeln!(f, "  CONFLICT  {}", axiom_tag(&c.origin))?;
            for a in &c.atoms {
                writeln!(f, "      {}", a)?;
            }
        }
        for w in &self.warnings {
            writeln!(f, "  WARNING   {}", axiom_tag(&w.origin))?;
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
                axiom_tag(&d.origin)
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
        assert_eq!(r.conflicts[0].origin.axiom.as_deref(), Some("fly_xor_swim"));
        assert_eq!(r.conflicts[0].atoms.len(), 2);
    }

    #[test]
    fn exclusive_with_unknown_is_consistent_not_warning() {
        // flying TRUE, swimming UNKNOWN — at most one can hold, no conflict, no warning.
        let r = verify_source("<t>", "FACT A has flying\nAXIOM e:\n    EXCLUSIVE\n        A has flying\n        A has swimming\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn implication_missing_consequent_is_warning() {
        // WHEN flying THEN wing: flying TRUE, wing UNKNOWN → blocked → WARNING.
        let r = verify_source(
            "<t>",
            "FACT A has flying\nAXIOM w:\n    WHEN A has flying\n    THEN A has wing\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Warning);
        assert_eq!(r.warnings.len(), 1);
        assert_eq!(r.warnings[0].blocked_by, vec![String::from("A has wing")]);
    }

    #[test]
    fn implication_satisfied_is_consistent() {
        let r = verify_source("<t>", "FACT A has flying\nFACT A has wing\nAXIOM w:\n    WHEN A has flying\n    THEN A has wing\n").unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn implication_violated_is_conflict() {
        // antecedent TRUE, consequent FALSE → CONFLICT.
        let r = verify_source("<t>", "FACT A has flying\nNOT A has wing\nAXIOM w:\n    WHEN A has flying\n    THEN A has wing\n").unwrap();
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
            "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Underdetermined);
    }

    #[test]
    fn fact_pins_unique_model_consistent() {
        // Same axiom, but FACT x a forces b false → the only model → CONSISTENT.
        let r = verify_source(
            "<t>",
            "FACT x a\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x BIDIRECTIONAL\n",
        )
        .unwrap();
        assert_eq!(r.status, Status::Consistent);
    }

    #[test]
    fn no_bidirectional_skips_backward_pass() {
        // Plain CHECK: alternatives are not searched → stays CONSISTENT, not UNDERDETERMINED.
        let r = verify_source(
            "<t>",
            "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nCHECK x\n",
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
}
