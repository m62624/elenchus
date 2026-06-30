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
//! pool, the four results) lives in `elenchus-solver`. `IMPORT` resolution is a
//! source-agnostic [`Resolver`] that flat-merges another source into the shared
//! atom universe ([`compile`] resolves imports; [`compile_source`] leaves them
//! pending).
//!
//! # Example
//!
//! ```
//! use elenchus_compiler::compile_source;
//!
//! // `ASSUME` lowers to a *soft* fact: the same atom universe as a `FACT`, but
//! // one the solver may retract. Here `x a` is asserted both ways (hard + soft).
//! let ir = compile_source("demo.vrf", "DOMAIN d\nFACT x a\nASSUME NOT x a\nCHECK x\n").unwrap();
//! assert_eq!(ir.facts.len(), 2);
//! assert!(ir.facts.iter().any(|f| f.soft)); // the ASSUME is the soft one
//! ```
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use elenchus_parser::{Atom, Body, CloseKind, Conn, ListOp, Literal, Located, Quant, Statement};
/// Re-exported so downstream crates name one source of truth: [`Diagnostics`] for
/// the syntax errors carried by [`CompileError::Parse`], and [`kw`] for the
/// keyword spellings an [`Origin::kind`] is built from (so the solver matches a
/// `kind` against `kw::PREMISE`, not a re-typed `"PREMISE"`).
pub use elenchus_parser::{Diagnostics, kw};
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

/// The identity of an atom: the `domain` plus the triple
/// `(subject, predicate, object?)`, owned so it survives across merged sources.
/// The domain is the leading sort key, so atoms group by domain; ordering is
/// otherwise lexicographic → canonical. Two atoms with the same triple in
/// *different* domains are distinct (no cross-domain unification).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AtomKey {
    /// The domain this atom belongs to (the resolved namespace, never a raw
    /// alias). `physics.engine` and `plan.engine` are different atoms.
    pub domain: String,
    /// The entity the claim is about (owned copy of the parser's `subject`).
    pub subject: String,
    /// The relation or property asserted. `None` for a **bare proposition** — a
    /// single-word atom introduced by a `VAR` port (e.g. `db_ready`). `None`
    /// sorts before any `Some`, so existing (always-`Some`) atoms keep their order.
    pub predicate: Option<String>,
    /// Optional object; part of identity, so `has flying` ≠ `has swimming`. Always
    /// `None` when `predicate` is `None`.
    pub object: Option<String>,
}

/// The domain context of one file being compiled: its own declared domain (where
/// bare atoms fall) and the local names — aliases or imported domain names — it
/// may reference other domains by. Resolving an atom's optional `domain.` prefix
/// against this context yields its canonical [`AtomKey`] domain.
struct DomainCtx {
    /// The file's own declared domain (the target for unqualified atoms).
    current: String,
    /// `local name -> canonical domain` for every name visible in this file
    /// (always includes `current -> current`, plus one entry per `IMPORT`).
    aliases: BTreeMap<String, String>,
}

impl DomainCtx {
    /// Resolve an atom's optional `domain.` prefix to a canonical domain name.
    /// `None` → the file's own domain; a prefix not imported here is an error.
    fn resolve(&self, prefix: Option<&str>) -> Result<String, CompileError> {
        match prefix {
            None => Ok(self.current.clone()),
            Some(p) => self
                .aliases
                .get(p)
                .cloned()
                .ok_or_else(|| CompileError::UnknownDomain {
                    domain: p.to_string(),
                }),
        }
    }

    /// Build the owned [`AtomKey`] for a borrowed parser [`Atom`], resolving its
    /// domain prefix against this file's context.
    fn key(&self, a: &Atom) -> Result<AtomKey, CompileError> {
        Ok(AtomKey {
            domain: self.resolve(a.domain)?,
            subject: a.subject.to_string(),
            predicate: a.predicate.map(|p| p.to_string()),
            object: a.object.map(|o| o.to_string()),
        })
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
    /// Surface kind for the report. A surface keyword (a [`kw`] constant such as
    /// `kw::FACT` / `kw::PREMISE`) for source constructs, or [`KIND_UNSAT`] for
    /// the synthetic origin the solver attaches to a global unsatisfiability.
    pub kind: &'static str,
}

/// The [`Origin::kind`] the solver stamps on a conflict that is not pinned to one
/// source construct but to the program being jointly unsatisfiable. Not a
/// keyword — so it lives here, next to the other kinds, as the one spelling both
/// the solver (which sets it) and any reader (which matches it) share.
pub const KIND_UNSAT: &str = "UNSAT";

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
    /// `true` for an `ASSUME` (a *soft*, retractable hypothesis). A soft fact
    /// behaves like a normal fact in the forward pass, but when the assumptions
    /// cannot all hold the solver may drop it (and only it) to explain the
    /// contradiction — a `FACT`/`NOT` is never retractable.
    pub soft: bool,
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
    /// Advisory: imports that a file makes but never references (no `domain.atom`
    /// from that file uses the imported domain). Structural, per-file, and inert —
    /// it never affects the solve. Only populated by [`compile`] (an unresolved
    /// import in [`compile_source`] cannot be classified). See [`UnusedImport`].
    pub unused_imports: Vec<UnusedImport>,
    /// Atoms consumed as data by a relation `FOR EACH` (the edge facts, e.g. each
    /// `a linked b`). They are read by the quantifier, so the solver must not
    /// report them as ORPHAN facts even though no clause references them.
    pub consumed: Vec<AtomId>,
    /// One record per declared `VAR` port: how it resolved (supplied / default /
    /// unset), its value and origin. Drives the report's PLACEHOLDERS section;
    /// purely advisory. Filled by `compile_source_with` / `compile_with` after
    /// [`Compiler::resolve_ports`]; empty when no port was declared.
    pub placeholders: Vec<PlaceholderInfo>,
}

/// An advisory record: a file `IMPORT`s a domain it never references. Such an
/// import is inert — no `domain.atom` in that file mentions it, so removing it
/// would not change the result. It is almost always a leftover or a forgotten
/// `domain.` prefix. **Purely informational** — it never changes the verdict.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnusedImport {
    /// The source that declared the unused `IMPORT`.
    pub file: String,
    /// The imported domain that is never referenced from `file`.
    pub domain: String,
    /// The local alias, if the import used `AS <alias>`.
    pub alias: Option<String>,
    /// 1-based line of the `IMPORT` statement in `file`.
    pub line: u32,
}

/// One external value bound to a port `key`, supplied from outside the program
/// (CLI / API / a data file). The `origin` is a short human tag used both in the
/// placeholders report and in a [`CompileError::PortConflict`] message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    /// The boolean truth supplied for the port.
    pub value: bool,
    /// Where it came from: `"CLI"`, `"api"`, `"data:<file>"`, or `"PROVIDE <file>"`.
    pub origin: String,
}

/// How a declared `VAR` port got (or did not get) its value — the per-port status
/// shown in the report's PLACEHOLDERS section. Advisory only; never affects the
/// verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderStatus {
    /// An external value (CLI/API/data) was supplied.
    Supplied,
    /// No external value; the port's `DEFAULT` was used.
    DefaultUsed,
    /// No external value and no `DEFAULT` — the port stays UNKNOWN.
    Unset,
}

/// A reporting record for one declared `VAR` port: its key, how it resolved, the
/// value it took (if any), and where that value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderInfo {
    /// The port's name (the external key).
    pub key: String,
    /// How it resolved (supplied / default / unset).
    pub status: PlaceholderStatus,
    /// The resolved boolean, or `None` when unset.
    pub value: Option<bool>,
    /// The origin of a supplied value (`None` for default/unset).
    pub origin: Option<String>,
}

/// Anything that can go wrong while compiling (and resolving imports).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    /// A source failed to parse; carries the full syntax diagnostics (every
    /// error, each as a caret block with the keyword's correct syntax). The
    /// source label is already inside the [`Diagnostics`] header.
    #[error("{0}")]
    Parse(elenchus_parser::Diagnostics),
    /// A name was reused with a different body *within the same source*.
    #[error("'{name}' redefined with a different body")]
    PremiseRedefinition {
        /// The clashing premise/rule name.
        name: String,
    },
    /// A source did not declare its `DOMAIN` (required, once, as the first
    /// statement).
    #[error("{file}: missing a DOMAIN declaration (every file must start with `DOMAIN <name>`)")]
    MissingDomain {
        /// The source label that lacked a `DOMAIN`.
        file: String,
    },
    /// A source declared `DOMAIN` more than once (a file has exactly one domain).
    #[error("{file}: more than one DOMAIN declaration (a file has exactly one domain)")]
    DuplicateDomain {
        /// The source label with the duplicate `DOMAIN`.
        file: String,
    },
    /// An atom referenced a `domain.` prefix that is not the file's own domain and
    /// was not imported in this file.
    #[error("unknown domain '{domain}' — declare it with DOMAIN, or IMPORT it in this file")]
    UnknownDomain {
        /// The unresolved domain prefix.
        domain: String,
    },
    /// Two imports bound the same local domain name to different domains (use a
    /// distinct `AS <alias>` to tell them apart).
    #[error("domain name '{alias}' is bound to two different imports (disambiguate with AS)")]
    DomainAliasClash {
        /// The clashing local domain name.
        alias: String,
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
    /// A reference used a value outside the closed set an `ONEOF` declared for that
    /// variable. Almost always a typo: the misspelling would otherwise mint a new
    /// atom that hangs in the air as UNKNOWN. Closed-world is opt-in — it only
    /// applies to a `(subject, predicate)` whose values an `ONEOF` enumerated.
    /// Boxed so this comparatively large payload does not bloat every `Result`.
    #[error(transparent)]
    UnknownValue(Box<UnknownValue>),
    /// A `FOR EACH … IN <set>` named a set that was never declared with `SET`.
    /// Usually a typo in the set name; the suggestion offers the nearest declared
    /// set when one is close.
    #[error("{file}:{line}: FOR EACH ranges over '{set}', which is not a declared SET{suggestion}")]
    UnknownSet {
        /// The source the offending `FOR EACH` is in.
        file: String,
        /// 1-based line of the `FOR EACH`.
        line: u32,
        /// The undeclared set name that was referenced.
        set: String,
        /// ` — did you mean \`x\`?`, or empty when nothing is close enough.
        suggestion: String,
    },
    /// `CLOSE <relation> TRANSITIVE` found a cycle: a node transitively reaches
    /// itself. Transitive closure requires a DAG (e.g. a dependency graph).
    #[error(
        "{file}:{line}: relation '{relation}' has a cycle (`{node}` reaches itself) \
         — CLOSE … TRANSITIVE requires a DAG"
    )]
    CyclicRelation {
        /// The source the `CLOSE` is in.
        file: String,
        /// 1-based line of the `CLOSE`.
        line: u32,
        /// The relation predicate being closed.
        relation: String,
        /// A node on the cycle (reaches itself).
        node: String,
    },
    /// A bare proposition (a single-word atom like `db_ready`) was used in the
    /// program but never declared with `VAR`. Almost always a typo or a forgotten
    /// declaration; the suggestion offers the nearest declared port when close.
    #[error(
        "{file}:{line}: '{name}' is a bare proposition but no VAR declares it \
         — add `VAR {name}`{suggestion}"
    )]
    UndeclaredPort {
        /// The source the offending reference is in.
        file: String,
        /// 1-based line of the offending reference.
        line: u32,
        /// The undeclared bare-proposition name.
        name: String,
        /// ` — did you mean \`x\`?`, or empty when nothing is close enough.
        suggestion: String,
    },
    /// An external value (`--set`, API, or `PROVIDE`) named a port that no `VAR`
    /// declares. Strict by design: silently ignoring an unknown key would hide a
    /// mistake.
    #[error("no VAR declares the port '{name}' that an external value sets{suggestion}")]
    UnknownPort {
        /// The supplied key that matches no declared port.
        name: String,
        /// ` — did you mean \`x\`?`, or empty when nothing is close enough.
        suggestion: String,
    },
    /// An external value named a bare target declared in more than one domain, so
    /// which one it sets is ambiguous. Resolve it by qualifying the key with a
    /// `domain.` prefix.
    #[error(
        "'{name}' is declared in multiple domains ({domains}); qualify which one as `<domain>.{name}`"
    )]
    AmbiguousPort {
        /// The ambiguous bare name (port or atom subject).
        name: String,
        /// The domains that declare this name, comma-joined (sorted).
        domains: String,
    },
    /// A multi-word external value (`PROVIDE engine has_fuel: true` or a `--set`/API
    /// key with a predicate) named an atom that no statement in the program uses.
    /// Strict by design — like [`CompileError::UnknownPort`], a typo must not be
    /// silently injected as a new fact.
    #[error(
        "no atom '{name}' is used in the program, so an external value cannot set it \
         (use it in a FACT/PREMISE/RULE, or fix a typo)"
    )]
    UnknownExternalAtom {
        /// The atom reference (e.g. `engine has_fuel`) that matched nothing.
        name: String,
    },
    /// Two sources supplied different values for the same port. Ambiguity is a hard
    /// error: the engine is about determinism, so it never silently picks one.
    #[error(
        "the port '{name}' is set to two different values: {a_value} (from {a_origin}) \
         and {b_value} (from {b_origin})"
    )]
    PortConflict {
        /// The port set inconsistently.
        name: String,
        /// The first binding's value.
        a_value: bool,
        /// The first binding's origin tag.
        a_origin: String,
        /// The second (conflicting) binding's value.
        b_value: bool,
        /// The second binding's origin tag.
        b_origin: String,
    },
    /// A data file (loaded via `--data`) contained a statement other than `PROVIDE`
    /// (or `DOMAIN`). Data files carry only values, never logic.
    #[error("{file}:{line}: a data file may only contain PROVIDE (and DOMAIN), not this statement")]
    DataFileStatement {
        /// The data file with the offending statement.
        file: String,
        /// 1-based line of the offending statement.
        line: u32,
    },
}

/// Details of a closed-world violation (see [`CompileError::UnknownValue`]). Kept
/// in its own (boxed) struct so the common error path stays small.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "{file}:{line}: '{value}' is not a declared value of '{subject} {predicate}' \
     — ONEOF declares {{ {declared} }}{suggestion}"
)]
pub struct UnknownValue {
    /// The source the offending reference is in.
    pub file: String,
    /// 1-based line of the offending reference.
    pub line: u32,
    /// The variable's subject.
    pub subject: String,
    /// The variable's predicate.
    pub predicate: String,
    /// The out-of-set value that was used.
    pub value: String,
    /// The declared legal values, comma-joined (sorted).
    pub declared: String,
    /// ` — did you mean \`x\`?`, or empty when nothing is close enough.
    pub suggestion: String,
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
    soft: bool,
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

/// A declared `VAR` port, keyed in the compiler by `(domain, name)`: its optional
/// `DEFAULT` fallback and where it was declared (for provenance on the synthetic
/// fact and the placeholders report).
struct PortDecl {
    default: Option<bool>,
    source: String,
    line: u32,
}

/// A parsed reference an external value binds: an optional canonical `domain`, a
/// `subject`, and optional `predicate`/`object`. A lone subject (predicate `None`)
/// names a `VAR` port; a multi-word ref names an atom an external value asserts.
/// `domain == None` means "search every domain" (the historical bare-name match);
/// `Some(d)` pins it to that domain. Built from a `PROVIDE` atom (alias resolved to
/// a canonical domain) or parsed from a CLI/API key by [`parse_port_ref`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortRef {
    domain: Option<String>,
    subject: String,
    predicate: Option<String>,
    object: Option<String>,
}

impl PortRef {
    /// The human label used in error messages — the key the way it was written
    /// (`engine has_fuel`, `self.has_vision`).
    fn label(&self) -> String {
        let mut s = String::new();
        if let Some(d) = &self.domain {
            s.push_str(d);
            s.push('.');
        }
        s.push_str(&self.subject);
        if let Some(p) = &self.predicate {
            s.push(' ');
            s.push_str(p);
        }
        if let Some(o) = &self.object {
            s.push(' ');
            s.push_str(o);
        }
        s
    }
}

/// Parse a CLI/API key (`db_ready`, `self.has_vision`, `engine has_fuel`) into a
/// [`PortRef`]. A leading `domain.` (a `.` inside the first whitespace token)
/// becomes the domain; the rest splits on whitespace into `subject [predicate
/// [object]]`. `.` is not an identifier character, so the split is unambiguous and
/// backward-compatible with the bare names used before qualification existed.
fn parse_port_ref(key: &str) -> PortRef {
    let mut words = key.split_whitespace();
    let first = words.next().unwrap_or("");
    let (domain, subject) = match first.split_once('.') {
        Some((d, s)) => (Some(d.to_string()), s.to_string()),
        None => (None, first.to_string()),
    };
    PortRef {
        domain,
        subject,
        predicate: words.next().map(str::to_string),
        object: words.next().map(str::to_string),
    }
}

/// The human label for a resolved [`AtomKey`] in port diagnostics
/// (`domain.subject predicate object`).
fn atomkey_label(k: &AtomKey) -> String {
    let mut s = alloc::format!("{}.{}", k.domain, k.subject);
    if let Some(p) = &k.predicate {
        s.push(' ');
        s.push_str(p);
    }
    if let Some(o) = &k.object {
        s.push(' ');
        s.push_str(o);
    }
    s
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
    /// Closed-world value sets declared by `ONEOF`: `(domain, subject, predicate)`
    /// → the set of legal objects. Once a variable's values are enumerated by a
    /// `ONEOF`, a reference to that variable with an object *outside* the set is a
    /// compile error (a likely typo), not a silent new atom. Only `ONEOF` members
    /// that carry an object register here (binary atoms have no value slot to
    /// close). See [`Compiler::validate_closed_world`].
    oneof_values: BTreeMap<(String, String, String), BTreeSet<String>>,
    /// Declared `SET <name>` collections: name → elements, used to ground a
    /// `FOR EACH <binder> IN <name>` quantifier by instantiating the body once
    /// per element. Populated in a pre-pass so a `FOR EACH` may reference a set
    /// declared later in the file.
    sets: BTreeMap<String, Vec<String>>,
    /// Declared relation pairs: predicate → `(subject, object)` of every 3-part
    /// `FACT`, used to ground a `FOR EACH <a> <predicate> <b>` quantifier. Also a
    /// pre-pass, so the edges may be declared after the quantifier.
    relations: BTreeMap<String, Vec<(String, String)>>,
    /// Edge atoms consumed by a relation `FOR EACH` (e.g. each `a linked b`).
    /// They are *read as data* by the quantifier, so they are not idle facts —
    /// [`Compiler::finalize`] passes them to the report to suppress the ORPHAN
    /// lint.
    relation_consumed: BTreeSet<AtomKey>,
    /// Declared `VAR` ports, keyed by `(domain, name)`. Collected as statements are
    /// added (across every imported file), then resolved against external values in
    /// [`Compiler::resolve_ports`]. The first declaration of a `(domain, name)`
    /// wins; later duplicates are ignored.
    ports: BTreeMap<(String, String), PortDecl>,
    /// `PROVIDE` bindings seen in compiled sources (origin `"PROVIDE <file>"`).
    /// Each target's `domain.` prefix is already resolved to a canonical domain.
    /// They join the same conflict pool as external `--set`/API values in
    /// [`Compiler::resolve_ports`].
    provides: Vec<(PortRef, PortBinding)>,
}

impl Compiler {
    /// A fresh, empty compiler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one source and accumulate its statements. `source` is a label used
    /// in provenance (e.g. a file name or `"<root>"`). The source must declare its
    /// `DOMAIN`; `IMPORT`s are recorded as pending (their domains cannot be bound
    /// without a [`Resolver`]), so a single source may only reference its own
    /// domain. Use [`compile`] for cross-domain references.
    pub fn add_source(&mut self, source: &str, src: &str) -> Result<(), CompileError> {
        let program = parse_tagged(source, src)?;
        let domain = extract_domain(&program, source)?;
        let mut aliases = BTreeMap::new();
        aliases.insert(domain.clone(), domain.clone());
        let ctx = DomainCtx {
            current: domain,
            aliases,
        };
        self.collect_decls(&program);
        self.apply_closures(&program, source)?;
        for stmt in &program.statements {
            match stmt {
                Statement::Domain(_) => {}
                Statement::Import { path, .. } => {
                    self.pending_imports.push(path.data.to_string());
                }
                other => self.add_statement(source, other, &ctx)?,
            }
        }
        Ok(())
    }

    /// Apply every `CLOSE … TRANSITIVE`: replace the relation's pairs with their
    /// transitive closure so a relation `FOR EACH` ranges over reachability, and
    /// reject a cycle (a node reaching itself). A pure compile-time graph pass —
    /// the solver never sees it. Runs after [`collect_decls`] (the direct edges
    /// must be known) and before grounding.
    fn apply_closures(
        &mut self,
        program: &elenchus_parser::Program,
        source: &str,
    ) -> Result<(), CompileError> {
        for stmt in &program.statements {
            if let Statement::Close { relation, kind } = stmt {
                let pairs = self
                    .relations
                    .get(relation.data)
                    .cloned()
                    .unwrap_or_default();
                let closed = match kind {
                    CloseKind::Transitive => {
                        let closed = transitive_closure(pairs);
                        // TRANSITIVE requires a DAG: a node reaching itself is a
                        // cycle. (The other kinds expect or add self/back pairs by
                        // design, so only this one rejects them.)
                        if let Some((node, _)) = closed.iter().find(|(a, b)| a == b) {
                            return Err(CompileError::CyclicRelation {
                                file: source.to_string(),
                                line: relation.span.location_line(),
                                relation: relation.data.to_string(),
                                node: node.clone(),
                            });
                        }
                        closed
                    }
                    CloseKind::Symmetric => symmetric_closure(pairs),
                    CloseKind::Reflexive => reflexive_closure(pairs),
                    CloseKind::Equivalence => equivalence_closure(pairs),
                    CloseKind::Scc => scc_closure(pairs),
                };
                self.relations.insert(relation.data.to_string(), closed);
            }
        }
        Ok(())
    }

    /// Pre-pass: record every `SET` and every relation pair (3-part `FACT`) so a
    /// `FOR EACH` may reference a set or relation declared anywhere in the same
    /// source, including after the quantifier.
    fn collect_decls(&mut self, program: &elenchus_parser::Program) {
        for stmt in &program.statements {
            match stmt {
                Statement::Set { name, elements } => {
                    self.sets.insert(
                        name.data.to_string(),
                        elements.iter().map(|e| e.data.to_string()).collect(),
                    );
                }
                Statement::Fact(a) => {
                    // Only a 3-part fact (`a rel b`) declares a relation pair; a bare
                    // proposition or a 2-word fact has no object (hence no predicate
                    // pair to record).
                    if let (Some(pred), Some(obj)) = (a.data.predicate, a.data.object) {
                        self.relations
                            .entry(pred.to_string())
                            .or_default()
                            .push((a.data.subject.to_string(), obj.to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    /// Compile one already-resolved file's statements under its domain context.
    fn add_resolved(&mut self, file: &ResolvedFile) -> Result<(), CompileError> {
        let program = parse_tagged(&file.path, &file.content)?;
        self.collect_decls(&program);
        self.apply_closures(&program, &file.path)?;
        for stmt in &program.statements {
            match stmt {
                Statement::Import { .. } | Statement::Domain(_) => {}
                other => self.add_statement(&file.path, other, &file.ctx)?,
            }
        }
        Ok(())
    }

    /// Route one statement (never `IMPORT`/`DOMAIN` — handled by the loaders) to
    /// the right accumulator, resolving atom domains through `ctx`.
    fn add_statement(
        &mut self,
        source: &str,
        stmt: &Statement,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        match stmt {
            // Handled by `add_source` / `load_recursive`, never reach here.
            Statement::Import { .. } | Statement::Domain(_) => {}
            Statement::Fact(a) => self.add_fact(source, a, Value::True, kw::FACT, false, ctx)?,
            Statement::Negation(a) => {
                self.add_fact(source, a, Value::False, kw::NOT, false, ctx)?
            }
            Statement::Assume(l) => {
                let value = if l.data.negated {
                    Value::False
                } else {
                    Value::True
                };
                // A soft assertion shares the FACT accumulator; the atom is the
                // literal's atom, the polarity its `NOT`, and `soft` marks it
                // retractable. The span is the whole `ASSUME` line.
                let located = elenchus_parser::Located {
                    data: l.data.atom.clone(),
                    span: l.span,
                };
                self.add_fact(source, &located, value, kw::ASSUME, true, ctx)?;
            }
            Statement::Check {
                subject,
                bidirectional,
            } => self.checks.push(Check {
                subject: subject.as_ref().map(|s| s.data.to_string()),
                bidirectional: *bidirectional,
            }),
            // Declared in the `collect_decls` / `apply_closures` pre-passes;
            // nothing to emit here.
            Statement::Set { .. } | Statement::Close { .. } => {}
            // A port declaration: record it under this file's domain (the first
            // declaration of a `(domain, name)` wins). It is resolved against
            // external values later, in `resolve_ports`.
            Statement::Var { name, default } => {
                self.ports
                    .entry((ctx.current.clone(), name.data.to_string()))
                    .or_insert(PortDecl {
                        default: *default,
                        source: source.to_string(),
                        line: name.span.location_line(),
                    });
            }
            // A value binding: resolve the target's `domain.` prefix against this
            // file's context (so an alias becomes a canonical domain; no prefix
            // stays `None` = search every domain, the historical behaviour), then
            // queue it into the conflict pool alongside any external values.
            Statement::Provide { atom, value } => {
                let a = &atom.data;
                let domain = match a.domain {
                    None => None,
                    Some(p) => Some(ctx.resolve(Some(p))?),
                };
                self.provides.push((
                    PortRef {
                        domain,
                        subject: a.subject.to_string(),
                        predicate: a.predicate.map(|p| p.to_string()),
                        object: a.object.map(|o| o.to_string()),
                    },
                    PortBinding {
                        value: *value,
                        origin: alloc::format!("PROVIDE {source}"),
                    },
                ));
            }
            Statement::Premise { name, quant, body } => {
                self.add_named(source, name, quant.as_ref(), body, false, ctx)?;
            }
            Statement::Rule { name, quant, body } => {
                self.add_named(source, name, quant.as_ref(), body, true, ctx)?;
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
        soft: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        let key = ctx.key(&a.data)?;
        self.intern(&key);
        let sig = alloc::format!(
            "{}|{}|{}|{}",
            key_sig(&key),
            matches!(value, Value::True) as u8,
            kind,
            "" // facts dedup ignores line; identical FACT twice is idempotent
        );
        if !self.fact_sigs.insert(sig) {
            return Ok(()); // exact duplicate fact — idempotent
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
            soft,
        });
        Ok(())
    }

    /// Handle a named construct (`PREMISE` or `RULE`). `is_rule` selects derivation
    /// vs checking. Returns an error on redefinition with a different body.
    fn add_named(
        &mut self,
        source: &str,
        name: &Located<&str>,
        quant: Option<&Quant>,
        body: &Body,
        is_rule: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        let line = name.span.location_line();
        let name = name.data;
        // The redefinition hash covers the quantifier too, so two same-named
        // premises that differ only in their `FOR EACH` are still a redefinition.
        let mut canon = canonical_body(name, body, is_rule, ctx)?;
        if let Some(q) = quant {
            canon.push_str(&quant_sig(q));
        }
        let body_hash = hash_hex(canon.as_bytes());
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

        match quant {
            // Unquantified: emit the body once, as before.
            None => self.emit_named(source, name, line, body, is_rule, ctx),
            // `FOR EACH <binder> IN <set>`: instantiate the body once per element,
            // substituting the binder. Grounding is exactly `|set|` repetitions of
            // the *same* desugar — linear, never a domain product (a second binder
            // is unrepresentable in the grammar).
            Some(Quant::InSet { binder, set }) => {
                let elements = match self.sets.get(set.data) {
                    Some(els) => els.clone(),
                    None => {
                        return Err(CompileError::UnknownSet {
                            file: source.to_string(),
                            line: set.span.location_line(),
                            set: set.data.to_string(),
                            suggestion: nearest_set_suggestion(set.data, &self.sets),
                        });
                    }
                };
                for el in &elements {
                    let grounded = subst_body(body, &[(binder.data, el)]);
                    self.emit_named(source, name, line, &grounded, is_rule, ctx)?;
                }
                Ok(())
            }
            // `FOR EACH <a> <relation> <b>`: instantiate the body once per declared
            // FACT pair of that relation, binding `a`→subject, `b`→object. The pair
            // is pinned by data, so this is linear in the number of facts — never a
            // product of the domain with itself.
            Some(Quant::Relation {
                left,
                predicate,
                right,
            }) => {
                let pairs = self
                    .relations
                    .get(predicate.data)
                    .cloned()
                    .unwrap_or_default();
                for (subj, obj) in &pairs {
                    let grounded = subst_body(body, &[(left.data, subj), (right.data, obj)]);
                    self.emit_named(source, name, line, &grounded, is_rule, ctx)?;
                    // The edge atom is read as data by the quantifier, not idle —
                    // record it so the ORPHAN lint does not flag it.
                    if let Ok(k) = ctx.key(&Atom {
                        domain: None,
                        subject: subj,
                        predicate: Some(predicate.data),
                        object: Some(obj),
                    }) {
                        self.relation_consumed.insert(k);
                    }
                }
                Ok(())
            }
        }
    }

    /// Emit the clauses/rule for one (already-grounded) named construct's body.
    /// Split out of [`Compiler::add_named`] so a `FOR EACH` can call it once per
    /// element with the binder substituted, reusing the exact same desugar.
    fn emit_named(
        &mut self,
        source: &str,
        name: &str,
        line: u32,
        body: &Body,
        is_rule: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
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
                let (ante, cons) = (raw_lits(antecedent, ctx)?, raw_lits(consequent, ctx)?);
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), kw::RULE);
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
                let keys: Vec<AtomKey> = atoms
                    .iter()
                    .map(|a| ctx.key(&a.data))
                    .collect::<Result<_, _>>()?;
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
                    // ONEOF: pairwise (at most one) + at-least-one. A ONEOF also
                    // *closes* each of its variables: record every member's object
                    // as a legal value of `(domain, subject, predicate)` so a later
                    // out-of-set reference is caught as a typo (closed-world).
                    ListOp::OneOf => {
                        self.emit_pairwise(&keys, &origin);
                        self.emit_at_least_one(&keys, &origin);
                        for k in &keys {
                            if let (Some(pred), Some(obj)) = (&k.predicate, &k.object) {
                                self.oneof_values
                                    .entry((k.domain.clone(), k.subject.clone(), pred.clone()))
                                    .or_default()
                                    .insert(obj.clone());
                            }
                        }
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
                let ante = raw_lits(antecedent, ctx)?;
                let cons = raw_lits(consequent, ctx)?;
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), kw::PREMISE);

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
            Body::Exists { binder, set, atom } => {
                // ∃: at least one element of the SET satisfies the condition.
                // Instantiate the condition per element (binder substituted) and
                // emit a single at-least-one — exactly an `ATLEAST` whose atoms are
                // generated from the set instead of hand-listed. Linear in `|set|`,
                // one clause; the solver sees nothing new. The dual of a `FOR EACH`
                // over a set (an "all"), so it resolves the set the same way.
                let elements = match self.sets.get(set.data) {
                    Some(els) => els.clone(),
                    None => {
                        return Err(CompileError::UnknownSet {
                            file: source.to_string(),
                            line: set.span.location_line(),
                            set: set.data.to_string(),
                            suggestion: nearest_set_suggestion(set.data, &self.sets),
                        });
                    }
                };
                let keys: Vec<AtomKey> = elements
                    .iter()
                    .map(|el| ctx.key(&subst_atom(&atom.data, &[(binder.data, el)])))
                    .collect::<Result<_, _>>()?;
                for k in &keys {
                    self.intern(k);
                }
                let origin = self.origin(source, line, Some(name), kw::EXISTS);
                self.emit_at_least_one(&keys, &origin);
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

    /// Closed-world check: once an `ONEOF` enumerates a variable's values, any
    /// reference to that `(domain, subject, predicate)` with an object outside the
    /// declared set is rejected as a likely typo — instead of silently minting a
    /// new atom that then "hangs in the air" as an UNKNOWN. Reports the earliest
    /// (by source, line) offender, with a `did you mean` suggestion when a declared
    /// value is within edit distance. Must run after all sources are accumulated
    /// (a `ONEOF` may follow the reference) and before [`finalize`].
    fn validate_closed_world(&self) -> Result<(), CompileError> {
        if self.oneof_values.is_empty() {
            return Ok(());
        }
        // Every atom reference reachable from a fact, clause, or rule, with the
        // line it came from. ONEOF members appear here too (as clause literals) but
        // are in-set by construction, so they never trip the check. `out_of_set`
        // tests one key against its variable's declared values.
        let out_of_set = |key: &AtomKey| -> bool {
            // A bare proposition (predicate `None`) has no object, so it never trips
            // closed-world; only `(predicate, object)` atoms are checked.
            match (key.predicate.as_ref(), key.object.as_ref()) {
                (Some(pred), Some(obj)) => self
                    .oneof_values
                    .get(&(key.domain.clone(), key.subject.clone(), pred.clone()))
                    .is_some_and(|set| !set.contains(obj)),
                _ => false,
            }
        };
        let mut offenders: Vec<(&str, u32, &AtomKey)> = Vec::new();
        for f in &self.facts {
            if out_of_set(&f.key) {
                offenders.push((&f.origin.source, f.origin.line, &f.key));
            }
        }
        for c in &self.clauses {
            for l in &c.lits {
                if out_of_set(&l.key) {
                    offenders.push((&c.origin.source, c.origin.line, &l.key));
                }
            }
        }
        for r in &self.rules {
            for l in r.antecedent.iter().chain(r.consequent.iter()) {
                if out_of_set(&l.key) {
                    offenders.push((&r.origin.source, r.origin.line, &l.key));
                }
            }
        }
        // Earliest offender wins, for a stable, source-ordered diagnostic.
        let Some(&(source, line, key)) = offenders.iter().min_by(|a, b| {
            (a.0, a.1, &a.2.subject, &a.2.object).cmp(&(b.0, b.1, &b.2.subject, &b.2.object))
        }) else {
            return Ok(());
        };
        // The offender tripped `out_of_set`, so its predicate is `Some`.
        let pred = key.predicate.clone().unwrap_or_default();
        let set = &self.oneof_values[&(key.domain.clone(), key.subject.clone(), pred.clone())];
        let declared: Vec<&str> = set.iter().map(|s| s.as_str()).collect(); // BTreeSet → sorted
        let value = key.object.clone().unwrap_or_default();
        let suggestion = did_you_mean(&value, &declared);
        Err(CompileError::UnknownValue(Box::new(UnknownValue {
            file: source.to_string(),
            line,
            subject: key.subject.clone(),
            predicate: pred,
            value,
            declared: declared.join(", "),
            suggestion,
        })))
    }

    /// Resolve every declared `VAR` port against the external `inputs`, validate
    /// that every used bare proposition is declared, and push a synthetic hard fact
    /// for each resolved port. Returns the per-port report records (the PLACEHOLDERS
    /// section). Run after the add-statement loop (all ports declared, all bare
    /// props interned) and before [`finalize`].
    ///
    /// Every ambiguity is a hard error, by design (the engine is deterministic):
    /// - a used bare proposition with no `VAR` → [`CompileError::UndeclaredPort`];
    /// - an external key naming no declared port → [`CompileError::UnknownPort`];
    /// - an external key whose bare name is declared in >1 domain → [`CompileError::AmbiguousPort`];
    /// - two bindings disagreeing on one key → [`CompileError::PortConflict`].
    ///
    /// A resolved port becomes a hard fact (observationally equal to `FACT name` /
    /// `NOT name`); a port with neither value nor `DEFAULT` stays UNKNOWN (no fact).
    fn resolve_ports(
        &mut self,
        inputs: &[(String, PortBinding)],
    ) -> Result<Vec<PlaceholderInfo>, CompileError> {
        // (1) Every *used* bare proposition (in a fact, clause, or rule) must be a
        // declared port. Reported with the earliest source/line, like closed-world.
        {
            let bad = |k: &AtomKey| {
                k.predicate.is_none()
                    && !self
                        .ports
                        .contains_key(&(k.domain.clone(), k.subject.clone()))
            };
            let mut offenders: Vec<(&str, u32, &str)> = Vec::new();
            for f in &self.facts {
                if bad(&f.key) {
                    offenders.push((&f.origin.source, f.origin.line, &f.key.subject));
                }
            }
            for c in &self.clauses {
                for l in &c.lits {
                    if bad(&l.key) {
                        offenders.push((&c.origin.source, c.origin.line, &l.key.subject));
                    }
                }
            }
            for r in &self.rules {
                for l in r.antecedent.iter().chain(r.consequent.iter()) {
                    if bad(&l.key) {
                        offenders.push((&r.origin.source, r.origin.line, &l.key.subject));
                    }
                }
            }
            if let Some(&(source, line, name)) = offenders
                .iter()
                .min_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
            {
                let names: Vec<&str> = self.ports.keys().map(|(_, n)| n.as_str()).collect();
                return Err(CompileError::UndeclaredPort {
                    file: source.to_string(),
                    line,
                    name: name.to_string(),
                    suggestion: did_you_mean(name, &names),
                });
            }
        }

        // (2) Resolve every binding's ref (in-file `PROVIDE`s plus external values)
        // to a canonical `AtomKey`, then merge by that key — so a bare and a
        // `domain.`-qualified key for the same target meet in one pool. Two
        // disagreeing values for one resolved key conflict.
        let external = inputs.iter().map(|(k, b)| (parse_port_ref(k), b.clone()));
        let bindings: Vec<(PortRef, PortBinding)> =
            self.provides.iter().cloned().chain(external).collect();
        let mut merged: BTreeMap<AtomKey, PortBinding> = BTreeMap::new();
        for (rf, b) in bindings {
            let key = self.resolve_port_ref(&rf)?;
            match merged.get(&key) {
                Some(prev) if prev.value != b.value => {
                    return Err(CompileError::PortConflict {
                        name: atomkey_label(&key),
                        a_value: prev.value,
                        a_origin: prev.origin.clone(),
                        b_value: b.value,
                        b_origin: b.origin.clone(),
                    });
                }
                _ => {
                    merged.entry(key).or_insert(b);
                }
            }
        }

        // (3) Resolve each declared port: supplied > DEFAULT > unset (UNKNOWN). The
        // placeholder key is bare unless the name collides across domains, in which
        // case it is qualified so the PLACEHOLDERS section stays unambiguous.
        let mut name_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, n) in self.ports.keys() {
            *name_counts.entry(n.as_str()).or_default() += 1;
        }
        let mut placeholders: Vec<PlaceholderInfo> = Vec::new();
        let mut interns: Vec<AtomKey> = Vec::new();
        let mut synth: Vec<(AtomKey, bool, String, u32)> = Vec::new();
        for ((domain, name), decl) in &self.ports {
            let key = AtomKey {
                domain: domain.clone(),
                subject: name.clone(),
                predicate: None,
                object: None,
            };
            interns.push(key.clone());
            let (status, value, origin) = match merged.get(&key) {
                Some(b) => (
                    PlaceholderStatus::Supplied,
                    Some(b.value),
                    Some(b.origin.clone()),
                ),
                None => match decl.default {
                    Some(v) => (PlaceholderStatus::DefaultUsed, Some(v), None),
                    None => (PlaceholderStatus::Unset, None, None),
                },
            };
            if let Some(v) = value {
                synth.push((key, v, decl.source.clone(), decl.line));
            }
            let label = if name_counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
                alloc::format!("{domain}.{name}")
            } else {
                name.clone()
            };
            placeholders.push(PlaceholderInfo {
                key: label,
                status,
                value,
                origin,
            });
        }

        // (4) Intern every declared port (so it appears even if unused), keep it out
        // of the ORPHAN lint, and assert a hard fact for each resolved one.
        for key in interns {
            self.intern(&key);
            self.relation_consumed.insert(key);
        }
        for (key, value, source, line) in synth {
            self.facts.push(RawFact {
                key,
                value: if value { Value::True } else { Value::False },
                origin: Origin {
                    source,
                    line,
                    premise: None,
                    kind: kw::VAR,
                },
                soft: false,
            });
        }

        // (5) A multi-word binding asserts an *atom* (not a VAR port) — inject it as
        // a hard fact, tagged with its supply origin. The atom is already interned
        // (it must occur in the program; `resolve_port_ref` rejects unknown ones).
        for (key, b) in &merged {
            if key.predicate.is_some() {
                self.facts.push(RawFact {
                    key: key.clone(),
                    value: if b.value { Value::True } else { Value::False },
                    origin: Origin {
                        source: b.origin.clone(),
                        line: 0,
                        premise: None,
                        kind: kw::PROVIDE,
                    },
                    soft: false,
                });
            }
        }

        Ok(placeholders)
    }

    /// Resolve one external [`PortRef`] to the canonical [`AtomKey`] it binds.
    /// A single-word ref (`predicate == None`) must match a declared `VAR` port; a
    /// multi-word ref must match an atom the program already uses. A `domain.`
    /// prefix pins the domain; without one, a name found in more than one domain is
    /// [`CompileError::AmbiguousPort`] (resolve by qualifying it).
    fn resolve_port_ref(&self, rf: &PortRef) -> Result<AtomKey, CompileError> {
        if rf.predicate.is_none() {
            let domains: Vec<&str> = self
                .ports
                .keys()
                .filter(|k| k.1 == rf.subject && rf.domain.as_deref().is_none_or(|d| k.0 == d))
                .map(|k| k.0.as_str())
                .collect();
            match domains.as_slice() {
                [] => {
                    let names: Vec<&str> = self.ports.keys().map(|(_, n)| n.as_str()).collect();
                    Err(CompileError::UnknownPort {
                        name: rf.label(),
                        suggestion: did_you_mean(&rf.subject, &names),
                    })
                }
                [d] => Ok(AtomKey {
                    domain: (*d).to_string(),
                    subject: rf.subject.clone(),
                    predicate: None,
                    object: None,
                }),
                many => Err(CompileError::AmbiguousPort {
                    name: rf.subject.clone(),
                    domains: many.join(", "),
                }),
            }
        } else {
            let cands: Vec<&AtomKey> = self
                .keys
                .iter()
                .filter(|k| {
                    k.subject == rf.subject
                        && k.predicate.as_deref() == rf.predicate.as_deref()
                        && k.object.as_deref() == rf.object.as_deref()
                        && rf.domain.as_deref().is_none_or(|d| k.domain == d)
                })
                .collect();
            match cands.as_slice() {
                [] => Err(CompileError::UnknownExternalAtom { name: rf.label() }),
                [k] => Ok((*k).clone()),
                many => Err(CompileError::AmbiguousPort {
                    name: rf.label(),
                    domains: many
                        .iter()
                        .map(|k| k.domain.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                }),
            }
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
                soft: f.soft,
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

        let consumed = self
            .relation_consumed
            .iter()
            .filter_map(|k| id_of.get(k).copied())
            .collect();

        Compiled {
            atoms,
            facts,
            clauses,
            rules,
            checks: self.checks,
            pending_imports: self.pending_imports,
            unused_imports: Vec::new(), // filled by `compile` (advisory, post-resolution)
            consumed,
            placeholders: Vec::new(), // filled by `*_with` after `resolve_ports`
        }
    }
}

/// Parse a data-only `.vrf` source and extract its `PROVIDE` bindings as
/// `(name, value)` pairs. A data file carries only values: any statement other
/// than `PROVIDE` (or a `DOMAIN`) is a [`CompileError::DataFileStatement`]. Used to
/// load a `--data <file>` of port values without compiling it as a program.
pub fn read_data_source(file: &str, src: &str) -> Result<Vec<(String, bool)>, CompileError> {
    let program = parse_tagged(file, src)?;
    let mut out = Vec::new();
    for stmt in &program.statements {
        match stmt {
            Statement::Provide { atom, value } => {
                // Serialize the target into a key string (`[domain.]subject[
                // predicate[ object]]`) that `parse_port_ref` re-parses uniformly,
                // so a data file and a `--set` key share one resolution path.
                let a = &atom.data;
                let mut key = String::new();
                if let Some(d) = a.domain {
                    key.push_str(d);
                    key.push('.');
                }
                key.push_str(a.subject);
                if let Some(p) = a.predicate {
                    key.push(' ');
                    key.push_str(p);
                }
                if let Some(o) = a.object {
                    key.push(' ');
                    key.push_str(o);
                }
                out.push((key, *value));
            }
            Statement::Domain(_) => {}
            other => {
                return Err(CompileError::DataFileStatement {
                    file: file.to_string(),
                    line: statement_line(other),
                });
            }
        }
    }
    Ok(out)
}

/// Parse a data-only `.vrf` source into ready-to-merge port [`PortBinding`]s, each
/// tagged with origin `data:<file>`. The shared bridge every surface uses to turn a
/// `--data` / data-map source into engine inputs, so a data file behaves identically
/// whether it arrives from the CLI, wasm, or MCP.
pub fn read_data_bindings(
    file: &str,
    src: &str,
) -> Result<Vec<(String, PortBinding)>, CompileError> {
    Ok(read_data_source(file, src)?
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                PortBinding {
                    value,
                    origin: alloc::format!("data:{file}"),
                },
            )
        })
        .collect())
}

/// The 1-based source line a statement begins on (for diagnostics).
fn statement_line(s: &Statement) -> u32 {
    match s {
        Statement::Domain(n) => n.span.location_line(),
        Statement::Import { path, .. } => path.span.location_line(),
        Statement::Fact(a) | Statement::Negation(a) => a.span.location_line(),
        Statement::Assume(l) => l.span.location_line(),
        Statement::Set { name, .. } => name.span.location_line(),
        Statement::Close { relation, .. } => relation.span.location_line(),
        Statement::Var { name, .. } => name.span.location_line(),
        Statement::Provide { atom, .. } => atom.span.location_line(),
        Statement::Premise { name, .. } | Statement::Rule { name, .. } => name.span.location_line(),
        Statement::Check { subject, .. } => subject
            .as_ref()
            .map(|s| s.span.location_line())
            .unwrap_or(0),
    }
}

/// Convenience: compile a single source into the IR. `IMPORT`s are recorded as
/// pending, not resolved (use [`compile`] with a [`Resolver`] to resolve them).
pub fn compile_source(source: &str, src: &str) -> Result<Compiled, CompileError> {
    compile_source_with(source, src, &[])
}

/// Like [`compile_source`], but resolving declared `VAR` ports against external
/// `inputs` (`(name, binding)` pairs from CLI / API / data). The `Compiled` carries
/// a `placeholders` record per declared port.
pub fn compile_source_with(
    source: &str,
    src: &str,
    inputs: &[(String, PortBinding)],
) -> Result<Compiled, CompileError> {
    let mut c = Compiler::new();
    c.add_source(source, src)?;
    c.validate_closed_world()?;
    let placeholders = c.resolve_ports(inputs)?;
    let mut compiled = c.finalize();
    compiled.placeholders = placeholders;
    Ok(compiled)
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

/// Normalize an `IMPORT` path **identically on every OS and every surface**: treat
/// both `/` and `\` as separators, resolve `relative` against `base`'s directory,
/// collapse `.` / `..`, and re-join with `/`. Pure and `no_std`, so it does not
/// depend on the host (or the compile target's) path semantics — a Windows-style
/// and a Unix-style import resolve to the same virtual path whether the resolver is
/// filesystem-, JS-callback-, or in-memory-backed. The single source of truth all
/// three [`Resolver`]s share.
pub fn normalize_import_path(base: &str, relative: &str) -> String {
    fn is_sep(c: char) -> bool {
        c == '/' || c == '\\'
    }
    fn push<'a>(parts: &mut Vec<&'a str>, seg: &'a str) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(seg),
        }
    }
    let mut absolute = base.starts_with(is_sep);
    let mut parts: Vec<&str> = Vec::new();
    // `base`'s directory = every segment but the last (the importing file's name).
    let base_segs: Vec<&str> = base.split(is_sep).collect();
    for seg in base_segs.iter().take(base_segs.len().saturating_sub(1)) {
        push(&mut parts, seg);
    }
    // An absolute `relative` replaces the base directory entirely.
    if relative.starts_with(is_sep) {
        parts.clear();
        absolute = true;
    }
    for seg in relative.split(is_sep) {
        push(&mut parts, seg);
    }
    let joined = parts.join("/");
    if absolute {
        alloc::format!("/{joined}")
    } else {
        joined
    }
}

impl Resolver for MemoryResolver {
    fn load(&self, path: &str) -> Result<String, CompileError> {
        self.sources
            .get(path)
            .cloned()
            .ok_or_else(|| CompileError::ImportNotFound(path.to_string()))
    }

    fn resolve(&self, base: &str, relative: &str) -> String {
        normalize_import_path(base, relative)
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
        // Share the one OS-independent normalizer (forward slashes, `..` collapsed)
        // so a resolved path — and the provenance recorded in the IR — is identical
        // on Windows and Unix, and identical to the in-memory and JS resolvers.
        // Windows `std::fs` accepts `/` just fine.
        normalize_import_path(base, relative)
    }
}

/// One resolved source ready to compile: its provenance path, raw text, and the
/// domain context (own domain + import-alias bindings) its atoms resolve against.
struct ResolvedFile {
    path: String,
    content: String,
    ctx: DomainCtx,
}

/// Compile a root source and all its transitive `IMPORT`s into one IR.
///
/// Each file is keyed by `DOMAIN`; atoms unify only within a domain. Imports are
/// referenced by `<domain>.<atom>` and visibility is file-local (naming is not
/// transitive, though a dependency's clauses still participate). Sources are
/// content-addressed (sha256): a source reached by several paths is compiled once
/// (so a diamond — or an exponential fan-out — stays linear, never blowing up),
/// and an import cycle is an error.
///
/// Resolution is **iterative** (an explicit work stack, not native recursion), so
/// an arbitrarily deep import chain cannot overflow the call stack.
///
/// Premise/rule names are per-source labels, not global identifiers: different
/// files may reuse a name, and the report qualifies them by source. A name reused
/// with a different body is an error only *within the same source*.
pub fn compile<R: Resolver>(root: &str, resolver: &R) -> Result<Compiled, CompileError> {
    compile_with(root, resolver, &[])
}

/// Like [`compile`], but resolving declared `VAR` ports against external `inputs`.
/// Ports declared in any file of the import graph are aggregated, then resolved
/// once after every file is added.
pub fn compile_with<R: Resolver>(
    root: &str,
    resolver: &R,
    inputs: &[(String, PortBinding)],
) -> Result<Compiled, CompileError> {
    let (files, unused_imports) = resolve_graph(root, resolver)?;
    let mut c = Compiler::new();
    for file in &files {
        c.add_resolved(file)?;
    }
    c.validate_closed_world()?;
    let placeholders = c.resolve_ports(inputs)?;
    let mut compiled = c.finalize();
    compiled.unused_imports = unused_imports;
    compiled.placeholders = placeholders;
    Ok(compiled)
}

/// One `IMPORT` edge: the optional local alias, the resolved child path, and the
/// `IMPORT` line (for the unused-import advisory).
struct ImportEdge {
    alias: Option<String>,
    child_path: String,
    line: u32,
}

/// A discovered source during graph resolution: its first-seen path, raw text,
/// declared domain, import edges, and the set of domain prefixes its atoms use
/// (`None` = its own domain; `Some(p)` = a `p.` prefix) — used to flag imports
/// that the file never references.
struct DiscoveredFile {
    path: String,
    content: String,
    domain: String,
    edges: Vec<ImportEdge>,
    used_prefixes: BTreeSet<Option<String>>,
}

/// Resolve the whole import graph reachable from `root` into a flat list of
/// [`ResolvedFile`]s, each distinct source appearing once.
///
/// Iterative depth-first traversal with an explicit work stack (`Enter`/`Exit`
/// frames) — no native recursion, so depth is unbounded without risking a stack
/// overflow. Memoized by content hash (a diamond/repeat is visited once); a hash
/// re-encountered while still on the active path is a [`CompileError::CircularImport`].
fn resolve_graph<R: Resolver>(
    root: &str,
    resolver: &R,
) -> Result<(Vec<ResolvedFile>, Vec<UnusedImport>), CompileError> {
    /// One unit of pending work on the traversal stack.
    enum Step {
        /// Visit a file at this resolved path (load, parse, enqueue its imports).
        Enter(String),
        /// Mark this content hash finished (pop it off the active path).
        Exit(String),
    }

    let mut discovered: BTreeMap<String, DiscoveredFile> = BTreeMap::new(); // by hash
    let mut path_hash: BTreeMap<String, String> = BTreeMap::new(); // resolved path → hash
    let mut order: Vec<String> = Vec::new(); // finish order, by hash
    let mut active: BTreeSet<String> = BTreeSet::new(); // hashes on the current DFS path
    let mut work: Vec<Step> = vec![Step::Enter(root.to_string())];

    while let Some(step) = work.pop() {
        match step {
            Step::Exit(hash) => {
                active.remove(&hash);
                order.push(hash);
            }
            Step::Enter(path) => {
                let content = resolver.load(&path)?;
                let hash = hash_hex(content.as_bytes());
                path_hash.insert(path.clone(), hash.clone());
                if active.contains(&hash) {
                    return Err(CompileError::CircularImport(path)); // back-edge to an ancestor
                }
                if discovered.contains_key(&hash) {
                    continue; // already fully resolved by another path — dedup
                }
                let program = parse_tagged(&path, &content)?;
                let domain = extract_domain(&program, &path)?;
                let mut edges = Vec::new();
                let mut used_prefixes = BTreeSet::new();
                for stmt in &program.statements {
                    if let Statement::Import { path: p, alias } = stmt {
                        edges.push(ImportEdge {
                            alias: alias.as_ref().map(|a| a.data.to_string()),
                            child_path: resolver.resolve(&path, p.data),
                            line: p.span.location_line(),
                        });
                    } else {
                        collect_prefixes(stmt, &mut used_prefixes);
                    }
                }
                drop(program); // release the borrow on `content` before moving it
                active.insert(hash.clone());
                work.push(Step::Exit(hash.clone()));
                for e in edges.iter().rev() {
                    work.push(Step::Enter(e.child_path.clone()));
                }
                discovered.insert(
                    hash,
                    DiscoveredFile {
                        path,
                        content,
                        domain,
                        edges,
                        used_prefixes,
                    },
                );
            }
        }
    }

    // Build each file's domain context now that every domain is known.
    // Look up every file's domain (small strings) so we can then *move* each
    // file's (potentially large) content out of `discovered` instead of cloning.
    let domain_of: BTreeMap<&str, &str> = discovered
        .iter()
        .map(|(h, f)| (h.as_str(), f.domain.as_str()))
        .collect();

    let mut out = Vec::with_capacity(order.len());
    let mut unused: Vec<UnusedImport> = Vec::new();
    for hash in &order {
        let file = &discovered[hash];
        let mut aliases = BTreeMap::new();
        aliases.insert(file.domain.clone(), file.domain.clone());
        for edge in &file.edges {
            let child_domain = domain_of[path_hash[&edge.child_path].as_str()];
            let bind = edge
                .alias
                .clone()
                .unwrap_or_else(|| child_domain.to_string());
            match aliases.get(&bind) {
                Some(existing) if existing != child_domain => {
                    return Err(CompileError::DomainAliasClash { alias: bind });
                }
                _ => {
                    aliases.insert(bind, child_domain.to_string());
                }
            }
        }

        // The domains this file actually references (each used prefix resolved
        // against its own domain / imports). An imported domain absent from this
        // set is an unused import.
        let referenced: BTreeSet<&str> = file
            .used_prefixes
            .iter()
            .filter_map(|p| match p {
                None => Some(file.domain.as_str()),
                Some(name) => aliases.get(name).map(|d| d.as_str()),
            })
            .collect();
        for edge in &file.edges {
            let child_domain = domain_of[path_hash[&edge.child_path].as_str()];
            if !referenced.contains(child_domain) {
                unused.push(UnusedImport {
                    file: file.path.clone(),
                    domain: child_domain.to_string(),
                    alias: edge.alias.clone(),
                    line: edge.line,
                });
            }
        }

        let ctx = DomainCtx {
            current: file.domain.clone(),
            aliases,
        };
        out.push((hash.clone(), ctx));
    }
    unused.sort();

    // Now move content/path out of `discovered` (no large clones) and pair with
    // the contexts built above.
    let files = out
        .into_iter()
        .map(|(hash, ctx)| {
            let file = discovered.remove(&hash).expect("hash was discovered");
            ResolvedFile {
                path: file.path,
                content: file.content,
                ctx,
            }
        })
        .collect();
    Ok((files, unused))
}

/// A canonical signature of a `FOR EACH` quantifier, appended to the body hash so
/// two same-named premises that differ only in their quantifier still count as a
/// redefinition.
fn quant_sig(q: &Quant) -> String {
    match q {
        Quant::InSet { binder, set } => alloc::format!("|FOREACH {} IN {}", binder.data, set.data),
        Quant::Relation {
            left,
            predicate,
            right,
        } => alloc::format!("|FOREACH {} {} {}", left.data, predicate.data, right.data),
    }
}

/// Parse one source, tagging any syntax [`Diagnostics`] with its file label so a
/// `CompileError::Parse` names the right file. The single spelling of "parse, and
/// on failure attach the file" — shared by the inline, resolved, and import paths.
fn parse_tagged<'a>(
    file: &str,
    content: &'a str,
) -> Result<elenchus_parser::Program<'a>, CompileError> {
    elenchus_parser::parse(content).map_err(|mut diag| {
        diag.set_file(file);
        CompileError::Parse(diag)
    })
}

/// `" — did you mean \`x\`?"` for an undeclared set name, or empty when no
/// declared set name is close enough.
fn nearest_set_suggestion(set: &str, sets: &BTreeMap<String, Vec<String>>) -> String {
    let names: Vec<&str> = sets.keys().map(String::as_str).collect();
    did_you_mean(set, &names)
}

/// A list of binder substitutions `(name, value)` applied during grounding: one
/// entry for an `IN <set>` quantifier, two for a `<a> <rel> <b>` relation.
type Subs<'s> = [(&'s str, &'s str)];

/// Replace any binder with its value in one identifier; non-matching pass through.
fn subst_ident<'s>(s: &'s str, subs: &Subs<'s>) -> &'s str {
    subs.iter()
        .find_map(|&(b, v)| (s == b).then_some(v))
        .unwrap_or(s)
}

/// Replace the binders in an atom (subject, predicate, and object positions).
fn subst_atom<'s>(a: &Atom<'s>, subs: &Subs<'s>) -> Atom<'s> {
    Atom {
        domain: a.domain,
        subject: subst_ident(a.subject, subs),
        predicate: a.predicate.map(|p| subst_ident(p, subs)),
        object: a.object.map(|o| subst_ident(o, subs)),
    }
}

/// Replace the binders in one located literal (preserving its span and `NOT`).
fn subst_lit<'s>(ll: &Located<'s, Literal<'s>>, subs: &Subs<'s>) -> Located<'s, Literal<'s>> {
    Located {
        data: Literal {
            negated: ll.data.negated,
            atom: subst_atom(&ll.data.atom, subs),
        },
        span: ll.span,
    }
}

/// Build a grounded copy of a body with the `FOR EACH` binders substituted.
/// Spans are preserved so any error still points at the original source line. The
/// result borrows from the original body and from the substitution values, so it
/// is consumed immediately (its keys interned) and never stored.
fn subst_body<'s>(body: &Body<'s>, subs: &Subs<'s>) -> Body<'s> {
    match body {
        Body::List { op, atoms } => Body::List {
            op: *op,
            atoms: atoms
                .iter()
                .map(|la| Located {
                    data: subst_atom(&la.data, subs),
                    span: la.span,
                })
                .collect(),
        },
        Body::Impl {
            antecedent,
            ante_conn,
            consequent,
            cons_conn,
        } => Body::Impl {
            antecedent: antecedent.iter().map(|l| subst_lit(l, subs)).collect(),
            ante_conn: *ante_conn,
            consequent: consequent.iter().map(|l| subst_lit(l, subs)).collect(),
            cons_conn: *cons_conn,
        },
        Body::Exists { binder, set, atom } => Body::Exists {
            binder: binder.clone(),
            set: set.clone(),
            atom: Located {
                data: subst_atom(&atom.data, subs),
                span: atom.span,
            },
        },
    }
}

/// Collect the domain prefixes used by a statement's atoms into `out` (`None` for
/// a bare atom, `Some(p)` for a `p.`-qualified one) — feeds the unused-import lint.
fn collect_prefixes(stmt: &Statement, out: &mut BTreeSet<Option<String>>) {
    let mut add = |a: &Atom| {
        out.insert(a.domain.map(|d| d.to_string()));
    };
    match stmt {
        Statement::Fact(a) | Statement::Negation(a) => add(&a.data),
        Statement::Assume(l) => add(&l.data.atom),
        Statement::Premise { body, .. } | Statement::Rule { body, .. } => match body {
            Body::List { atoms, .. } => atoms.iter().for_each(|a| add(&a.data)),
            Body::Impl {
                antecedent,
                consequent,
                ..
            } => antecedent
                .iter()
                .chain(consequent)
                .for_each(|l| add(&l.data.atom)),
            Body::Exists { atom, .. } => add(&atom.data),
        },
        Statement::Domain(_)
        | Statement::Import { .. }
        | Statement::Check { .. }
        | Statement::Set { .. }
        | Statement::Close { .. }
        | Statement::Var { .. }
        | Statement::Provide { .. } => {}
    }
}

/// Every node a relation's pairs mention (subjects and objects), deduped/sorted.
fn relation_nodes(pairs: &[(String, String)]) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for (a, b) in pairs {
        s.insert(a.clone());
        s.insert(b.clone());
    }
    s
}

/// Symmetric closure: add `(b, a)` for every `(a, b)`. O(E). Compile-time.
fn symmetric_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut set: BTreeSet<(String, String)> = pairs.into_iter().collect();
    let backs: Vec<(String, String)> = set.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
    set.extend(backs);
    set.into_iter().collect()
}

/// Reflexive closure: add `(x, x)` for every node the relation mentions. O(V).
fn reflexive_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let nodes = relation_nodes(&pairs);
    let mut set: BTreeSet<(String, String)> = pairs.into_iter().collect();
    for n in nodes {
        set.insert((n.clone(), n));
    }
    set.into_iter().collect()
}

/// Equivalence closure: reflexive + symmetric + transitive — groups nodes into
/// classes (`a ~ b` iff connected ignoring direction). O(V³) via the transitive
/// step. Cycles are expected here, so (unlike `transitive_closure`) no error.
fn equivalence_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    reflexive_closure(transitive_closure(symmetric_closure(pairs)))
}

/// Strongly-connected grouping: keep `(a, b)` where `a` and `b` reach each other
/// (mutual reachability), plus each node to itself. Isolates directed cycles.
/// O(V³) for the reachability + O(V²) for the mutual filter. Compile-time.
fn scc_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let nodes = relation_nodes(&pairs);
    let reach: BTreeSet<(String, String)> = transitive_closure(pairs).into_iter().collect();
    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    for (a, b) in &reach {
        if reach.contains(&(b.clone(), a.clone())) {
            out.insert((a.clone(), b.clone()));
        }
    }
    // Each node is its own (possibly singleton) component.
    for n in nodes {
        out.insert((n.clone(), n));
    }
    out.into_iter().collect()
}

/// The transitive closure of a relation's `(from, to)` pairs: add `(a, c)`
/// whenever `(a, b)` and `(b, c)` are present, to a fixpoint. A self-pair
/// `(x, x)` in the result marks a cycle. A small compile-time graph op.
fn transitive_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut set: BTreeSet<(String, String)> = pairs.into_iter().collect();
    loop {
        let mut added: Vec<(String, String)> = Vec::new();
        for (a, b) in &set {
            for (c, d) in &set {
                if b == c {
                    let p = (a.clone(), d.clone());
                    if !set.contains(&p) {
                        added.push(p);
                    }
                }
            }
        }
        if added.is_empty() {
            break;
        }
        set.extend(added);
    }
    set.into_iter().collect()
}

/// The single `DOMAIN` a source declares, or an error if it has none or several.
fn extract_domain(
    program: &elenchus_parser::Program,
    source: &str,
) -> Result<String, CompileError> {
    let mut found: Option<String> = None;
    for stmt in &program.statements {
        if let Statement::Domain(name) = stmt {
            if found.is_some() {
                return Err(CompileError::DuplicateDomain {
                    file: source.to_string(),
                });
            }
            found = Some(name.data.to_string());
        }
    }
    found.ok_or_else(|| CompileError::MissingDomain {
        file: source.to_string(),
    })
}

// --- helpers ---------------------------------------------------------------

/// Levenshtein edit distance over Unicode scalars (rolling two-row DP). Small
/// inputs (atom/value names), so the simple DP is plenty. The one edit-distance
/// implementation in the workspace: the compiler's "did you mean" suggestions
/// (via [`nearest`]) and the solver's typo-hint lint both build on it.
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
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

/// The closest candidate to `word` within an edit-distance threshold, or `None`
/// when nothing is close enough to be a useful "did you mean".
///
/// The threshold scales with length (`len / 3`, in Unicode scalars) and is **not**
/// floored at 1: a value of 1–2 characters yields a budget of 0, so no suggestion
/// is offered. This is deliberate — for very short tokens (a single CJK character,
/// where one symbol is a whole word, or a two-letter code) every other short value
/// sits at distance 1, so a "did you mean" there is pure noise, not a typo cue. The
/// rejection itself is exact (set membership), so suppressing the guess never hides
/// a real error; it only withholds a meaningless one. Longer names tolerate a slip
/// or two, mirroring the spirit of the solver's typo-hint lint.
fn nearest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let budget = word.chars().count() / 3;
    if budget == 0 {
        return None;
    }
    let w: Vec<char> = word.chars().collect();
    candidates
        .iter()
        .map(|&c| (levenshtein(&w, &c.chars().collect::<Vec<char>>()), c))
        .filter(|&(d, _)| d <= budget)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c)
}

/// `" — did you mean `x`?"` for the nearest candidate to `word`, or empty when
/// none is close enough. The single spelling of the suggestion suffix, shared by
/// every "unknown name" diagnostic (values, sets, …).
fn did_you_mean(word: &str, candidates: &[&str]) -> String {
    match nearest(word, candidates) {
        Some(s) => alloc::format!(" — did you mean `{s}`?"),
        None => String::new(),
    }
}

/// Lower parsed, located literals to key-based [`RawLit`]s (drops spans),
/// resolving each atom's domain through `ctx`.
fn raw_lits(
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
fn list_kind(op: ListOp) -> &'static str {
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
fn key_sig(k: &AtomKey) -> String {
    alloc::format!(
        "{}|{}|{}|{}",
        k.domain,
        k.subject,
        k.predicate.as_deref().unwrap_or(""),
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
/// Resolves atom domains through `ctx` so the signature keys on resolved identity.
fn canonical_body(
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
            s.push_str(&lit_sigs(antecedent, ctx)?);
            s.push_str("|CONS|");
            s.push_str(conn(cons_conn));
            s.push('|');
            s.push_str(&lit_sigs(consequent, ctx)?);
        }
        Body::Exists { binder, set, atom } => {
            let _ = write!(s, "EXISTS|{}|{}|", binder.data, set.data);
            s.push_str(&key_sig(&ctx.key(&atom.data)?));
        }
    }
    Ok(s)
}

/// Sorted `key|negated` signature of a literal list (order-independent), used
/// inside [`canonical_body`] so reordering a body does not look like a redefinition.
fn lit_sigs(
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
    parts.sort();
    Ok(parts.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile a single inline source under a default `DOMAIN t`, so test
    /// programs need not repeat the declaration. Atoms land in domain `t`.
    fn cs(src: &str) -> Result<Compiled, CompileError> {
        compile_source("<t>", &alloc::format!("DOMAIN t\n{src}"))
    }

    /// An atom key in the default test domain `t`.
    fn key(subject: &str, predicate: &str, object: Option<&str>) -> AtomKey {
        key_in("t", subject, predicate, object)
    }

    /// An atom key in an explicit domain.
    fn key_in(domain: &str, subject: &str, predicate: &str, object: Option<&str>) -> AtomKey {
        AtomKey {
            domain: domain.to_string(),
            subject: subject.to_string(),
            predicate: Some(predicate.to_string()),
            object: object.map(|o| o.to_string()),
        }
    }

    fn id(c: &Compiled, k: &AtomKey) -> AtomId {
        c.atoms.iter().position(|a| a == k).unwrap() as AtomId
    }

    #[test]
    fn exclusive_unfolds_pairwise() {
        let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
                x c
        "#;
        let c = cs(src).unwrap();
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
        let src = r#"
        PREMISE r:
            WHEN x a
            THEN x b
        "#;
        let c = cs(src).unwrap();
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
        let src = r#"
        PREMISE r:
            WHEN x a
            THEN NOT x b
        "#;
        let c = cs(src).unwrap();
        let b = id(&c, &key("x", "b", None));
        assert!(c.clauses[0].lits.contains(&Lit {
            atom: b,
            negated: false
        }));
    }

    #[test]
    fn consequent_or_is_one_clause_with_all_negated() {
        // WHEN x p THEN x a OR x b  ==  Impossible([x p, NOT x a, NOT x b])
        let src = r#"
        PREMISE r:
            WHEN x p
            THEN x a
            OR x b
        "#;
        let c = cs(src).unwrap();
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
        let src = r#"
        PREMISE r:
            WHEN x a
            OR x b
            THEN x c
        "#;
        let c = cs(src).unwrap();
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
        let src = r#"
        PREMISE r:
            WHEN x a
            OR x b
            THEN x c
            OR x d
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 2);
        for cl in &c.clauses {
            assert_eq!(cl.lits.len(), 3);
        }
    }

    #[test]
    fn rule_with_or_antecedent_splits_into_two_rules() {
        // (a ∨ b) → c derives c whenever either fires: two single-antecedent rules.
        let src = r#"
        RULE r:
            WHEN x a
            OR x b
            THEN x c
        "#;
        let c = cs(src).unwrap();
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
        let src = r#"
        RULE r:
            WHEN x a
            THEN x b
            OR x c
        "#;
        let err = cs(src).unwrap_err();
        assert!(matches!(
            err,
            CompileError::RuleDisjunctiveConsequent { .. }
        ));
    }

    #[test]
    fn oneof_is_pairwise_plus_at_least_one() {
        let src = r#"
        PREMISE o:
            ONEOF
                x a
                x b
        "#;
        let c = cs(src).unwrap();
        // pairwise C(2,2)=1 + 1 at-least-one = 2 clauses
        assert_eq!(c.clauses.len(), 2);
        // the at-least-one clause is the all-negated one
        assert!(c.clauses.iter().any(|cl| cl.lits.iter().all(|l| l.negated)));
    }

    #[test]
    fn atleast_is_one_negated_clause() {
        let src = r#"
        PREMISE a:
            ATLEAST
                x a
                x b
                x c
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        assert_eq!(c.clauses[0].lits.len(), 3);
        assert!(c.clauses[0].lits.iter().all(|l| l.negated));
    }

    #[test]
    fn rules_are_separate_from_clauses() {
        let src = r#"
        RULE needs:
            WHEN x a
            THEN x b
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 0);
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].antecedent.len(), 1);
        assert_eq!(c.rules[0].consequent.len(), 1);
    }

    #[test]
    fn atoms_are_canonically_sorted() {
        let src = r#"
        FACT z z
        FACT a a
        FACT m m
        "#;
        let c = cs(src).unwrap();
        let mut sorted = c.atoms.clone();
        sorted.sort();
        assert_eq!(c.atoms, sorted);
    }

    #[test]
    fn duplicate_premise_is_idempotent() {
        let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 1);
    }

    #[test]
    fn redefinition_with_different_body_errors() {
        let src = r#"
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x c
        "#;
        let err = cs(src).unwrap_err();
        assert_eq!(
            err,
            CompileError::PremiseRedefinition {
                name: "e".to_string()
            }
        );
    }

    #[test]
    fn duplicate_fact_is_idempotent() {
        let c = cs("FACT x a\nFACT x a\n").unwrap();
        assert_eq!(c.facts.len(), 1);
    }

    #[test]
    fn conflicting_facts_are_both_kept() {
        // FACT X + NOT X is a CONFLICT for the solver, not a compile error.
        let c = cs("FACT x a\nNOT x a\n").unwrap();
        assert_eq!(c.facts.len(), 2);
    }

    #[test]
    fn import_is_recorded_pending() {
        let c = cs("IMPORT \"physics.vrf\"\nFACT x a\n").unwrap();
        assert_eq!(c.pending_imports, vec!["physics.vrf".to_string()]);
    }

    #[test]
    fn qualified_fact_lands_in_the_imported_domain() {
        // The library's premise is about `physics.Engine_X has fuel`; the main file
        // asserts a fact qualified INTO that domain, so the two share one atom id.
        let mut r = MemoryResolver::new();
        r.add(
            "lib.vrf",
            r#"
        DOMAIN physics
        PREMISE needs_fuel:
            WHEN Engine_X has engine
            THEN Engine_X has fuel
        "#,
        );
        r.add(
            "main.vrf",
            r#"
        DOMAIN main
        IMPORT "lib.vrf"
        FACT physics.Engine_X has engine
        FACT physics.Engine_X has fuel
        "#,
        );
        let c = compile("main.vrf", &r).unwrap();
        assert!(c.pending_imports.is_empty());
        assert_eq!(c.clauses.len(), 1); // the imported premise
        assert_eq!(c.facts.len(), 2);

        // `physics.Engine_X has fuel` from the FACT and the imported premise share an id.
        let fuel = key_in("physics", "Engine_X", "has", Some("fuel"));
        let fuel_id = id(&c, &fuel);
        assert!(c.facts.iter().any(|f| f.atom == fuel_id));
        assert!(c.clauses[0].lits.iter().any(|l| l.atom == fuel_id));
    }

    #[test]
    fn same_triple_in_different_domains_does_not_unify() {
        // Without a domain prefix the fact lands in `main`, NOT `physics`, so it is
        // a distinct atom from the library's `physics.Engine_X has fuel`.
        let mut r = MemoryResolver::new();
        r.add("lib.vrf", "DOMAIN physics\nFACT Engine_X has fuel\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"lib.vrf\"\nFACT Engine_X has fuel\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        // Two distinct atoms: physics.Engine_X has fuel and main.Engine_X has fuel.
        assert!(c.atoms.iter().any(|a| a.domain == "physics"));
        assert!(c.atoms.iter().any(|a| a.domain == "main"));
        assert_eq!(
            c.atoms
                .iter()
                .filter(|a| a.subject == "Engine_X" && a.predicate.as_deref() == Some("has"))
                .count(),
            2
        );
    }

    #[test]
    fn import_alias_binds_a_local_domain_name() {
        // `AS phys` lets the consumer reference the imported domain by a local name.
        let mut r = MemoryResolver::new();
        r.add("lib.vrf", "DOMAIN physics\nFACT Motor over_200\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"lib.vrf\" AS phys\nFACT phys.Motor over_100\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        // Both facts live in the physics domain (one via its own name, one via alias).
        assert_eq!(c.atoms.iter().filter(|a| a.domain == "physics").count(), 2);
    }

    #[test]
    fn unknown_domain_reference_errors() {
        // Referencing a domain that is neither this file's nor imported here fails.
        let err = cs("FACT ghost.x a\n").unwrap_err();
        assert!(matches!(err, CompileError::UnknownDomain { .. }));
    }

    #[test]
    fn imports_are_not_transitive_for_naming() {
        // main imports physics; physics imports math. main may NOT name math.
        let mut r = MemoryResolver::new();
        r.add("math.vrf", "DOMAIN math\nFACT foo bar\n");
        r.add(
            "physics.vrf",
            "DOMAIN physics\nIMPORT \"math.vrf\"\nFACT Motor over_100\n",
        );
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT math.foo bar\n",
        );
        let err = compile("main.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::UnknownDomain { .. }));
    }

    #[test]
    fn transitive_dependency_clauses_still_load() {
        // Even though main can't *name* math, math's clauses still participate.
        let mut r = MemoryResolver::new();
        r.add(
            "math.vrf",
            r"
        DOMAIN math
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        ",
        );
        r.add("physics.vrf", "DOMAIN physics\nIMPORT \"math.vrf\"\n");
        r.add("main.vrf", "DOMAIN main\nIMPORT \"physics.vrf\"\n");
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.clauses.len(), 1); // math's clause loaded transitively
        assert!(c.clauses.iter().all(|cl| cl.origin.source == "math.vrf"));
    }

    #[test]
    fn missing_domain_errors() {
        let err = compile_source("nodomain.vrf", "FACT x a\n").unwrap_err();
        assert!(matches!(err, CompileError::MissingDomain { .. }));
    }

    #[test]
    fn duplicate_domain_errors() {
        let err = compile_source("dup.vrf", "DOMAIN a\nDOMAIN b\nFACT x a\n").unwrap_err();
        assert!(matches!(err, CompileError::DuplicateDomain { .. }));
    }

    #[test]
    fn alias_clash_when_one_local_name_binds_two_domains() {
        // The same local alias `x` bound to two genuinely different domains is a
        // clash: disambiguate with distinct aliases.
        let mut r = MemoryResolver::new();
        r.add("a.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add("b.vrf", "DOMAIN chemistry\nFACT atom reacts\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"a.vrf\" AS x\nIMPORT \"b.vrf\" AS x\n",
        );
        let err = compile("main.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::DomainAliasClash { .. }));
    }

    #[test]
    fn two_files_with_the_same_domain_name_merge() {
        // Nominal domains: two files both declaring DOMAIN physics share it (the
        // value of importing a premise library is exactly this unification).
        let mut r = MemoryResolver::new();
        r.add("a.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add(
            "main.vrf",
            "DOMAIN physics\nIMPORT \"a.vrf\"\nFACT Motor over_200\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        // Both motors live in the single shared `physics` domain.
        assert!(c.atoms.iter().all(|a| a.domain == "physics"));
        assert_eq!(c.atoms.len(), 2);
    }

    #[test]
    fn diamond_import_is_deduped() {
        // main → a, c ; a → base ; c → base. base merged once.
        let mut r = MemoryResolver::new();
        r.add(
            "base.vrf",
            r#"
        DOMAIN base
        PREMISE b:
            EXCLUSIVE
                x a
                x b
        "#,
        );
        r.add("a.vrf", "DOMAIN a\nIMPORT \"base.vrf\"\n");
        r.add("c.vrf", "DOMAIN c\nIMPORT \"base.vrf\"\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"a.vrf\"\nIMPORT \"c.vrf\"\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.clauses.len(), 1); // base's single clause, not two
    }

    #[test]
    fn circular_import_errors() {
        let mut r = MemoryResolver::new();
        r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\n");
        r.add("b.vrf", "DOMAIN b\nIMPORT \"a.vrf\"\n");
        let err = compile("a.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::CircularImport(_)));
    }

    #[test]
    fn three_node_cycle_errors() {
        // a → b → c → a. The back-edge to the on-path ancestor is detected.
        let mut r = MemoryResolver::new();
        r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\n");
        r.add("b.vrf", "DOMAIN b\nIMPORT \"c.vrf\"\n");
        r.add("c.vrf", "DOMAIN c\nIMPORT \"a.vrf\"\n");
        let err = compile("a.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::CircularImport(_)));
    }

    #[test]
    fn shared_grandchild_diamond_loads_once() {
        // The user's case: a imports B and C; C ALSO imports B. B must be compiled
        // exactly once (its single clause is not duplicated by the two paths to it).
        let mut r = MemoryResolver::new();
        r.add(
            "b.vrf",
            r"
        DOMAIN b
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        ",
        );
        r.add("c.vrf", "DOMAIN c\nIMPORT \"b.vrf\"\n");
        r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\nIMPORT \"c.vrf\"\n");
        let c = compile("a.vrf", &r).unwrap();
        assert_eq!(
            c.clauses.len(),
            1,
            "b.vrf's clause must appear exactly once"
        );
    }

    #[test]
    fn exponential_fan_out_is_memoized_not_blown_up() {
        // f_k imports f_{k-1} TWICE. Without content-hash memoization the visit
        // count is 2^k (2^40 ≈ a trillion); with it, the work is linear, so this
        // finishes instantly. A guard against any combinatorial blow-up / DoS.
        let mut r = MemoryResolver::new();
        r.add("f0.vrf", "DOMAIN d0\nFACT x a\n");
        let n = 40;
        for k in 1..=n {
            r.add(
                &alloc::format!("f{k}.vrf"),
                &alloc::format!(
                    "DOMAIN d{k}\nIMPORT \"f{}.vrf\"\nIMPORT \"f{}.vrf\"\n",
                    k - 1,
                    k - 1
                ),
            );
        }
        let c = compile(&alloc::format!("f{n}.vrf"), &r).unwrap();
        assert_eq!(c.facts.len(), 1); // the single fact from f0, reached once
    }

    #[test]
    fn very_deep_linear_chain_does_not_overflow() {
        // A long non-cyclic chain. Resolution is iterative (explicit work stack),
        // so a depth that would overflow a recursive loader compiles cleanly.
        let mut r = MemoryResolver::new();
        r.add("f0.vrf", "DOMAIN d0\nFACT x a\n");
        let n = 10_000;
        for k in 1..=n {
            r.add(
                &alloc::format!("f{k}.vrf"),
                &alloc::format!("DOMAIN d{k}\nIMPORT \"f{}.vrf\"\n", k - 1),
            );
        }
        let c = compile(&alloc::format!("f{n}.vrf"), &r).unwrap();
        assert_eq!(c.facts.len(), 1);
    }

    #[test]
    fn missing_import_errors() {
        let mut r = MemoryResolver::new();
        r.add("main.vrf", "DOMAIN main\nIMPORT \"ghost.vrf\"\n");
        let err = compile("main.vrf", &r).unwrap_err();
        assert!(matches!(err, CompileError::ImportNotFound(_)));
    }

    #[test]
    fn unused_import_is_flagged() {
        // main imports physics but never writes a `physics.` atom → unused.
        let mut r = MemoryResolver::new();
        r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT x a\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.unused_imports.len(), 1);
        assert_eq!(c.unused_imports[0].domain, "physics");
        assert_eq!(c.unused_imports[0].file, "main.vrf");
        assert_eq!(c.unused_imports[0].alias, None);
    }

    #[test]
    fn referenced_import_is_not_unused() {
        // The same import, but now a `physics.` atom uses it → not flagged.
        let mut r = MemoryResolver::new();
        r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT physics.Motor over_200\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert!(c.unused_imports.is_empty(), "{:?}", c.unused_imports);
    }

    #[test]
    fn unused_import_records_its_alias() {
        let mut r = MemoryResolver::new();
        r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"physics.vrf\" AS phys\nFACT x a\n",
        );
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.unused_imports.len(), 1);
        assert_eq!(c.unused_imports[0].alias.as_deref(), Some("phys"));
    }

    #[test]
    fn import_referenced_only_inside_a_premise_is_used() {
        // The reference can be anywhere — here inside a premise body, not a fact.
        let mut r = MemoryResolver::new();
        r.add("physics.vrf", "DOMAIN physics\nFACT Motor over_100\n");
        r.add(
            "main.vrf",
            r#"
        DOMAIN main
        IMPORT "physics.vrf"
        PREMISE p:
            WHEN physics.Motor over_100
            THEN x ok
        "#,
        );
        let c = compile("main.vrf", &r).unwrap();
        assert!(c.unused_imports.is_empty(), "{:?}", c.unused_imports);
    }

    #[test]
    fn same_premise_name_across_files_coexists() {
        // Two files may legitimately reuse a premise NAME with different bodies.
        // Names are per-source labels — both premises apply, qualified by source.
        // NOT a redefinition error. (Atoms stay apart too: different domains.)
        let mut r = MemoryResolver::new();
        r.add(
            "physics.vrf",
            r#"
        DOMAIN physics
        PREMISE safety:
            EXCLUSIVE
                x a
                x b
        "#,
        );
        r.add(
            "main.vrf",
            r#"
        DOMAIN main
        IMPORT "physics.vrf"
        PREMISE safety:
            EXCLUSIVE
                x a
                x c
        "#,
        );
        let c = compile("main.vrf", &r).unwrap();
        assert_eq!(c.clauses.len(), 2); // a-b from physics, a-c from main
        assert!(c.clauses.iter().any(|cl| cl.origin.source == "physics.vrf"));
        assert!(c.clauses.iter().any(|cl| cl.origin.source == "main.vrf"));
    }

    #[test]
    fn redefinition_within_one_source_still_errors() {
        // But reusing a name with a different body *inside one source* is a mistake.
        let src = r#"
        DOMAIN m
        PREMISE e:
            EXCLUSIVE
                x a
                x b
        PREMISE e:
            EXCLUSIVE
                x a
                x c
        "#;
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
        // The qualified facts (`physics.Motor …`) share ids with the imported premise.
        let over_100 = id(&c, &key_in("physics", "Motor", "over_100", None));
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
        let src = r#"
        PREMISE f:
            FORBIDS
                x a
                x b
                x c
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 3); // C(3,2), like EXCLUSIVE
        assert!(
            c.clauses
                .iter()
                .all(|cl| cl.lits.len() == 2 && cl.lits.iter().all(|l| !l.negated))
        );
    }

    #[test]
    fn rule_with_multiple_consequents() {
        let src = r#"
        RULE r:
            WHEN x a
            THEN x b
            AND  x c
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.rules.len(), 1);
        assert_eq!(c.rules[0].consequent.len(), 2);
    }

    #[test]
    fn negated_antecedent_literal_keeps_polarity() {
        // WHEN NOT x a THEN x b  ==  Impossible([NOT x a, NOT x b])
        let src = r#"
        PREMISE a:
            WHEN NOT x a
            THEN x b
        "#;
        let c = cs(src).unwrap();
        let xa = id(&c, &key("x", "a", None));
        assert!(c.clauses[0].lits.contains(&Lit {
            atom: xa,
            negated: true
        }));
    }

    #[test]
    fn rule_keeps_consequent_negation() {
        let src = r#"
        RULE r:
            WHEN x a
            THEN NOT x b
        "#;
        let c = cs(src).unwrap();
        assert!(c.rules[0].consequent[0].negated);
    }

    #[test]
    fn compilation_is_deterministic() {
        let src = r#"
        PREMISE e:
            EXCLUSIVE
                z z
                a a
                m m
        FACT q q
        "#;
        assert_eq!(cs(src).unwrap(), cs(src).unwrap());
    }

    #[test]
    fn empty_program_compiles_to_empty_ir() {
        let c = cs("// nothing here\n").unwrap();
        assert!(c.atoms.is_empty() && c.clauses.is_empty() && c.facts.is_empty());
    }

    #[test]
    fn same_clause_from_two_named_premises_is_deduped() {
        // Different names, identical logical content → one clause, no redefinition.
        let src = r#"
        PREMISE e1:
            EXCLUSIVE
                x a
                x b
        PREMISE e2:
            EXCLUSIVE
                x a
                x b
        "#;
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 1);
    }

    #[test]
    fn object_distinguishes_atom_identity() {
        // `x p a` and `x p b` differ only by object → two distinct atoms.
        let c = cs("FACT x p a\nFACT x p b\n").unwrap();
        assert_eq!(c.atoms.len(), 2);
    }

    // --- closed-world: ONEOF closes its variable's value set -----------------

    /// A `ONEOF` body declaring three values of `resolved is …`. Flush-left so it
    /// concatenates cleanly in front of an appended line (CAPSTONE-style const).
    const ONEOF_RESOLVED: &str = r"PREMISE pick:
    ONEOF
        resolved is censored
        resolved is censored_mtp
        resolved is uncensored
";

    #[test]
    fn value_outside_oneof_is_rejected() {
        let src = alloc::format!("{ONEOF_RESOLVED}FACT resolved is censoredmtp\n");
        let err = cs(&src).unwrap_err();
        let CompileError::UnknownValue(e) = err else {
            panic!("expected UnknownValue, got {err:?}");
        };
        assert_eq!(e.value, "censoredmtp");
        assert_eq!(e.subject, "resolved");
        assert_eq!(e.predicate, "is");
        assert_eq!(e.declared, "censored, censored_mtp, uncensored");
    }

    #[test]
    fn near_miss_value_suggests_the_intended_one() {
        let src = alloc::format!("{ONEOF_RESOLVED}FACT resolved is censoredmtp\n");
        let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
            panic!("expected UnknownValue");
        };
        assert_eq!(e.suggestion, " — did you mean `censored_mtp`?");
    }

    #[test]
    fn far_off_value_offers_no_suggestion() {
        // `wildly_different` is past the edit-distance budget of every declared
        // value, so we reject it but do not guess.
        let src = alloc::format!("{ONEOF_RESOLVED}FACT resolved is wildly_different\n");
        let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
            panic!("expected UnknownValue");
        };
        assert_eq!(e.suggestion, "");
    }

    #[test]
    fn declared_value_compiles_cleanly() {
        let src = alloc::format!("{ONEOF_RESOLVED}FACT resolved is censored_mtp\n");
        assert!(cs(&src).is_ok());
    }

    #[test]
    fn oneof_declared_after_the_reference_still_catches_it() {
        // The check runs once every source is accumulated, so order is irrelevant.
        let src = alloc::format!("FACT resolved is censoredmtp\n{ONEOF_RESOLVED}");
        assert!(matches!(
            cs(&src).unwrap_err(),
            CompileError::UnknownValue(_)
        ));
    }

    #[test]
    fn out_of_set_value_inside_a_premise_is_caught() {
        // Closed-world covers references anywhere — not just FACTs.
        let src = alloc::format!(
            r"{ONEOF_RESOLVED}
            PREMISE p:
                WHEN resolved is censoredmtp
                THEN x done
        "
        );
        assert!(matches!(
            cs(&src).unwrap_err(),
            CompileError::UnknownValue(_)
        ));
    }

    #[test]
    fn out_of_set_value_inside_a_rule_is_caught() {
        let src = alloc::format!(
            r"{ONEOF_RESOLVED}
            RULE r:
                WHEN x go
                THEN resolved is censoredmtp
        "
        );
        assert!(matches!(
            cs(&src).unwrap_err(),
            CompileError::UnknownValue(_)
        ));
    }

    #[test]
    fn binary_atoms_in_a_oneof_do_not_close_anything() {
        // `alice cooks` / `alice cleans` have no object slot, so there is no value
        // set to violate — a later `alice bakes` is just another atom, not an error.
        let src = r"
        PREMISE chores:
            ONEOF
                alice cooks
                alice cleans
        FACT alice bakes
        ";
        assert!(cs(src).is_ok());
    }

    #[test]
    fn a_subject_without_a_oneof_stays_open() {
        // No ONEOF over `mood is …` → open world, any value is a fresh atom.
        let src = alloc::format!("{ONEOF_RESOLVED}FACT mood is anything_goes\n");
        assert!(cs(&src).is_ok());
    }

    #[test]
    fn two_oneofs_union_their_declared_values() {
        // A value declared by either ONEOF for the same variable is legal.
        let src = r"
        PREMISE a:
            ONEOF
                v is one
                v is two
        PREMISE b:
            ONEOF
                v is two
                v is three
        FACT v is three
        ";
        assert!(cs(src).is_ok());
    }

    #[test]
    fn earliest_offender_is_reported() {
        // Two violations; the diagnostic points at the first by line.
        let src = alloc::format!(
            "{ONEOF_RESOLVED}FACT resolved is firstbad\nFACT resolved is secondbad\n"
        );
        let CompileError::UnknownValue(e) = cs(&src).unwrap_err() else {
            panic!("expected UnknownValue");
        };
        assert_eq!(e.value, "firstbad");
    }

    #[test]
    fn closed_world_spans_imported_domains() {
        // physics closes `Motor speed …`; main, referencing it via the prefix with
        // a typo, is rejected — the value set is shared across the domain boundary.
        let mut r = MemoryResolver::new();
        r.add(
            "physics.vrf",
            r"
        DOMAIN physics
        PREMISE g:
            ONEOF
                Motor speed slow
                Motor speed fast
        ",
        );
        r.add(
            "main.vrf",
            "DOMAIN main\nIMPORT \"physics.vrf\"\nFACT physics.Motor speed faast\n",
        );
        let CompileError::UnknownValue(e) = compile("main.vrf", &r).unwrap_err() else {
            panic!("expected UnknownValue");
        };
        assert_eq!(e.value, "faast");
        assert_eq!(e.suggestion, " — did you mean `fast`?");
    }

    #[test]
    fn same_value_in_a_different_domain_does_not_clash() {
        // `state is open` is closed in domain a; domain b's own `state is shut`
        // (never declared in a) is fine — value sets are per-domain.
        let mut r = MemoryResolver::new();
        r.add(
            "a.vrf",
            r"
        DOMAIN a
        PREMISE s:
            ONEOF
                state is open
                state is closed
        ",
        );
        r.add("b.vrf", "DOMAIN b\nIMPORT \"a.vrf\"\nFACT state is shut\n");
        // `state is shut` is in domain b, which has no ONEOF → open, so it compiles.
        assert!(compile("b.vrf", &r).is_ok());
    }

    #[test]
    fn levenshtein_basics() {
        // The canonical distance works on char slices; spell the string cases
        // through a tiny adapter so the table below reads as before.
        fn lev(a: &str, b: &str) -> usize {
            levenshtein(
                &a.chars().collect::<Vec<char>>(),
                &b.chars().collect::<Vec<char>>(),
            )
        }
        assert_eq!(lev("", ""), 0);
        assert_eq!(lev("abc", "abc"), 0);
        assert_eq!(lev("censoredmtp", "censored_mtp"), 1);
        assert_eq!(lev("norml", "normal"), 1);
        assert_eq!(lev("kitten", "sitting"), 3);
    }

    #[test]
    fn nearest_respects_the_length_budget() {
        let cands = ["censored", "censored_mtp", "uncensored"];
        assert_eq!(nearest("censoredmtp", &cands), Some("censored_mtp"));
        // "zzz" is far from all; no suggestion.
        assert_eq!(nearest("zzz", &cands), None);
    }

    #[test]
    fn nearest_offers_nothing_for_very_short_values() {
        // 1–2 character values get a budget of 0: every other short token is at
        // distance 1, so a "did you mean" carries no signal. True for single CJK
        // characters (one symbol = a whole word) and for two-letter codes alike.
        assert_eq!(nearest("七", &["一", "二", "三"]), None);
        assert_eq!(nearest("us", &["uk", "eu", "jp"]), None);
        // A multi-character CJK word still gets a sensible nearest (one wrong
        // character = distance 1, budget = 3/3 = 1).
        assert_eq!(nearest("中文字", &["中文学", "日本語"]), Some("中文学"));
    }

    #[test]
    fn short_value_is_still_rejected_just_without_a_guess() {
        // The closed-world error does not depend on the suggestion: an out-of-set
        // single-character value is rejected exactly, only the `did you mean` is
        // suppressed.
        let src = r"
        PREMISE pick:
            ONEOF
                roll is 一
                roll is 二
        FACT roll is 七
        ";
        let CompileError::UnknownValue(e) = cs(src).unwrap_err() else {
            panic!("expected UnknownValue");
        };
        assert_eq!(e.value, "七");
        assert_eq!(e.suggestion, "");
    }

    // --- FOR EACH / SET (bounded quantification, Phase 1) ------------------

    #[test]
    fn for_each_grounds_once_per_element() {
        // A ONEOF body over a 2-element set: each element yields one pairwise
        // clause + one at-least-one clause = 2 clauses; 2 elements → 4 clauses,
        // and 4 distinct grounded atoms (a/b × slot m/n).
        let src = r"
        SET xs
            a
            b
        PREMISE slot FOR EACH t IN xs:
            ONEOF
                t slot m
                t slot n
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 4);
        for s in ["a", "b"] {
            for o in ["m", "n"] {
                assert!(c.atoms.contains(&key(s, "slot", Some(o))));
            }
        }
    }

    #[test]
    fn for_each_in_a_rule_derives_per_element() {
        // A quantified RULE grounds to one rule per element.
        let src = r"
        SET xs
            a
            b
        RULE r FOR EACH t IN xs:
            WHEN t on
            THEN t hot
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn for_each_over_an_undeclared_set_is_rejected() {
        let src = r"
        SET tasks
            a
        PREMISE p FOR EACH t IN taske:
            ONEOF
                t s x
                t s y
        ";
        let CompileError::UnknownSet {
            set, suggestion, ..
        } = cs(src).unwrap_err()
        else {
            panic!("expected UnknownSet");
        };
        assert_eq!(set, "taske");
        assert_eq!(suggestion, " — did you mean `tasks`?");
    }

    #[test]
    fn for_each_closes_each_grounded_variable() {
        // ONEOF inside FOR EACH closes the variable per element, so an out-of-set
        // value on a grounded subject is a hard error (closed-world after subst).
        let src = r"
        SET xs
            a
            b
        PREMISE c FOR EACH t IN xs:
            ONEOF
                t color red
                t color blue
        FACT a color gren
        ";
        let CompileError::UnknownValue(e) = cs(src).unwrap_err() else {
            panic!("expected UnknownValue from the grounded ONEOF");
        };
        assert_eq!(e.value, "gren");
        assert_eq!(e.subject, "a");
    }

    #[test]
    fn nested_for_each_is_a_parse_error() {
        // The structural guarantee: a second FOR EACH is unrepresentable — the
        // header carries exactly one, so nesting fails to parse (no domain
        // product can ever be written).
        let src = r"
        SET xs
            a
        PREMISE p FOR EACH x IN xs FOR EACH y IN xs:
            ONEOF
                x r y
                x s y
        ";
        assert!(matches!(cs(src), Err(CompileError::Parse(_))));
    }

    #[test]
    fn relation_for_each_grounds_per_fact_pair() {
        // Two declared edges → the body is instantiated once per edge (two
        // pairwise clauses), and both edge atoms are recorded as consumed so the
        // ORPHAN lint will not flag them.
        let src = r"
        FACT a linked b
        FACT b linked c
        PREMISE p FOR EACH x linked y:
            FORBIDS
                x hot on
                y hot on
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 2);
        assert_eq!(c.consumed.len(), 2);
        assert!(c.consumed.contains(&id(&c, &key("a", "linked", Some("b")))));
    }

    #[test]
    fn relation_for_each_over_no_edges_is_inert() {
        // A relation with no matching facts grounds to nothing (vacuous), not an
        // error — unlike an undeclared SET.
        let src = r"
        PREMISE p FOR EACH x linked y:
            FORBIDS
                x hot on
                y hot on
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 0);
        assert!(c.consumed.is_empty());
    }

    #[test]
    fn close_transitive_extends_the_relation() {
        // a->b, b->c; CLOSE adds a->c, so a relation FOR EACH grounds over all
        // three pairs (without CLOSE it would be two).
        let src = r"
        FACT a r b
        FACT b r c
        CLOSE r TRANSITIVE
        PREMISE p FOR EACH x r y:
            FORBIDS
                x hot on
                y hot on
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 3);
    }

    #[test]
    fn close_transitive_rejects_a_cycle() {
        let src = r"
        FACT a r b
        FACT b r a
        CLOSE r TRANSITIVE
        ";
        let CompileError::CyclicRelation { relation, .. } = cs(src).unwrap_err() else {
            panic!("expected CyclicRelation");
        };
        assert_eq!(relation, "r");
    }

    /// Count the directed pairs a relation `r` holds after a CLOSE, by grounding a
    /// relation `FOR EACH x r y` over it (one clause per pair). The body is an
    /// *implication* (asymmetric in x and y) so (a,b) and (b,a) stay distinct — a
    /// symmetric `FORBIDS` would dedup them and undercount.
    fn pairs_after_close(close: &str) -> usize {
        let src = alloc::format!(
            "FACT a r b\nFACT b r c\n{close}\nPREMISE p FOR EACH x r y:\n    WHEN x src\n    THEN y dst\n"
        );
        cs(&src).unwrap().clauses.len()
    }

    #[test]
    fn close_symmetric_adds_the_back_edge() {
        // a->b, b->c plus their reverses b->a, c->b = 4 pairs.
        assert_eq!(pairs_after_close("CLOSE r SYMMETRIC"), 4);
    }

    #[test]
    fn close_reflexive_adds_self_pairs() {
        // a->b, b->c plus a->a, b->b, c->c (3 nodes) = 5 pairs.
        assert_eq!(pairs_after_close("CLOSE r REFLEXIVE"), 5);
    }

    #[test]
    fn close_equivalence_groups_a_whole_component() {
        // a->b->c connects {a,b,c} into one class: every ordered pair incl. self =
        // 3 x 3 = 9. A cycle would be fine here (no DAG requirement), unlike TRANSITIVE.
        assert_eq!(pairs_after_close("CLOSE r EQUIVALENCE"), 9);
    }

    #[test]
    fn close_equivalence_does_not_reject_a_cycle() {
        // EQUIVALENCE expects cycles (they are the classes), so a back-edge that
        // would make TRANSITIVE fail is accepted here.
        let src = "FACT a r b\nFACT b r a\nCLOSE r EQUIVALENCE\n";
        assert!(cs(src).is_ok());
    }

    #[test]
    fn close_scc_isolates_a_directed_cycle() {
        // a<->b form a strongly-connected pair; c is reachable from b but does not
        // reach back, so it is its own singleton. SCC keeps the mutual pairs of
        // {a,b} (a-a,a-b,b-a,b-b) + c-c = 5, and does NOT error on the cycle.
        let src = "FACT a r b\nFACT b r a\nFACT b r c\nCLOSE r SCC\nPREMISE p FOR EACH x r y:\n    WHEN x src\n    THEN y dst\n";
        assert_eq!(cs(src).unwrap().clauses.len(), 5);
    }

    #[test]
    fn exists_grounds_to_one_at_least_one_over_the_set() {
        // ∃ over a 2-element set = a single at-least-one clause over the two
        // instantiated atoms (exactly an ATLEAST whose atoms come from the set).
        let src = r"
        SET handlers
            a
            b
        PREMISE covered:
            EXISTS h IN handlers
                h handles request
        ";
        let c = cs(src).unwrap();
        assert_eq!(c.clauses.len(), 1);
        assert!(c.atoms.contains(&key("a", "handles", Some("request"))));
        assert!(c.atoms.contains(&key("b", "handles", Some("request"))));
    }

    #[test]
    fn exists_matches_a_hand_written_atleast() {
        // Oracle: EXISTS over {a,b} produces the same clause set as a manual ATLEAST
        // of the two instantiated atoms.
        let via_exists =
            cs("SET hs\n    a\n    b\nPREMISE p:\n    EXISTS h IN hs\n        h does x\n").unwrap();
        let via_atleast =
            cs("PREMISE p:\n    ATLEAST\n        a does x\n        b does x\n").unwrap();
        assert_eq!(via_exists.clauses.len(), 1);
        assert_eq!(via_exists.clauses.len(), via_atleast.clauses.len());
    }

    #[test]
    fn exists_over_an_undeclared_set_is_rejected() {
        let src = "SET handlers\n    a\nPREMISE p:\n    EXISTS h IN handlerz\n        h does x\n";
        let CompileError::UnknownSet { set, .. } = cs(src).unwrap_err() else {
            panic!("expected UnknownSet");
        };
        assert_eq!(set, "handlerz");
    }

    #[test]
    fn grounding_count_is_linear_in_the_set() {
        // No domain product: N elements → exactly N groundings (here N clauses,
        // one at-least-one per element), never N².
        let elems: alloc::string::String = (0..20).map(|i| alloc::format!("    e{i}\n")).collect();
        let src = alloc::format!(
            "SET xs\n{elems}PREMISE p FOR EACH t IN xs:\n    ATLEAST\n        t a\n        t b\n"
        );
        let c = cs(&src).unwrap();
        assert_eq!(c.clauses.len(), 20);
    }
}
