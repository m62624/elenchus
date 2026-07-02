//! The report data model: status, conflicts, warnings, derived facts, and the
//! advisory pools, plus the shared atom-label helper.
use alloc::string::String;
use alloc::vec::Vec;
use elenchus_compiler::{AtomId, Compiled, Origin, PlaceholderInfo, UnusedImport, Value};

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
    /// A directed fix for the most informative blocking atom, distinguishing the
    /// two reasons a check stays blocked: the atom is a *free input* nothing can
    /// determine (→ add a `FACT`/`NOT`, or make a `PREMISE` a `RULE` so it derives
    /// the value), versus the atom *is* derivable by a `RULE` that has not fired
    /// (→ assert that rule's antecedent). Advisory text; never changes the verdict.
    pub hint: Option<String>,
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

/// A defeasible `RULE` whose default was suppressed by an established `UNLESS`
/// exception: the antecedent held, but an exception was TRUE, so the rule derived
/// nothing. **Purely informational** — it never changes the verdict, the warning
/// pool, or the exit code (a defeated default is not a conflict). It makes the
/// non-monotonic step legible: "this default was overridden, and by what".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defeated {
    /// Provenance of the defeated `RULE` (source, line, name).
    pub origin: Origin,
    /// The consequent the rule would have derived, as a human label (`,`-joined if
    /// the `THEN` had several literals).
    pub consequent: String,
    /// Human labels of the established `UNLESS` exceptions that suppressed it.
    pub blocked_by: Vec<String>,
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
    /// Defeasible `RULE`s whose default was suppressed by an established `UNLESS`
    /// exception. Never affects [`Report::status`] or [`Report::exit_code`] — purely
    /// informational (a defeated default is not a conflict).
    pub defeated: Vec<Defeated>,
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
    /// Advisory "orphan fact" lints: a `FACT`/`NOT`/`ASSUME` whose atom is never
    /// referenced by any `PREMISE` or `RULE`, so it can neither be checked nor
    /// derive anything — it has no effect on the verdict. Never affects
    /// [`Report::status`] or [`Report::exit_code`] — purely informational.
    pub orphans: Vec<OrphanFact>,
    /// Advisory "unused import" lints: a file `IMPORT`s a domain it never
    /// references (no `domain.atom` from that file uses it), so the import is
    /// inert. Never affects [`Report::status`] or [`Report::exit_code`] — purely
    /// informational. (Carried through from compilation; see [`UnusedImport`].)
    pub unused_imports: Vec<UnusedImport>,
    /// One record per declared `VAR` port: whether its value was supplied, fell
    /// back to `DEFAULT`, or stayed UNKNOWN. The report's PLACEHOLDERS section.
    /// Never affects [`Report::status`] or [`Report::exit_code`] — purely
    /// informational. (Carried through from compilation; see [`PlaceholderInfo`].)
    pub placeholders: Vec<PlaceholderInfo>,
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

/// An advisory lint: a `FACT`/`NOT`/`ASSUME` whose atom appears in **no**
/// `PREMISE` or `RULE`. Such an assertion is logically inert — nothing checks it
/// and nothing is derived from it, so it can never produce a CONFLICT, WARNING or
/// DERIVED. It is almost always a typo'd atom name or a leftover line. **Purely
/// informational** — it never changes the verdict, the warning pool, or the exit
/// code (a program full of orphans is still perfectly CONSISTENT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanFact {
    /// The atom's human label (`subject predicate [object]`), without polarity.
    pub atom: String,
    /// The asserted value — `False` means the surface line was `NOT`/`ASSUME NOT`.
    pub value: Value,
    /// Provenance of the inert assertion (source, line, kind = `FACT`/`NOT`/`ASSUME`).
    pub origin: Origin,
}

/// One construct named in an [`Report::unsat_core`] or [`Report::retract`], with the
/// concrete repair actions that would clear the contradiction if applied to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreItem {
    /// Provenance of the construct (source, line, kind, premise name if any).
    pub origin: Origin,
    /// A human label: the premise/rule name, or the atom for a bare `FACT`/`NOT`.
    pub label: String,
    /// Engine-verified minimal repairs for this construct: always a [`FixKind::Drop`]
    /// (removing it restores consistency — that is what put it in the minimal set),
    /// plus a [`FixKind::Flip`] **only when re-solving with the flipped fact actually
    /// yields a consistent system**. Purely advisory — naming which single edit clears
    /// the CONFLICT, without changing the verdict or exit code.
    pub fixes: Vec<Fix>,
}

/// One concrete, engine-checked repair the reader can apply, then re-run — the
/// engine's answer to "CONFLICT, but *what do I change?*". Purely advisory: applying
/// it is the caller's choice, and its presence never affects [`Report::status`] or
/// [`Report::exit_code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// What to do to the target.
    pub kind: FixKind,
    /// The ready-to-print target: the construct to drop, or the literal a flip would
    /// assert instead (e.g. `NOT rel has_rollback`).
    pub target: String,
}

/// The kind of a [`Fix`]. Only these two can repair a CONFLICT: a jointly-unsatisfiable
/// clause set stays unsatisfiable under *more* clauses, so **adding** a fact can never
/// clear a conflict (that is monotonicity, not a heuristic) — the only levers are
/// removing a written construct or flipping a written fact's polarity. Proposing a
/// *new* missing premise is a different layer (abduction), where the LLM supplies the
/// candidate and the engine checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixKind {
    /// Delete the written construct (`FACT` / `ASSUME` / `PREMISE` / `RULE`).
    Drop,
    /// Reverse a written fact's polarity (`FACT x` ↔ `NOT x`). Every `Flip` in a
    /// [`CoreItem::fixes`] list is engine-verified: re-solving with it is consistent.
    Flip,
}

/// Render atom `a` as the human string `domain.subject predicate [object]`. The
/// domain prefix is always shown — atom identity now includes it, so the label
/// is unambiguous (`physics.engine runs` vs `plan.engine runs`).
pub(crate) fn label(c: &Compiled, a: AtomId) -> String {
    let k = &c.atoms[a as usize];
    match (&k.predicate, &k.object) {
        // Full triple `domain.subject predicate object`.
        (Some(p), Some(o)) => alloc::format!("{}.{} {} {}", k.domain, k.subject, p, o),
        // Two-word atom `domain.subject predicate`.
        (Some(p), None) => alloc::format!("{}.{} {}", k.domain, k.subject, p),
        // Bare proposition (a VAR port): just `domain.subject`.
        (None, _) => alloc::format!("{}.{}", k.domain, k.subject),
    }
}

mod human;
mod json;
