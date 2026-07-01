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
use crate::unsat::retract_assumptions;

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
    Conflict, CoreItem, Derived, OrphanFact, Report, SimilarAtoms, Status, TraceReason, TraceStep,
    Warning,
};
pub use v3::V3;

/// The engine version (this crate's package version). Exposed so a wrapper —
/// e.g. the wasm/npm build, which carries its own, independent package version —
/// can report the *engine* version (and compare it to a skill's
/// `<!-- skill-version -->` marker) rather than its own.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Evaluate a compiled program: the three-valued forward pass, then the backward
/// pass on `BIDIRECTIONAL`.
pub fn solve(c: &Compiled) -> Report {
    let mut e = Eval::new(c);
    e.seed_facts();
    e.saturate_rules();
    e.check_premises();
    // Unwitnessed EXISTS → WARNING; must precede `finish` so it can raise the verdict.
    e.flag_unwitnessed_exists();
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
    // Advisory only: surface logically-inert assertions (orphan facts). Also
    // post-verdict, so it can never influence status/exit code.
    report.orphans = orphan_facts(c);
    // Advisory only: imports a file never references (computed at compile time,
    // carried through the IR). Never influences status/exit code.
    report.unused_imports = c.unused_imports.clone();
    // Advisory only: the per-port placeholders record (computed at compile time).
    // Never influences status/exit code.
    report.placeholders = c.placeholders.clone();
    report
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
