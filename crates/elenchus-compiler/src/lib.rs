//! elenchus-compiler — compiles parsed elenchus DSL into a canonical clause IR.
//!
//! This crate is **preparation, not solving**. It takes the AST (from
//! `elenchus-parser`) and produces a deterministic, solver-ready intermediate
//! representation:
//!
//! - **atom interner**: `(subject, predicate, object?)` → dense `u32` id,
//!   canonically sorted so ids (and any later enumeration) are deterministic;
//! - **desugaring**: surface CAPS sugar → `Impossible` clauses
//!   (`EXCLUSIVE` pairwise, `WHEN…THEN` → `Impossible([A, …, NOT C])`, etc.);
//! - **content-addressing** (sha256, mirroring vsm-guard's CAS): identical
//!   clauses are deduped (idempotent — `P ∧ P ≡ P`), and a named construct
//!   redefined with a different body is an `AxiomRedefinition` error.
//!
//! The actual reasoning (3-valued forward chaining, SAT, all-SAT, the WARNING
//! pool, the four results) belongs to the future `elenchus-solver` crate.
//!
//! `IMPORT` resolution (a source-agnostic `Resolver`, flat-merge into the shared
//! atom universe) lands next; for now imports are recorded as pending.
#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use elenchus_parser::{Atom, Body, ListOp, Literal, Statement};
use sha2::{Digest, Sha256};
use thiserror::Error;

// --- content-addressing (mirrors vsm-guard::hashing) -----------------------

/// SHA-256 content addressing. Used only for dedup / redefinition / provenance,
/// never for namespacing atoms.
pub fn hash_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

// --- IR types --------------------------------------------------------------

/// Dense atom identifier (also the SAT variable number).
pub type AtomId = u32;

/// The identity of an atom: the triple `(subject, predicate, object?)`, owned so
/// it survives across merged sources. Ordering is lexicographic → canonical.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AtomKey {
    pub subject: String,
    pub predicate: String,
    pub object: Option<String>,
}

impl AtomKey {
    fn from_atom(a: &Atom) -> Self {
        AtomKey {
            subject: a.subject.to_string(),
            predicate: a.predicate.to_string(),
            object: a.object.map(|o| o.to_string()),
        }
    }
}

/// A literal as it appears *inside* an `Impossible` clause: an atom, optionally
/// negated. `negated = true` means the literal is `NOT atom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lit {
    pub atom: AtomId,
    pub negated: bool,
}

/// A confident truth value. UNKNOWN is the *absence* of a fact, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    True,
    False,
}

/// Where a piece of IR came from — for readable conflict/warning pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub source: String,
    pub line: u32,
    pub axiom: Option<String>,
    pub kind: &'static str,
}

/// A confident fact (from `FACT` / `NOT`). Conflicting facts on the same atom
/// are preserved (both kept) — the solver reports that as a CONFLICT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub atom: AtomId,
    pub value: Value,
    pub origin: Origin,
}

/// An `Impossible` clause: the listed literals cannot all hold simultaneously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub lits: Vec<Lit>,
    pub origin: Origin,
}

/// A forward-chaining rule (from `RULE`): if all antecedent literals hold, derive
/// the consequent literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub antecedent: Vec<Lit>,
    pub consequent: Vec<Lit>,
    pub origin: Origin,
}

/// A `CHECK` query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub subject: Option<String>,
    pub bidirectional: bool,
}

/// The compiled IR: the solver's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// Indexed by [`AtomId`]; canonically sorted.
    pub atoms: Vec<AtomKey>,
    pub facts: Vec<Fact>,
    pub clauses: Vec<Clause>,
    pub rules: Vec<Rule>,
    pub checks: Vec<Check>,
    /// Imports seen but not yet resolved (resolution lands next).
    pub pending_imports: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("parse error in {file}: {message}")]
    Parse { file: String, message: String },
    #[error("'{name}' redefined with a different body")]
    AxiomRedefinition { name: String },
}

// --- raw (key-based) intermediate, before interning ------------------------

#[derive(Clone)]
struct RawLit {
    key: AtomKey,
    negated: bool,
}

struct RawFact {
    key: AtomKey,
    value: Value,
    origin: Origin,
}

struct RawClause {
    lits: Vec<RawLit>,
    origin: Origin,
}

struct RawRule {
    antecedent: Vec<RawLit>,
    consequent: Vec<RawLit>,
    origin: Origin,
}

// --- compiler --------------------------------------------------------------

/// Accumulates statements from one or more sources, then interns + emits the IR.
#[derive(Default)]
pub struct Compiler {
    keys: BTreeSet<AtomKey>,
    facts: Vec<RawFact>,
    clauses: Vec<RawClause>,
    rules: Vec<RawRule>,
    checks: Vec<Check>,
    pending_imports: Vec<String>,
    /// name → content hash of its body, for redefinition detection.
    defined: BTreeMap<String, String>,
    /// dedup of identical clauses by canonical content hash.
    clause_sigs: BTreeSet<String>,
    /// dedup of identical facts by (key, value).
    fact_sigs: BTreeSet<String>,
}

impl Compiler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one source and accumulate its statements. `source` is a label used
    /// in provenance (e.g. a file name or `"<root>"`).
    pub fn add_source(&mut self, source: &str, src: &str) -> Result<(), CompileError> {
        let program = elenchus_parser::parse(src).map_err(|e| CompileError::Parse {
            file: source.to_string(),
            message: e.message,
        })?;
        for stmt in &program.statements {
            self.add_statement(source, stmt)?;
        }
        Ok(())
    }

    fn add_statement(&mut self, source: &str, stmt: &Statement) -> Result<(), CompileError> {
        match stmt {
            Statement::Import(path) => {
                self.pending_imports.push(path.data.to_string());
            }
            Statement::Fact(a) => self.add_fact(source, a, Value::True, "FACT"),
            Statement::Negation(a) => self.add_fact(source, a, Value::False, "NOT"),
            Statement::Check {
                subject,
                bidirectional,
            } => self.checks.push(Check {
                subject: subject.as_ref().map(|s| s.data.to_string()),
                bidirectional: *bidirectional,
            }),
            Statement::Axiom { name, body } => {
                let line = name.span.location_line();
                self.add_named(source, name.data, line, body, false)?;
            }
            Statement::Rule { name, body } => {
                let line = name.span.location_line();
                self.add_named(source, name.data, line, body, true)?;
            }
        }
        Ok(())
    }

    fn intern(&mut self, key: &AtomKey) {
        if !self.keys.contains(key) {
            self.keys.insert(key.clone());
        }
    }

    fn add_fact(&mut self, source: &str, a: &elenchus_parser::Located<Atom>, value: Value, kind: &'static str) {
        let key = AtomKey::from_atom(&a.data);
        self.intern(&key);
        let sig = alloc::format!(
            "{}|{}|{}|{}",
            key_sig(&key),
            matches!(value, Value::True) as u8,
            kind,
            "" // facts dedup ignores line; identical FACT twice is idempotent
        );
        if !self.fact_sigs.insert(sig) {
            return; // exact duplicate fact — idempotent
        }
        self.facts.push(RawFact {
            key,
            value,
            origin: Origin {
                source: source.to_string(),
                line: a.span.location_line(),
                axiom: None,
                kind,
            },
        });
    }

    /// Handle a named construct (`AXIOM` or `RULE`). `is_rule` selects derivation
    /// vs checking. Returns an error on redefinition with a different body.
    fn add_named(
        &mut self,
        source: &str,
        name: &str,
        line: u32,
        body: &Body,
        is_rule: bool,
    ) -> Result<(), CompileError> {
        let body_hash = hash_hex(canonical_body(name, body, is_rule).as_bytes());
        match self.defined.get(name) {
            Some(prev) if *prev == body_hash => return Ok(()), // identical → idempotent
            Some(_) => {
                return Err(CompileError::AxiomRedefinition {
                    name: name.to_string(),
                });
            }
            None => {
                self.defined.insert(name.to_string(), body_hash);
            }
        }

        if is_rule {
            // RULE always has an implication body (guaranteed by the grammar).
            if let Body::Impl {
                antecedent,
                consequent,
            } = body
            {
                let (ante, cons) = (raw_lits(antecedent), raw_lits(consequent));
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                self.rules.push(RawRule {
                    antecedent: ante,
                    consequent: cons,
                    origin: self.origin(source, line, Some(name), "RULE"),
                });
            }
            return Ok(());
        }

        match body {
            Body::List { op, atoms } => {
                let keys: Vec<AtomKey> = atoms.iter().map(|a| AtomKey::from_atom(&a.data)).collect();
                for k in &keys {
                    self.intern(k);
                }
                let kind = list_kind(*op);
                let origin = self.origin(source, line, Some(name), kind);
                match op {
                    // EXCLUSIVE / FORBIDS: "at most one" → pairwise Impossible([a_i, a_j]).
                    ListOp::Exclusive | ListOp::Forbids => {
                        self.emit_pairwise(&keys, &origin);
                    }
                    // ONEOF: pairwise (at most one) + at-least-one.
                    ListOp::OneOf => {
                        self.emit_pairwise(&keys, &origin);
                        self.emit_at_least_one(&keys, &origin);
                    }
                    // ATLEAST: Impossible([NOT a_1, …, NOT a_n]).
                    ListOp::AtLeast => {
                        self.emit_at_least_one(&keys, &origin);
                    }
                }
            }
            Body::Impl {
                antecedent,
                consequent,
            } => {
                // A1 ∧ … ∧ An → C  ==  Impossible([A1, …, An, NOT C]) for each consequent C.
                let ante = raw_lits(antecedent);
                for l in &ante {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), "AXIOM");
                for c in consequent {
                    let neg_c = RawLit {
                        key: AtomKey::from_atom(&c.data.atom),
                        negated: !c.data.negated,
                    };
                    self.intern(&neg_c.key);
                    let mut lits = ante.clone();
                    lits.push(neg_c);
                    self.push_clause(lits, origin.clone());
                }
            }
        }
        Ok(())
    }

    fn emit_pairwise(&mut self, keys: &[AtomKey], origin: &Origin) {
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                let lits = vec![
                    RawLit {
                        key: keys[i].clone(),
                        negated: false,
                    },
                    RawLit {
                        key: keys[j].clone(),
                        negated: false,
                    },
                ];
                self.push_clause(lits, origin.clone());
            }
        }
    }

    fn emit_at_least_one(&mut self, keys: &[AtomKey], origin: &Origin) {
        let lits = keys
            .iter()
            .map(|k| RawLit {
                key: k.clone(),
                negated: true,
            })
            .collect();
        self.push_clause(lits, origin.clone());
    }

    fn push_clause(&mut self, lits: Vec<RawLit>, origin: Origin) {
        let sig = clause_sig(&lits);
        if self.clause_sigs.insert(sig) {
            self.clauses.push(RawClause { lits, origin });
        }
        // else: identical clause already present — idempotent.
    }

    fn origin(&self, source: &str, line: u32, axiom: Option<&str>, kind: &'static str) -> Origin {
        Origin {
            source: source.to_string(),
            line,
            axiom: axiom.map(|s| s.to_string()),
            kind,
        }
    }

    /// Intern all atoms (canonical sort), then lower the raw IR to ids.
    pub fn finalize(self) -> Compiled {
        let atoms: Vec<AtomKey> = self.keys.into_iter().collect(); // BTreeSet → sorted
        let mut id_of: BTreeMap<AtomKey, AtomId> = BTreeMap::new();
        for (i, k) in atoms.iter().enumerate() {
            id_of.insert(k.clone(), i as AtomId);
        }
        let lower = |l: &RawLit| Lit {
            atom: id_of[&l.key],
            negated: l.negated,
        };

        let facts = self
            .facts
            .into_iter()
            .map(|f| Fact {
                atom: id_of[&f.key],
                value: f.value,
                origin: f.origin,
            })
            .collect();
        let clauses = self
            .clauses
            .into_iter()
            .map(|c| Clause {
                lits: c.lits.iter().map(lower).collect(),
                origin: c.origin,
            })
            .collect();
        let rules = self
            .rules
            .into_iter()
            .map(|r| Rule {
                antecedent: r.antecedent.iter().map(lower).collect(),
                consequent: r.consequent.iter().map(lower).collect(),
                origin: r.origin,
            })
            .collect();

        Compiled {
            atoms,
            facts,
            clauses,
            rules,
            checks: self.checks,
            pending_imports: self.pending_imports,
        }
    }
}

/// Convenience: compile a single source into the IR.
pub fn compile_source(source: &str, src: &str) -> Result<Compiled, CompileError> {
    let mut c = Compiler::new();
    c.add_source(source, src)?;
    Ok(c.finalize())
}

// --- helpers ---------------------------------------------------------------

fn raw_lits(lits: &[elenchus_parser::Located<Literal>]) -> Vec<RawLit> {
    lits.iter()
        .map(|l| RawLit {
            key: AtomKey::from_atom(&l.data.atom),
            negated: l.data.negated,
        })
        .collect()
}

fn list_kind(op: ListOp) -> &'static str {
    match op {
        ListOp::Exclusive => "EXCLUSIVE",
        ListOp::Forbids => "FORBIDS",
        ListOp::OneOf => "ONEOF",
        ListOp::AtLeast => "ATLEAST",
    }
}

fn key_sig(k: &AtomKey) -> String {
    alloc::format!(
        "{}|{}|{}",
        k.subject,
        k.predicate,
        k.object.as_deref().unwrap_or("")
    )
}

/// Canonical, order-independent signature of a clause's literals (for dedup).
fn clause_sig(lits: &[RawLit]) -> String {
    let mut parts: Vec<String> = lits
        .iter()
        .map(|l| alloc::format!("{}|{}", key_sig(&l.key), l.negated as u8))
        .collect();
    parts.sort();
    parts.dedup();
    parts.join(";")
}

/// Canonical body string for a named construct, hashed for redefinition checks.
fn canonical_body(name: &str, body: &Body, is_rule: bool) -> String {
    let mut s = String::new();
    let _ = write!(s, "{}|{}|", if is_rule { "RULE" } else { "AXIOM" }, name);
    match body {
        Body::List { op, atoms } => {
            let _ = write!(s, "LIST|{}|", list_kind(*op));
            let mut keys: Vec<String> = atoms
                .iter()
                .map(|a| key_sig(&AtomKey::from_atom(&a.data)))
                .collect();
            keys.sort();
            s.push_str(&keys.join(";"));
        }
        Body::Impl {
            antecedent,
            consequent,
        } => {
            s.push_str("IMPL|ANTE|");
            s.push_str(&lit_sigs(antecedent));
            s.push_str("|CONS|");
            s.push_str(&lit_sigs(consequent));
        }
    }
    s
}

fn lit_sigs(lits: &[elenchus_parser::Located<Literal>]) -> String {
    let mut parts: Vec<String> = lits
        .iter()
        .map(|l| {
            alloc::format!(
                "{}|{}",
                key_sig(&AtomKey::from_atom(&l.data.atom)),
                l.data.negated as u8
            )
        })
        .collect();
    parts.sort();
    parts.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(subject: &str, predicate: &str, object: Option<&str>) -> AtomKey {
        AtomKey {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.map(|o| o.to_string()),
        }
    }

    fn id(c: &Compiled, k: &AtomKey) -> AtomId {
        c.atoms.iter().position(|a| a == k).unwrap() as AtomId
    }

    #[test]
    fn exclusive_unfolds_pairwise() {
        let src = "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\n        x c\n";
        let c = compile_source("<t>", src).unwrap();
        // C(3,2) = 3 clauses, each of 2 positive literals.
        assert_eq!(c.clauses.len(), 3);
        for cl in &c.clauses {
            assert_eq!(cl.lits.len(), 2);
            assert!(cl.lits.iter().all(|l| !l.negated));
        }
    }

    #[test]
    fn implication_negates_consequent() {
        // WHEN x a THEN x b  ==  Impossible([x a, NOT x b])
        let src = "AXIOM r:\n    WHEN x a\n    THEN x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        let cl = &c.clauses[0];
        assert_eq!(cl.lits.len(), 2);
        let a = id(&c, &key("x", "a", None));
        let b = id(&c, &key("x", "b", None));
        assert!(cl.lits.contains(&Lit { atom: a, negated: false }));
        assert!(cl.lits.contains(&Lit { atom: b, negated: true }));
    }

    #[test]
    fn negated_consequent_flips_to_positive() {
        // THEN NOT x b  →  NOT(NOT x b) = x b positive inside Impossible
        let src = "AXIOM r:\n    WHEN x a\n    THEN NOT x b\n";
        let c = compile_source("<t>", src).unwrap();
        let b = id(&c, &key("x", "b", None));
        assert!(c.clauses[0].lits.contains(&Lit { atom: b, negated: false }));
    }

    #[test]
    fn oneof_is_pairwise_plus_at_least_one() {
        let src = "AXIOM o:\n    ONEOF\n        x a\n        x b\n";
        let c = compile_source("<t>", src).unwrap();
        // pairwise C(2,2)=1 + 1 at-least-one = 2 clauses
        assert_eq!(c.clauses.len(), 2);
        // the at-least-one clause is the all-negated one
        assert!(c.clauses.iter().any(|cl| cl.lits.iter().all(|l| l.negated)));
    }

    #[test]
    fn atleast_is_one_negated_clause() {
        let src = "AXIOM a:\n    ATLEAST\n        x a\n        x b\n        x c\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        assert_eq!(c.clauses[0].lits.len(), 3);
        assert!(c.clauses[0].lits.iter().all(|l| l.negated));
    }

    #[test]
    fn rules_are_separate_from_clauses() {
        let src = "RULE needs:\n    WHEN x a\n    THEN x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 0);
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].antecedent.len(), 1);
        assert_eq!(c.rules[0].consequent.len(), 1);
    }

    #[test]
    fn atoms_are_canonically_sorted() {
        let src = "FACT z z\nFACT a a\nFACT m m\n";
        let c = compile_source("<t>", src).unwrap();
        let mut sorted = c.atoms.clone();
        sorted.sort();
        assert_eq!(c.atoms, sorted);
    }

    #[test]
    fn duplicate_axiom_is_idempotent() {
        let src = "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
    }

    #[test]
    fn redefinition_with_different_body_errors() {
        let src = "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x c\n";
        let err = compile_source("<t>", src).unwrap_err();
        assert_eq!(err, CompileError::AxiomRedefinition { name: "e".to_string() });
    }

    #[test]
    fn duplicate_fact_is_idempotent() {
        let c = compile_source("<t>", "FACT x a\nFACT x a\n").unwrap();
        assert_eq!(c.facts.len(), 1);
    }

    #[test]
    fn conflicting_facts_are_both_kept() {
        // FACT X + NOT X is a CONFLICT for the solver, not a compile error.
        let c = compile_source("<t>", "FACT x a\nNOT x a\n").unwrap();
        assert_eq!(c.facts.len(), 2);
    }

    #[test]
    fn import_is_recorded_pending() {
        let c = compile_source("<t>", "IMPORT \"physics.vrf\"\nFACT x a\n").unwrap();
        assert_eq!(c.pending_imports, vec!["physics.vrf".to_string()]);
    }

    #[test]
    fn creature_example_compiles() {
        let src = include_str!("../../../docs/examples/creature.vrf");
        let c = compile_source("creature.vrf", src).unwrap();
        assert_eq!(c.facts.len(), 2); // flying, warm_blood
        assert_eq!(c.rules.len(), 1); // needs_oxygen
        assert_eq!(c.checks.len(), 1);
        // fly_xor_swim (1) + wings_need_bone (THEN wing AND bone → 2) + no_dual_temp (1) = 4
        assert_eq!(c.clauses.len(), 4);
        assert_eq!(c.atoms.len(), 7);
    }
}
