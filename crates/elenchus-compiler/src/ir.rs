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
/// the consequent literals — *unless* an exception defeats it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Literals that must all hold for the rule to fire.
    pub antecedent: Vec<Lit>,
    /// Literals derived (asserted) when the antecedent holds.
    pub consequent: Vec<Lit>,
    /// `UNLESS` exceptions (a defeasible default). The rule fires only when no
    /// exception literal is *established* TRUE — an exception that is FALSE or
    /// UNKNOWN lets the default stand. Empty = an ordinary indefeasible rule.
    pub exceptions: Vec<Lit>,
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
    /// One record per `FACT … BECAUSE <ground>` — the justification (L2) layer. The
    /// solver reads the ground atom's value: FALSE → CONFLICT ("your reason does not
    /// hold"), UNKNOWN → WARNING ("your reason is unestablished"), TRUE → silent. It
    /// emits **no clause** — the check is evaluative, not a constraint.
    pub justifications: Vec<Justification>,
    /// One record per `TRY <literal>` — the abduction (L5) layer. A hypothesis under
    /// test: the LLM supplies a candidate the engine has *not* committed. The solver
    /// runs one side-solve (the program plus this literal) and reports whether
    /// asserting it would close the open model, conflict with it, or leave it open.
    /// It emits **no clause and no fact** — it never enters the model or the verdict.
    pub hypotheses: Vec<Hypothesis>,
    /// One record per `PROVE <literal>` — the entailment (⊨) layer. The engine asks
    /// refutationally whether the theory entails the goal: `theory ∧ ¬goal`
    /// unsatisfiable → PROVED, `theory ∧ goal` unsatisfiable → REFUTED, both
    /// satisfiable → OPEN. It emits **no clause and no fact** — the goal never enters
    /// the model or the verdict; the check is a bounded post-verdict side-solve.
    pub goals: Vec<Goal>,
    /// One step per `HENCE <conclusion> FROM <refs>` — the checked-derivation (proof
    /// witness) layer, in program order. The solver verifies each step separately:
    /// do the clauses of the *cited* references alone entail the conclusion (one
    /// refutation side-solve per step)? A step emits **no clause and no fact** — the
    /// main solve is untouched; a broken step is reported by name.
    pub derivations: Vec<Derivation>,
    /// One record per `TOTAL <relation> ON <set>` — the Skolem witness-table (∀∃)
    /// layer, checked at compile time by a single linear scan of the declared
    /// pairs. `missing` lists the set elements with no witness pair; the solver
    /// raises each non-empty record to a WARNING naming them (a claimed existence
    /// with no witness, like an unwitnessed `EXISTS`). A fully-served check is
    /// silent. It emits **no clause** — the pairs are ordinary facts.
    pub totality: Vec<Totality>,
    /// One record per `KNOWS`/`BELIEVES <agent> <literal>` — the modal/epistemic (L6)
    /// layer. The solver checks each attribution against the settled world model:
    /// factive knowledge (`KNOWS`) that is FALSE → CONFLICT (you cannot know a
    /// falsehood), UNKNOWN → WARNING; a non-factive belief (`BELIEVES`) that is FALSE
    /// → an informational note (exit 0, never raises the verdict); plus a per-agent
    /// coherence check (knowing φ and ¬φ → CONFLICT). It emits **no clause and no fact**
    /// — the agent is a report-side label, never a SAT atom.
    pub attributions: Vec<Attribution>,
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

/// One `FACT … BECAUSE <ground>` justification: the belief atom, the ground it is
/// claimed to rest on, and the provenance of the `BECAUSE`. The solver checks the
/// ground's model value (FALSE → CONFLICT, UNKNOWN → WARNING, TRUE → silent). It is
/// **evaluative, not a constraint** — it emits no clause, so an UNKNOWN ground is
/// reported rather than silently forced true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Justification {
    /// The asserted atom (the belief), for the report message.
    pub belief: AtomId,
    /// The cited ground atom whose value the engine checks.
    pub ground: AtomId,
    /// Provenance of the `BECAUSE` (source, line, kind = `BECAUSE`).
    pub origin: Origin,
}

/// One `TRY <literal> [FOR <goal>]` hypothesis: the candidate atom (with its
/// polarity), the optional targeted goal, and the provenance of the `TRY`. The
/// solver adds the candidate literal to the program and re-solves, judging the
/// outcome; with a goal it instead asks the targeted-abduction pair — is
/// `theory + H` consistent, and does it entail the goal? It is **evaluative, not a
/// constraint**: it emits no clause and no fact, so it never enters the model or
/// affects the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hypothesis {
    /// The candidate literal being tested (atom id + polarity from an optional `NOT`).
    pub lit: Lit,
    /// The `FOR <goal>` literal this hypothesis is supposed to explain, if any.
    pub goal: Option<Lit>,
    /// Provenance of the `TRY` (source, line, kind = `TRY`).
    pub origin: Origin,
}

/// One `PROVE <literal>` entailment goal: the goal literal (atom id + polarity from
/// an optional `NOT`) and the provenance of the `PROVE`. The solver answers the ⊨
/// question with two bounded refutation solves — it is **evaluative, not a
/// constraint**: it emits no clause and no fact, so it never enters the model or
/// affects the verdict (the sibling of [`Hypothesis`], asking consequence instead
/// of compatibility).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    /// The goal literal being asked about (atom id + polarity from an optional `NOT`).
    pub lit: Lit,
    /// Provenance of the `PROVE` (source, line, kind = `PROVE`).
    pub origin: Origin,
}

/// How one `HENCE` step's `FROM` reference resolved — the three kinds of
/// *already-written* things a proof step may rest on (anything else fails
/// compilation with `UnknownHenceRef`; the engine never guesses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepRef {
    /// A named `PREMISE`/`RULE` of the same source: the step may use every clause
    /// that construct desugared to (a defeasible rule keeps its `UNLESS` escapes).
    Construct {
        /// The source the construct is defined in (same as the step's).
        source: String,
        /// The construct's name.
        name: String,
    },
    /// A written `FACT`/`NOT`/`ASSUME` with the same polarity as the reference.
    Fact(Lit),
    /// The conclusion of an **earlier** `HENCE` step — a linear chain (the index
    /// into [`Compiled::derivations`] is always smaller than this step's own, so a
    /// cycle is unrepresentable by line order).
    Earlier(u32),
}

/// One `HENCE <conclusion> FROM <refs>` step: the conclusion literal, the resolved
/// references, and the provenance. The solver checks `clauses(refs) ∧ ¬conclusion`
/// for unsatisfiability — natural deduction as a *witness language*, where the LLM
/// writes the proof and the kernel only re-checks each step. **Evaluative, not a
/// constraint**: no clause, no fact, the main solve and verdict are untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    /// The step's conclusion (atom id + polarity from an optional `NOT`).
    pub conclusion: Lit,
    /// What the step claims suffices, in written order.
    pub refs: Vec<StepRef>,
    /// Provenance of the `HENCE` (source, line, kind = `HENCE`).
    pub origin: Origin,
}

/// One `TOTAL <relation> ON <set>` check, already evaluated at compile time (the
/// registries of sets and relation pairs are compile-time data, so the scan needs
/// no solver). The engine never proposes a witness — it only verifies the table
/// the author supplied (the LLM discharges the `∃` as `FACT` data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totality {
    /// The relation whose declared pairs were scanned.
    pub relation: String,
    /// The declared `SET` whose elements each need a witness pair.
    pub set: String,
    /// Set elements with **no** pair (`element relation _`) — empty means the
    /// check is fully served (`total (checked)`).
    pub missing: Vec<String>,
    /// Provenance of the `TOTAL` (source, line, kind = `TOTAL`).
    pub origin: Origin,
}

/// One `KNOWS`/`BELIEVES <agent> <literal>` attribution: the agent (a report-side
/// label, not an atom), the claimed literal, whether it is factive (`KNOWS`), and the
/// provenance. The solver checks the literal's model value per agent — factive:
/// FALSE → CONFLICT (you cannot know a falsehood), UNKNOWN → WARNING; non-factive:
/// FALSE → an informational note (exit 0, a false belief), else silent — plus a
/// per-agent coherence check (knowing φ and ¬φ → CONFLICT). It is **evaluative, not a
/// constraint**: no clause, no fact, the agent never enters the SAT core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    /// The agent the claim is attributed to (a bare label, not an atom).
    pub agent: String,
    /// The claimed literal (atom id + polarity from an optional `NOT`).
    pub lit: Lit,
    /// `true` for `KNOWS` (factive), `false` for `BELIEVES` (non-factive).
    pub factive: bool,
    /// Provenance of the `KNOWS`/`BELIEVES` (source, line, kind).
    pub origin: Origin,
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
