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
//!   redefined with a different body is a `PremiseRedefinition` error.
//!
//! The actual reasoning (3-valued forward chaining, SAT, all-SAT, the WARNING
//! pool, the four results) belongs to the future `elenchus-solver` crate.
//!
//! `IMPORT` resolution (a source-agnostic `Resolver`, flat-merge into the shared
//! atom universe) lands next; for now imports are recorded as pending.
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use elenchus_parser::{Atom, Body, Conn, ListOp, Literal, Statement};
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
    /// The entity the claim is about (owned copy of the parser's `subject`).
    pub subject: String,
    /// The relation or property asserted.
    pub predicate: String,
    /// Optional object; part of identity, so `has flying` ≠ `has swimming`.
    pub object: Option<String>,
}

impl AtomKey {
    /// Owned copy of a borrowed parser [`Atom`].
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
    /// Interned id of the atom (also its SAT variable number).
    pub atom: AtomId,
    /// `true` means this literal is `NOT atom` inside the clause.
    pub negated: bool,
}

/// A confident truth value. UNKNOWN is the *absence* of a fact, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// The atom is asserted TRUE (from `FACT`).
    True,
    /// The atom is asserted FALSE (from `NOT`).
    False,
}

/// Where a piece of IR came from — for readable conflict/warning pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// The source label this came from (file name or `"<root>"`/`"<text>"`).
    pub source: String,
    /// 1-based line number of the originating statement.
    pub line: u32,
    /// The premise/rule name, if it came from a named construct.
    pub premise: Option<String>,
    /// Surface kind for the report, e.g. `"FACT"`, `"EXCLUSIVE"`, `"PREMISE"`.
    pub kind: &'static str,
}

/// A confident fact (from `FACT` / `NOT`). Conflicting facts on the same atom
/// are preserved (both kept) — the solver reports that as a CONFLICT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// The atom this fact pins down.
    pub atom: AtomId,
    /// The asserted truth value.
    pub value: Value,
    /// Where it came from (for the report).
    pub origin: Origin,
}

/// An `Impossible` clause: the listed literals cannot all hold simultaneously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    /// The literals that cannot all hold at once (an `Impossible([...])`).
    pub lits: Vec<Lit>,
    /// Where it came from (for the report).
    pub origin: Origin,
}

/// A forward-chaining rule (from `RULE`): if all antecedent literals hold, derive
/// the consequent literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Literals that must all hold for the rule to fire.
    pub antecedent: Vec<Lit>,
    /// Literals derived (asserted) when the antecedent holds.
    pub consequent: Vec<Lit>,
    /// Where it came from (for the report).
    pub origin: Origin,
}

/// A `CHECK` query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Restrict the report to this subject; `None` means check everything.
    pub subject: Option<String>,
    /// `true` runs the backward (all-SAT) pass to detect UNDERDETERMINED.
    pub bidirectional: bool,
}

/// The compiled IR: the solver's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// Indexed by [`AtomId`]; canonically sorted.
    pub atoms: Vec<AtomKey>,
    /// Confident assertions from `FACT`/`NOT`.
    pub facts: Vec<Fact>,
    /// `Impossible` clauses (desugared premises + the built-in non-contradiction).
    pub clauses: Vec<Clause>,
    /// Forward-chaining rules from `RULE`.
    pub rules: Vec<Rule>,
    /// `CHECK` queries.
    pub checks: Vec<Check>,
    /// Imports seen but not yet resolved (only populated by [`compile_source`];
    /// [`compile`] resolves them, leaving this empty).
    pub pending_imports: Vec<String>,
}

/// Anything that can go wrong while compiling (and resolving imports).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    /// A source failed to parse; carries the file label and the parser message.
    #[error("parse error in {file}: {message}")]
    Parse {
        /// The source label that failed to parse.
        file: String,
        /// The parser's error message.
        message: String,
    },
    /// A name was reused with a different body *within the same source*.
    #[error("'{name}' redefined with a different body")]
    PremiseRedefinition {
        /// The clashing premise/rule name.
        name: String,
    },
    /// An `IMPORT` target could not be loaded by the [`Resolver`].
    #[error("import not found: {0}")]
    ImportNotFound(String),
    /// Imports form a cycle (a source transitively imports itself).
    #[error("circular import: {0}")]
    CircularImport(String),
    /// A `RULE` used `OR` in its `THEN`: forward chaining cannot derive a
    /// disjunction (it would not know which literal to assert). Model it as a
    /// `PREMISE` constraint instead.
    #[error("rule '{name}' cannot derive a disjunction (OR in THEN); use a PREMISE instead")]
    RuleDisjunctiveConsequent {
        /// The offending rule name.
        name: String,
    },
}

// --- raw (key-based) intermediate, before interning ------------------------
// While accumulating we key everything by `AtomKey` (the owned triple) rather
// than by `AtomId`, because ids only become stable once *all* sources are merged
// and the atom set is sorted in `finalize`. These mirror the public IR types but
// hold keys instead of ids.

/// A literal keyed by atom identity (pre-interning counterpart of [`Lit`]).
#[derive(Clone)]
struct RawLit {
    key: AtomKey,
    negated: bool,
}

/// A fact keyed by atom identity (pre-interning counterpart of [`Fact`]).
struct RawFact {
    key: AtomKey,
    value: Value,
    origin: Origin,
}

/// A clause keyed by atom identity (pre-interning counterpart of [`Clause`]).
struct RawClause {
    lits: Vec<RawLit>,
    origin: Origin,
}

/// A rule keyed by atom identity (pre-interning counterpart of [`Rule`]).
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
    /// (source, name) → content hash of its body, for redefinition detection.
    /// Scoped per source: premise/rule names are labels, not global identifiers,
    /// so different files (domains) may reuse a name. A clash is only an error
    /// *within the same source*.
    defined: BTreeMap<(String, String), String>,
    /// dedup of identical clauses by canonical content hash.
    clause_sigs: BTreeSet<String>,
    /// dedup of identical facts by (key, value).
    fact_sigs: BTreeSet<String>,
}

impl Compiler {
    /// A fresh, empty compiler.
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

    /// Resolve and flat-merge `path` and its transitive imports.
    /// `visited` holds content hashes already merged (dedup); `stack` holds the
    /// hashes on the current path (cycle detection).
    fn load_recursive<R: Resolver>(
        &mut self,
        path: &str,
        resolver: &R,
        visited: &mut BTreeSet<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), CompileError> {
        let content = resolver.load(path)?;
        let hash = hash_hex(content.as_bytes());
        if visited.contains(&hash) {
            return Ok(()); // already merged — idempotent
        }
        if stack.contains(&hash) {
            return Err(CompileError::CircularImport(path.to_string()));
        }
        stack.push(hash.clone());

        let program = elenchus_parser::parse(&content).map_err(|e| CompileError::Parse {
            file: path.to_string(),
            message: e.message,
        })?;
        for stmt in &program.statements {
            if let Statement::Import(p) = stmt {
                let resolved = resolver.resolve(path, p.data);
                self.load_recursive(&resolved, resolver, visited, stack)?;
            } else {
                self.add_statement(path, stmt)?;
            }
        }

        stack.pop();
        visited.insert(hash);
        Ok(())
    }

    /// Route one statement to the right accumulator (facts, checks, named
    /// constructs); `IMPORT` is only recorded as pending here (see [`compile`]
    /// / [`load_recursive`] for actual resolution).
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
            Statement::Premise { name, body } => {
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

    /// Record an atom identity in the shared universe (deduped by the `BTreeSet`).
    fn intern(&mut self, key: &AtomKey) {
        if !self.keys.contains(key) {
            self.keys.insert(key.clone());
        }
    }

    /// Accumulate a `FACT`/`NOT`; exact duplicates (same key+value+kind) are
    /// dropped as idempotent, while a `FACT` and a `NOT` on the same atom are
    /// both kept so the solver can report the CONFLICT.
    fn add_fact(
        &mut self,
        source: &str,
        a: &elenchus_parser::Located<Atom>,
        value: Value,
        kind: &'static str,
    ) {
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
                premise: None,
                kind,
            },
        });
    }

    /// Handle a named construct (`PREMISE` or `RULE`). `is_rule` selects derivation
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
        let key = (source.to_string(), name.to_string());
        match self.defined.get(&key) {
            Some(prev) if *prev == body_hash => return Ok(()), // identical → idempotent
            Some(_) => {
                // Same name + different body *in the same source* — a real mistake.
                return Err(CompileError::PremiseRedefinition {
                    name: name.to_string(),
                });
            }
            None => {
                self.defined.insert(key, body_hash);
            }
        }

        if is_rule {
            // RULE always has an implication body (guaranteed by the grammar).
            if let Body::Impl {
                antecedent,
                ante_conn,
                consequent,
                cons_conn,
            } = body
            {
                // A rule *derives* its consequent; an `OR` consequent is not a
                // single fact to assert, so reject it (use a PREMISE instead).
                if *cons_conn == Conn::Or {
                    return Err(CompileError::RuleDisjunctiveConsequent {
                        name: name.to_string(),
                    });
                }
                let (ante, cons) = (raw_lits(antecedent), raw_lits(consequent));
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), "RULE");
                match ante_conn {
                    // a ∧ b → C : one rule firing on the whole antecedent.
                    Conn::And => self.rules.push(RawRule {
                        antecedent: ante,
                        consequent: cons,
                        origin,
                    }),
                    // (a ∨ b) → C == (a → C) ∧ (b → C): one rule per antecedent.
                    Conn::Or => {
                        for a in &ante {
                            self.rules.push(RawRule {
                                antecedent: vec![a.clone()],
                                consequent: cons.clone(),
                                origin: origin.clone(),
                            });
                        }
                    }
                }
            }
            return Ok(());
        }

        match body {
            Body::List { op, atoms } => {
                let keys: Vec<AtomKey> =
                    atoms.iter().map(|a| AtomKey::from_atom(&a.data)).collect();
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
                ante_conn,
                consequent,
                cons_conn,
            } => {
                // Implication A → C as `Impossible(A_true ∧ ¬C)`. We group each
                // side by its connective and emit one clause per (ante × cons)
                // group pair — a uniform rule covering all AND/OR combinations:
                //   AND-ante → all its literals share every clause;
                //   OR-ante  → one clause per literal;
                //   AND-cons → one clause per (negated) literal;
                //   OR-cons  → all its (negated) literals share every clause.
                let ante = raw_lits(antecedent);
                let cons = raw_lits(consequent);
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), "PREMISE");

                let ante_groups: Vec<Vec<RawLit>> = match ante_conn {
                    Conn::And => vec![ante.clone()],
                    Conn::Or => ante.iter().map(|l| vec![l.clone()]).collect(),
                };
                let cons_groups: Vec<Vec<RawLit>> = match cons_conn {
                    Conn::And => cons.iter().map(|l| vec![l.clone()]).collect(),
                    Conn::Or => vec![cons.clone()],
                };
                for ag in &ante_groups {
                    for cg in &cons_groups {
                        let mut lits = ag.clone();
                        for c in cg {
                            lits.push(RawLit {
                                key: c.key.clone(),
                                negated: !c.negated,
                            });
                        }
                        self.push_clause(lits, origin.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Emit "at most one TRUE" as one `Impossible([a_i, a_j])` per unordered
    /// pair. Pairwise (not a single big clause) because `Impossible([a,b,c])`
    /// only forbids *all three* together — it would still allow two.
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

    /// Emit "at least one TRUE" as a single `Impossible([NOT a_1, …, NOT a_n])`
    /// — it is impossible for all of them to be false at once.
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

    /// Append a clause unless an identical one (by canonical [`clause_sig`]) is
    /// already present — `P ∧ P ≡ P`, so dedup keeps the IR minimal.
    fn push_clause(&mut self, lits: Vec<RawLit>, origin: Origin) {
        let sig = clause_sig(&lits);
        if self.clause_sigs.insert(sig) {
            self.clauses.push(RawClause { lits, origin });
        }
        // else: identical clause already present — idempotent.
    }

    /// Build an [`Origin`] for provenance from the current source/line/name.
    fn origin(&self, source: &str, line: u32, premise: Option<&str>, kind: &'static str) -> Origin {
        Origin {
            source: source.to_string(),
            line,
            premise: premise.map(|s| s.to_string()),
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

/// Convenience: compile a single source into the IR. `IMPORT`s are recorded as
/// pending, not resolved (use [`compile`] with a [`Resolver`] to resolve them).
pub fn compile_source(source: &str, src: &str) -> Result<Compiled, CompileError> {
    let mut c = Compiler::new();
    c.add_source(source, src)?;
    Ok(c.finalize())
}

// --- import resolution (source-agnostic) -----------------------------------

/// Resolves `IMPORT "path"` to source text. The engine is source-agnostic: it
/// consumes strings, so a file is merely one backing store. Mirrors
/// vsm-grammar's `SourceResolver`.
pub trait Resolver {
    /// Load the raw source text for a resolved path.
    fn load(&self, path: &str) -> Result<String, CompileError>;

    /// Normalize `relative` against the importing source `base`.
    /// Default: paths are absolute names, returned unchanged.
    fn resolve(&self, _base: &str, relative: &str) -> String {
        relative.to_string()
    }
}

/// An in-memory resolver: serves sources from a name → content map. Pure
/// `no_std`. Mirrors vsm-grammar's `MemoryResolver`.
#[derive(Default)]
pub struct MemoryResolver {
    sources: BTreeMap<String, String>,
}

impl MemoryResolver {
    /// An empty resolver with no sources.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `content` under `path`; returns `&mut self` for chaining.
    pub fn add(&mut self, path: &str, content: &str) -> &mut Self {
        self.sources.insert(path.to_string(), content.to_string());
        self
    }
}

impl Resolver for MemoryResolver {
    fn load(&self, path: &str) -> Result<String, CompileError> {
        self.sources
            .get(path)
            .cloned()
            .ok_or_else(|| CompileError::ImportNotFound(path.to_string()))
    }
}

/// A filesystem-backed resolver. Mirrors vsm-grammar's `FileResolver`:
/// relative imports resolve against the importing file's directory, with manual
/// `..` normalization (no canonicalization, to keep a virtual layout).
#[cfg(feature = "std")]
pub struct FileResolver;

#[cfg(feature = "std")]
impl Resolver for FileResolver {
    fn load(&self, path: &str) -> Result<String, CompileError> {
        std::fs::read_to_string(path)
            .map_err(|e| CompileError::ImportNotFound(alloc::format!("{}: {}", path, e)))
    }

    fn resolve(&self, base: &str, relative: &str) -> String {
        use std::path::{Component, Path, PathBuf};
        let parent = Path::new(base).parent().unwrap_or_else(|| Path::new("."));
        let joined = parent.join(relative);
        let mut out = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        // Normalize to forward slashes so resolved paths (and therefore the
        // provenance recorded in the IR) are identical on Windows and Unix.
        // Windows `std::fs` accepts `/` just fine.
        out.to_string_lossy().replace('\\', "/")
    }
}

/// Compile a root source and all its transitive `IMPORT`s into one IR.
///
/// Imports are flat-merged into a single shared atom universe (atoms unify by
/// identity across files). Sources are content-addressed (sha256): a source
/// already merged is skipped (dedup), and an import cycle is an error.
///
/// Premise/rule names are per-source labels, not global identifiers: different
/// files (domains) may reuse a name, and the report qualifies them by source. A
/// name reused with a different body is an error only *within the same source*.
pub fn compile<R: Resolver>(root: &str, resolver: &R) -> Result<Compiled, CompileError> {
    let mut c = Compiler::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    c.load_recursive(root, resolver, &mut visited, &mut stack)?;
    Ok(c.finalize())
}

// --- helpers ---------------------------------------------------------------

/// Lower parsed, located literals to key-based [`RawLit`]s (drops spans).
fn raw_lits(lits: &[elenchus_parser::Located<Literal>]) -> Vec<RawLit> {
    lits.iter()
        .map(|l| RawLit {
            key: AtomKey::from_atom(&l.data.atom),
            negated: l.data.negated,
        })
        .collect()
}

/// The surface keyword for a list op, used as [`Origin::kind`] in the report.
fn list_kind(op: ListOp) -> &'static str {
    match op {
        ListOp::Exclusive => "EXCLUSIVE",
        ListOp::Forbids => "FORBIDS",
        ListOp::OneOf => "ONEOF",
        ListOp::AtLeast => "ATLEAST",
    }
}

/// Stable `subject|predicate|object` string for an atom key (the unit from which
/// clause/fact/body signatures are built).
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
    let _ = write!(s, "{}|{}|", if is_rule { "RULE" } else { "PREMISE" }, name);
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
            ante_conn,
            consequent,
            cons_conn,
        } => {
            let conn = |c: &Conn| if *c == Conn::Or { "OR" } else { "AND" };
            s.push_str("IMPL|ANTE|");
            s.push_str(conn(ante_conn));
            s.push('|');
            s.push_str(&lit_sigs(antecedent));
            s.push_str("|CONS|");
            s.push_str(conn(cons_conn));
            s.push('|');
            s.push_str(&lit_sigs(consequent));
        }
    }
    s
}

/// Sorted `key|negated` signature of a literal list (order-independent), used
/// inside [`canonical_body`] so reordering a body does not look like a redefinition.
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
        let src = "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\n        x c\n";
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
        let src = "PREMISE r:\n    WHEN x a\n    THEN x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        let cl = &c.clauses[0];
        assert_eq!(cl.lits.len(), 2);
        let a = id(&c, &key("x", "a", None));
        let b = id(&c, &key("x", "b", None));
        assert!(cl.lits.contains(&Lit {
            atom: a,
            negated: false
        }));
        assert!(cl.lits.contains(&Lit {
            atom: b,
            negated: true
        }));
    }

    #[test]
    fn negated_consequent_flips_to_positive() {
        // THEN NOT x b  →  NOT(NOT x b) = x b positive inside Impossible
        let src = "PREMISE r:\n    WHEN x a\n    THEN NOT x b\n";
        let c = compile_source("<t>", src).unwrap();
        let b = id(&c, &key("x", "b", None));
        assert!(c.clauses[0].lits.contains(&Lit {
            atom: b,
            negated: false
        }));
    }

    #[test]
    fn consequent_or_is_one_clause_with_all_negated() {
        // WHEN x p THEN x a OR x b  ==  Impossible([x p, NOT x a, NOT x b])
        let src = "PREMISE r:\n    WHEN x p\n    THEN x a\n    OR x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        let cl = &c.clauses[0];
        assert_eq!(cl.lits.len(), 3);
        let p = id(&c, &key("x", "p", None));
        let a = id(&c, &key("x", "a", None));
        let b = id(&c, &key("x", "b", None));
        assert!(cl.lits.contains(&Lit {
            atom: p,
            negated: false
        }));
        assert!(cl.lits.contains(&Lit {
            atom: a,
            negated: true
        }));
        assert!(cl.lits.contains(&Lit {
            atom: b,
            negated: true
        }));
    }

    #[test]
    fn antecedent_or_is_one_clause_per_disjunct() {
        // WHEN x a OR x b THEN x c
        //   == Impossible([x a, NOT x c]) ∧ Impossible([x b, NOT x c])
        let src = "PREMISE r:\n    WHEN x a\n    OR x b\n    THEN x c\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 2);
        let a = id(&c, &key("x", "a", None));
        let b = id(&c, &key("x", "b", None));
        let cc = id(&c, &key("x", "c", None));
        // every clause has exactly two lits and carries NOT c
        for cl in &c.clauses {
            assert_eq!(cl.lits.len(), 2);
            assert!(cl.lits.contains(&Lit {
                atom: cc,
                negated: true
            }));
        }
        let has = |atom| {
            c.clauses.iter().any(|cl| {
                cl.lits.contains(&Lit {
                    atom,
                    negated: false,
                })
            })
        };
        assert!(has(a) && has(b));
    }

    #[test]
    fn antecedent_or_with_consequent_or_distributes() {
        // (a ∨ b) → (c ∨ d): Impossible([a,¬c,¬d]) ∧ Impossible([b,¬c,¬d])
        let src = "PREMISE r:\n    WHEN x a\n    OR x b\n    THEN x c\n    OR x d\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 2);
        for cl in &c.clauses {
            assert_eq!(cl.lits.len(), 3);
        }
    }

    #[test]
    fn rule_with_or_antecedent_splits_into_two_rules() {
        // (a ∨ b) → c derives c whenever either fires: two single-antecedent rules.
        let src = "RULE r:\n    WHEN x a\n    OR x b\n    THEN x c\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.rules.len(), 2);
        assert!(
            c.rules
                .iter()
                .all(|r| r.antecedent.len() == 1 && r.consequent.len() == 1)
        );
    }

    #[test]
    fn rule_with_or_consequent_is_rejected() {
        // A rule cannot derive a disjunction — must be a PREMISE.
        let src = "RULE r:\n    WHEN x a\n    THEN x b\n    OR x c\n";
        let err = compile_source("<t>", src).unwrap_err();
        assert!(matches!(
            err,
            CompileError::RuleDisjunctiveConsequent { .. }
        ));
    }

    #[test]
    fn oneof_is_pairwise_plus_at_least_one() {
        let src = "PREMISE o:\n    ONEOF\n        x a\n        x b\n";
        let c = compile_source("<t>", src).unwrap();
        // pairwise C(2,2)=1 + 1 at-least-one = 2 clauses
        assert_eq!(c.clauses.len(), 2);
        // the at-least-one clause is the all-negated one
        assert!(c.clauses.iter().any(|cl| cl.lits.iter().all(|l| l.negated)));
    }

    #[test]
    fn atleast_is_one_negated_clause() {
        let src = "PREMISE a:\n    ATLEAST\n        x a\n        x b\n        x c\n";
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
    fn duplicate_premise_is_idempotent() {
        let src = "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nPREMISE e:\n    EXCLUSIVE\n        x a\n        x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
    }

    #[test]
    fn redefinition_with_different_body_errors() {
        let src = "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nPREMISE e:\n    EXCLUSIVE\n        x a\n        x c\n";
        let err = compile_source("<t>", src).unwrap_err();
        assert_eq!(
            err,
            CompileError::PremiseRedefinition {
                name: "e".to_string()
            }
        );
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
    fn import_flat_merges_and_atoms_unify() {
        // The library constrains `Engine.X has fuel`; the main file asserts it.
        // After merge the atom must be ONE shared id (unification across files).
        let mut r = MemoryResolver::new();
        r.add(
            "lib.vrf",
            "PREMISE needs_fuel:\n    WHEN Engine.X has engine\n    THEN Engine.X has fuel\n",
        );
        r.add(
            "main.vrf",
            "IMPORT \"lib.vrf\"\nFACT Engine.X has engine\nFACT Engine.X has fuel\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert!(c.pending_imports.is_empty());
        assert_eq!(c.clauses.len(), 1); // the imported premise
        assert_eq!(c.facts.len(), 2);

        // `Engine.X has fuel` from the FACT and from the imported premise share an id.
        let fuel = key("Engine.X", "has", Some("fuel"));
        let fuel_id = id(&c, &fuel);
        assert!(c.facts.iter().any(|f| f.atom == fuel_id));
        assert!(c.clauses[0].lits.iter().any(|l| l.atom == fuel_id));
    }

    #[test]
    fn diamond_import_is_deduped() {
        // main → a, b ; a → base ; b → base. base merged once.
        let mut r = MemoryResolver::new();
        r.add(
            "base.vrf",
            "PREMISE b:\n    EXCLUSIVE\n        x a\n        x b\n",
        );
        r.add("a.vrf", "IMPORT \"base.vrf\"\n");
        r.add("c.vrf", "IMPORT \"base.vrf\"\n");
        r.add("main.vrf", "IMPORT \"a.vrf\"\nIMPORT \"c.vrf\"\n");
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.clauses.len(), 1); // base's single clause, not two
    }

    #[test]
    fn circular_import_errors() {
        let mut r = MemoryResolver::new();
        r.add("a.vrf", "IMPORT \"b.vrf\"\n");
        r.add("b.vrf", "IMPORT \"a.vrf\"\n");
        let err = compile("a.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::CircularImport(_)));
    }

    #[test]
    fn missing_import_errors() {
        let mut r = MemoryResolver::new();
        r.add("main.vrf", "IMPORT \"ghost.vrf\"\n");
        let err = compile("main.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::ImportNotFound(_)));
    }

    #[test]
    fn same_name_across_domains_coexists() {
        // Two files may legitimately reuse a premise NAME with different bodies
        // (different domains). Names are per-source labels — both premises apply,
        // and the report qualifies them by source. NOT a redefinition error.
        let mut r = MemoryResolver::new();
        r.add(
            "physics.vrf",
            "PREMISE safety:\n    EXCLUSIVE\n        x a\n        x b\n",
        );
        r.add(
            "main.vrf",
            "IMPORT \"physics.vrf\"\nPREMISE safety:\n    EXCLUSIVE\n        x a\n        x c\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.clauses.len(), 2); // a-b from physics, a-c from main
        assert!(c.clauses.iter().any(|cl| cl.origin.source == "physics.vrf"));
        assert!(c.clauses.iter().any(|cl| cl.origin.source == "main.vrf"));
    }

    #[test]
    fn two_libs_same_name_into_one_consumer() {
        // A.vrf and B.vrf each define their OWN `x` (different bodies); C imports
        // both. Both `x` coexist (per-source labels). Meanwhile the atom they
        // share (S has a) unifies into ONE id — names are scoped, atoms are not.
        let mut r = MemoryResolver::new();
        r.add(
            "A.vrf",
            "PREMISE x:\n    EXCLUSIVE\n        S has a\n        S has b\n",
        );
        r.add(
            "B.vrf",
            "PREMISE x:\n    EXCLUSIVE\n        S has a\n        S has c\n",
        );
        r.add("C.vrf", "IMPORT \"A.vrf\"\nIMPORT \"B.vrf\"\n");
        let c = compile("C.vrf", &r).unwrap();

        // both `x` premises contributed a clause, kept apart by source
        assert_eq!(c.clauses.len(), 2);
        assert!(
            c.clauses
                .iter()
                .any(|cl| cl.origin.source == "A.vrf" && cl.origin.premise.as_deref() == Some("x"))
        );
        assert!(
            c.clauses
                .iter()
                .any(|cl| cl.origin.source == "B.vrf" && cl.origin.premise.as_deref() == Some("x"))
        );

        // the shared atom `S has a` is a single interned id used by both clauses
        let s_a = id(&c, &key("S", "has", Some("a")));
        assert!(
            c.clauses
                .iter()
                .filter(|cl| cl.lits.iter().any(|l| l.atom == s_a))
                .count()
                == 2,
            "both clauses must reference the same unified `S has a` atom"
        );
    }

    #[test]
    fn redefinition_within_one_source_still_errors() {
        // But reusing a name with a different body *inside one source* is a mistake.
        let src = "PREMISE e:\n    EXCLUSIVE\n        x a\n        x b\nPREMISE e:\n    EXCLUSIVE\n        x a\n        x c\n";
        let err = compile_source("main.vrf", src).unwrap_err();
        assert_eq!(
            err,
            CompileError::PremiseRedefinition {
                name: "e".to_string()
            }
        );
    }

    #[test]
    fn import_demo_examples_resolve() {
        let mut r = MemoryResolver::new();
        r.add(
            "physics.vrf",
            include_str!("../../../docs/examples/physics.vrf"),
        );
        r.add(
            "import-demo.vrf",
            include_str!("../../../docs/examples/import-demo.vrf"),
        );
        let c = compile("import-demo.vrf", &r).unwrap();
        assert!(c.pending_imports.is_empty());
        // physics.vrf: one_path (EXCLUSIVE, 1 clause) + speed_order (impl, 1 clause)
        assert_eq!(c.clauses.len(), 2);
        // over_200 / over_100 unify between the facts and the imported premise.
        let over_100 = id(&c, &key("Motor", "over_100", None));
        assert!(c.facts.iter().any(|f| f.atom == over_100));
        assert!(
            c.clauses
                .iter()
                .any(|cl| cl.lits.iter().any(|l| l.atom == over_100))
        );
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

    #[test]
    fn forbids_unfolds_pairwise() {
        let src = "PREMISE f:\n    FORBIDS\n        x a\n        x b\n        x c\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 3); // C(3,2), like EXCLUSIVE
        assert!(
            c.clauses
                .iter()
                .all(|cl| cl.lits.len() == 2 && cl.lits.iter().all(|l| !l.negated))
        );
    }

    #[test]
    fn rule_with_multiple_consequents() {
        let src = "RULE r:\n    WHEN x a\n    THEN x b\n    AND  x c\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].consequent.len(), 2);
    }

    #[test]
    fn negated_antecedent_literal_keeps_polarity() {
        // WHEN NOT x a THEN x b  ==  Impossible([NOT x a, NOT x b])
        let src = "PREMISE a:\n    WHEN NOT x a\n    THEN x b\n";
        let c = compile_source("<t>", src).unwrap();
        let xa = id(&c, &key("x", "a", None));
        assert!(c.clauses[0].lits.contains(&Lit {
            atom: xa,
            negated: true
        }));
    }

    #[test]
    fn rule_keeps_consequent_negation() {
        let src = "RULE r:\n    WHEN x a\n    THEN NOT x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert!(c.rules[0].consequent[0].negated);
    }

    #[test]
    fn compilation_is_deterministic() {
        let src = "PREMISE e:\n    EXCLUSIVE\n        z z\n        a a\n        m m\nFACT q q\n";
        assert_eq!(
            compile_source("<t>", src).unwrap(),
            compile_source("<t>", src).unwrap()
        );
    }

    #[test]
    fn empty_program_compiles_to_empty_ir() {
        let c = compile_source("<t>", "// nothing here\n").unwrap();
        assert!(c.atoms.is_empty() && c.clauses.is_empty() && c.facts.is_empty());
    }

    #[test]
    fn same_clause_from_two_named_premises_is_deduped() {
        // Different names, identical logical content → one clause, no redefinition.
        let src = "PREMISE e1:\n    EXCLUSIVE\n        x a\n        x b\nPREMISE e2:\n    EXCLUSIVE\n        x a\n        x b\n";
        let c = compile_source("<t>", src).unwrap();
        assert_eq!(c.clauses.len(), 1);
    }

    #[test]
    fn object_distinguishes_atom_identity() {
        // `x p a` and `x p b` differ only by object → two distinct atoms.
        let c = compile_source("<t>", "FACT x p a\nFACT x p b\n").unwrap();
        assert_eq!(c.atoms.len(), 2);
    }
}
