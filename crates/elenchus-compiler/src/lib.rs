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

mod closure;
mod compiler;
mod data;
mod domain;
mod error;
mod ir;
mod ports;
mod resolver;
mod sig;
mod subst;

#[cfg(test)]
mod tests;

use alloc::string::String;
use core::fmt::Write as _;
use sha2::{Digest, Sha256};

use resolver::resolve_graph;

pub use compiler::Compiler;
pub use data::{read_data_bindings, read_data_source};
/// Re-exported so downstream crates name one source of truth: [`Diagnostics`] for
/// the syntax errors carried by [`CompileError::Parse`], and [`kw`] for the
/// keyword spellings an [`Origin::kind`] is built from (so the solver matches a
/// `kind` against `kw::PREMISE`, not a re-typed `"PREMISE"`).
pub use elenchus_parser::{Diagnostics, kw};
pub use error::{CompileError, UnknownValue, levenshtein};
pub use ir::{
    AtomId, AtomKey, Check, Clause, Compiled, Fact, KIND_UNSAT, Lit, Origin, PlaceholderInfo,
    PlaceholderStatus, PortBinding, Rule, UnusedImport, Value,
};
pub use resolver::{FileResolver, MemoryResolver, Resolver, normalize_import_path};

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
