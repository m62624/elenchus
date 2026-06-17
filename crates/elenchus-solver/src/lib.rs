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
//! This is **phase 1**: the forward pass needs no SAT backend. The backward pass
//! (`UNDERDETERMINED` via all-SAT / model finding) and a varisat port are future
//! work in this crate.
#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use elenchus_compiler::{AtomId, Compiled, Lit, Origin, Value};
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

/// Overall verdict for the system (UNDERDETERMINED is deferred to the backward pass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Consistent,
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

/// Evaluate a compiled program with the forward pass.
pub fn solve(c: &Compiled) -> Report {
    let n = c.atoms.len();
    let mut model = vec![V3::Unknown; n];
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let mut derived: Vec<Derived> = Vec::new();

    // 1. Seed the model from confident facts; catch FACT X + NOT X.
    let mut true_origin: Vec<Option<Origin>> = vec![None; n];
    let mut false_origin: Vec<Option<Origin>> = vec![None; n];
    for f in &c.facts {
        let slot = match f.value {
            Value::True => &mut true_origin,
            Value::False => &mut false_origin,
        };
        if slot[f.atom as usize].is_none() {
            slot[f.atom as usize] = Some(f.origin.clone());
        }
    }
    for a in 0..n {
        match (&true_origin[a], &false_origin[a]) {
            (Some(t), Some(_)) => {
                model[a] = V3::True;
                conflicts.push(Conflict {
                    origin: t.clone(),
                    atoms: vec![alloc::format!("{} (asserted both TRUE and FALSE)", label(c, a as AtomId))],
                });
            }
            (Some(_), None) => model[a] = V3::True,
            (None, Some(_)) => model[a] = V3::False,
            (None, None) => {}
        }
    }

    // 2. Forward-chain RULEs to a fixpoint.
    loop {
        let mut changed = false;
        for r in &c.rules {
            if conjunction(&model, &r.antecedent) != V3::True {
                continue; // rule does not fire (FALSE or blocked by UNKNOWN)
            }
            for cl in &r.consequent {
                let target = if cl.negated { V3::False } else { V3::True };
                let slot = &mut model[cl.atom as usize];
                match *slot {
                    V3::Unknown => {
                        *slot = target;
                        changed = true;
                        derived.push(Derived {
                            atom: label(c, cl.atom),
                            value: if cl.negated { Value::False } else { Value::True },
                            origin: r.origin.clone(),
                        });
                    }
                    v if v == target => {}
                    _ => conflicts.push(Conflict {
                        origin: r.origin.clone(),
                        atoms: vec![alloc::format!(
                            "{} (derived value contradicts a known fact)",
                            label(c, cl.atom)
                        )],
                    }),
                }
            }
        }
        if !changed {
            break;
        }
    }

    // 3. Evaluate Impossible clauses (the desugared AXIOMs).
    for clause in &c.clauses {
        let mut any_false = false;
        let mut all_true = true;
        let mut unknown_atoms: Vec<AtomId> = Vec::new();
        for l in &clause.lits {
            match lit_value(&model, l) {
                V3::False => {
                    any_false = true;
                    all_true = false;
                }
                V3::Unknown => {
                    all_true = false;
                    unknown_atoms.push(l.atom);
                }
                V3::True => {}
            }
        }

        if all_true {
            // Impossible([..]) with every literal TRUE → the constraint is violated.
            conflicts.push(Conflict {
                origin: clause.origin.clone(),
                atoms: clause.lits.iter().map(|l| label(c, l.atom)).collect(),
            });
        } else if any_false {
            // Some literal is FALSE → the literals cannot all be TRUE → satisfied.
        } else {
            // No FALSE literal, not all TRUE → an UNKNOWN blocks the check.
            // Implication axioms warn (missing required data); list axioms treat
            // UNKNOWN as "no conflict yet" and stay consistent.
            if clause.origin.kind == "AXIOM" {
                warnings.push(Warning {
                    origin: clause.origin.clone(),
                    blocked_by: unknown_atoms.iter().map(|a| label(c, *a)).collect(),
                });
            }
        }
    }

    // Deterministic ordering: by source then line.
    conflicts.sort_by_key(|c| key(&c.origin));
    warnings.sort_by_key(|w| key(&w.origin));

    let status = if !conflicts.is_empty() {
        Status::Conflict
    } else if !warnings.is_empty() {
        Status::Warning
    } else {
        Status::Consistent
    };

    Report {
        status,
        conflicts,
        warnings,
        derived,
    }
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
        write!(
            f,
            "SUMMARY: {} conflicts, {} warnings, {} derived",
            self.conflicts.len(),
            self.warnings.len(),
            self.derived.len()
        )
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
        let r = verify_source("<t>", "FACT A has flying\nAXIOM w:\n    WHEN A has flying\n    THEN A has wing\n").unwrap();
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
        let r = verify_source("<t>", "FACT A has flying\nRULE o:\n    WHEN A has flying\n    THEN A needs oxygen\n").unwrap();
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
