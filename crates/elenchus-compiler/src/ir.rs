//! IR types: atom identity, literals, facts, clauses, rules, the compiled output.
use alloc::string::String;
use alloc::vec::Vec;

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

/// The human label for a resolved atom (`domain.subject predicate object`), as
/// shown in port diagnostics and reports.
impl core::fmt::Display for AtomKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}", self.domain, self.subject)?;
        if let Some(p) = &self.predicate {
            write!(f, " {p}")?;
        }
        if let Some(o) = &self.object {
            write!(f, " {o}")?;
        }
        Ok(())
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
    /// One record per `EXISTS` that named neither a `SET` nor a `WITNESS` (an
    /// [`elenchus_parser::ExistsDomain::Open`]). Inert for the solver — it emits no
    /// clause — but surfaced as a WARNING nudging the author to name a witness.
    pub unwitnessed_exists: Vec<UnwitnessedExists>,
}

/// An advisory record: an `EXISTS` premise that named no candidate — neither a
/// `SET` (`IN`) nor a `WITNESS`. It cannot be checked (there is nothing to point
/// at), so it grounds to no clause and is reported as a WARNING. **Advisory to the
/// SAT core, but it does raise the verdict to WARNING** (a premise that could not
/// be checked), matching an implication blocked by an UNKNOWN atom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwitnessedExists {
    /// Provenance of the `EXISTS` premise (source, line, name).
    pub origin: Origin,
    /// Human label of the unwitnessed condition (`domain.subject predicate object`,
    /// with the binder still in subject position), shown as the blocked check.
    pub condition: String,
    /// The binder name, used to phrase the "name a witness" hint.
    pub binder: String,
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
