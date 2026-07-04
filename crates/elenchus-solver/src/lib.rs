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
//!
//! # Example
//!
//! ```
//! use elenchus_solver::{Status, verify_source};
//!
//! // `A has flying` fires the premise, but `A has wing` was never stated — so
//! // the engine cannot confirm the rule and reports WARNING (not CONFLICT).
//! let report = verify_source(
//!     "demo.vrf",
//!     "DOMAIN d\nFACT A has flying\nPREMISE w:\n    WHEN A has flying\n    THEN A has wing\n",
//! )
//! .unwrap();
//! assert_eq!(report.status, Status::Warning); // `A has wing` is UNKNOWN
//! println!("{report}"); // the full human report, ready to show a model
//! ```
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod sat;

mod analysis;
mod cnf;
mod eval;
mod report;
mod unsat;
mod v3;

use alloc::string::String;
use alloc::vec::Vec;

use elenchus_compiler::Compiled;

use crate::analysis::{orphan_facts, similar_atom_pairs};
use crate::eval::Eval;
use crate::unsat::{prove_goals, retract_assumptions, tried_hypotheses};

/// Re-exported so library users handling a [`CompileError::Parse`] can render the
/// syntax diagnostics with their own error limit (e.g. CLI `--max-errors`).
pub use elenchus_compiler::Diagnostics;
/// The filesystem-backed resolver (reads `IMPORT`s from disk). Only with `std`.
#[cfg(feature = "std")]
pub use elenchus_compiler::FileResolver;
pub use elenchus_compiler::{
    CompileError, MemoryResolver, PlaceholderInfo, PlaceholderStatus, PortBinding, Resolver,
    UnusedImport, compile, compile_source, compile_source_with, compile_with,
    normalize_import_path, read_data_bindings, read_data_source,
};
pub use report::{
    Conflict, CoreItem, Derived, FalseBelief, Fix, FixKind, OrphanFact, ProveOutcome, Proved,
    Report, SimilarAtoms, Status, TraceReason, TraceStep, Tried, TryOutcome, Warning,
};
pub use v3::V3;

/// The engine version (this crate's package version). Exposed so a wrapper —
/// e.g. the wasm/npm build, which carries its own, independent package version —
/// can report the *engine* version (and compare it to a skill's
/// `<!-- skill-version -->` marker) rather than its own.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Solve-time options for the `_opts` entry points. `Default` disables
/// everything, making them behave exactly like their plain counterparts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SolveOptions {
    /// A run-wide cap on SAT conflicts (see [`sat::Budget`]): every solver of
    /// the verification run draws from one shared pool; exceeding it aborts the
    /// whole run with [`VerifyError::ConflictBudget`]. Deterministic — the same
    /// program and limit abort identically on any hardware. `None` = unlimited.
    ///
    /// CDCL is worst-case exponential and no heuristic changes that (resolution
    /// lower bounds), so this cannot be derived from the program — it is a
    /// policy: how much work the caller is willing to pay. Real programs stay
    /// in the low thousands of conflicts; see the CLI `--max-conflicts` help
    /// for a calibrated recommendation.
    pub max_conflicts: Option<u64>,
}

/// Everything a `verify_*_opts` call can fail with: compilation, or a resource
/// abort. A resource abort is **not** a verdict — the program's status is
/// simply unknown; treat it like a build error, not like a `CONFLICT`.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// Parsing / compilation failed (see [`CompileError`]).
    Compile(CompileError),
    /// The conflict budget ([`SolveOptions::max_conflicts`]) ran out before the
    /// verification finished; `limit` echoes the configured cap.
    ConflictBudget {
        /// The configured [`SolveOptions::max_conflicts`] value that ran out.
        limit: u64,
    },
}

impl core::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VerifyError::Compile(e) => write!(f, "{e}"),
            VerifyError::ConflictBudget { limit } => {
                write!(f, "conflict budget exceeded ({limit} conflicts)")
            }
        }
    }
}

impl core::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            VerifyError::Compile(e) => Some(e),
            VerifyError::ConflictBudget { .. } => None,
        }
    }
}

impl From<CompileError> for VerifyError {
    fn from(e: CompileError) -> Self {
        VerifyError::Compile(e)
    }
}

/// Evaluate a compiled program: the three-valued forward pass, then the backward
/// pass on `BIDIRECTIONAL`.
pub fn solve(c: &Compiled) -> Report {
    match solve_impl(c, None) {
        Ok(report) => report,
        // No budget was installed, so no solver could have exhausted one.
        Err(sat::BudgetExhausted) => unreachable!("budget-free solve cannot exhaust"),
    }
}

/// [`solve`] under [`SolveOptions`]: identical output, unless the conflict
/// budget runs out first — then an explicit error instead of a verdict.
pub fn solve_opts(c: &Compiled, opts: &SolveOptions) -> Result<Report, VerifyError> {
    match opts.max_conflicts {
        None => Ok(solve(c)),
        Some(limit) => solve_impl(c, Some(sat::Budget::new(limit)))
            .map_err(|sat::BudgetExhausted| VerifyError::ConflictBudget { limit }),
    }
}

/// The one evaluation pipeline behind [`solve`] and [`solve_opts`]: clones of
/// the shared `budget` pool (if any) go to every SAT-backed phase — the
/// backward pass, core minimization, assumption retraction, and TRY counting.
fn solve_impl(c: &Compiled, budget: Option<sat::Budget>) -> Result<Report, sat::BudgetExhausted> {
    let mut e = Eval::new(c, budget.clone());
    e.seed_facts();
    e.saturate_rules();
    // Note each defeasible RULE whose default an established UNLESS suppressed —
    // read from the settled model. Informational only; never changes the verdict.
    e.flag_defeated_defaults();
    e.check_premises();
    // Unwitnessed EXISTS → WARNING; must precede `finish` so it can raise the verdict.
    e.flag_unwitnessed_exists();
    // FACT … BECAUSE justifications (L2): ground FALSE → CONFLICT, UNKNOWN → WARNING.
    // Also before `finish`, and after the forward pass has settled the model.
    e.check_justifications();
    // KNOWS/BELIEVES attributions (L6 modal/epistemic): factive knowledge that is FALSE
    // → CONFLICT, UNKNOWN → WARNING; a per-agent φ/¬φ incoherence → CONFLICT; a false
    // belief → a WARNING-level note. Also before `finish`, reading the settled model.
    e.check_attributions();
    let mut report = e.finish()?;
    // If the program is a CONFLICT but the facts/premises are consistent on their
    // own, the `ASSUME` hypotheses are what break it: name which to retract. The
    // verdict stays CONFLICT — this only adds the "drop one of these" hint and,
    // when it applies, supersedes the raw conflict/unsat-core pools (which would
    // otherwise point at the assumption clause itself).
    if report.status == Status::Conflict {
        let retract = retract_assumptions(c, budget.as_ref())?;
        if !retract.is_empty() {
            report.unsat_core = Vec::new();
            report.retract = retract;
        }
    }
    // Advisory only: surface likely atom-name typos. Computed after the verdict
    // so it can never influence status/exit code.
    report.hints = similar_atom_pairs(c);
    // Advisory only: surface logically-inert assertions (orphan facts). Also
    // post-verdict, so it can never influence status/exit code.
    report.orphans = orphan_facts(c);
    // Advisory only: imports a file never references (computed at compile time,
    // carried through the IR). Never influences status/exit code.
    report.unused_imports = c.unused_imports.clone();
    // Advisory only: the per-port placeholders record (computed at compile time).
    // Never influences status/exit code.
    report.placeholders = c.placeholders.clone();
    // Advisory only: the abduction (L5) side-check — for each `TRY <literal>`, whether
    // asserting it would close the open model, conflict, or leave it open. Post-verdict,
    // one bounded side-solve per hypothesis; never influences status/exit code.
    report.tried = tried_hypotheses(c, budget.as_ref())?;
    // Advisory only: the entailment (⊨) side-check — for each `PROVE <literal>` goal,
    // whether the theory entails it (PROVED), its negation (REFUTED), neither (OPEN),
    // or is itself inconsistent (VACUOUS). Post-verdict, two bounded side-solves per
    // goal; never influences status/exit code.
    report.goals = prove_goals(c, budget.as_ref())?;
    Ok(report)
}

/// Parse → compile → solve a single source.
pub fn verify_source(name: &str, src: &str) -> Result<Report, CompileError> {
    verify_source_with(name, src, &[])
}

/// Like [`verify_source`], but resolving declared `VAR` ports against external
/// `inputs` (`(name, binding)` pairs from CLI / API / data).
pub fn verify_source_with(
    name: &str,
    src: &str,
    inputs: &[(String, PortBinding)],
) -> Result<Report, CompileError> {
    Ok(solve(&compile_source_with(name, src, inputs)?))
}

/// Parse → compile (resolving imports) → solve, given a [`Resolver`].
pub fn verify<R: Resolver>(root: &str, resolver: &R) -> Result<Report, CompileError> {
    verify_with(root, resolver, &[])
}

/// Like [`verify`], but resolving declared `VAR` ports against external `inputs`.
pub fn verify_with<R: Resolver>(
    root: &str,
    resolver: &R,
    inputs: &[(String, PortBinding)],
) -> Result<Report, CompileError> {
    Ok(solve(&compile_with(root, resolver, inputs)?))
}

/// [`verify_source_with`] under [`SolveOptions`]. With default options the
/// report is byte-identical to [`verify_source_with`]'s; a conflict budget can
/// additionally fail with [`VerifyError::ConflictBudget`].
pub fn verify_source_opts(
    name: &str,
    src: &str,
    inputs: &[(String, PortBinding)],
    opts: &SolveOptions,
) -> Result<Report, VerifyError> {
    let compiled = compile_source_with(name, src, inputs)?;
    solve_opts(&compiled, opts)
}

/// [`verify_with`] under [`SolveOptions`]. With default options the report is
/// byte-identical to [`verify_with`]'s; a conflict budget can additionally
/// fail with [`VerifyError::ConflictBudget`].
pub fn verify_opts<R: Resolver>(
    root: &str,
    resolver: &R,
    inputs: &[(String, PortBinding)],
    opts: &SolveOptions,
) -> Result<Report, VerifyError> {
    let compiled = compile_with(root, resolver, inputs)?;
    solve_opts(&compiled, opts)
}
