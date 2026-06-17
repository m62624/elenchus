//! elenchus-compiler — compiles parsed elenchus DSL into a canonical clause IR.
//!
//! The compiler is source-agnostic: it consumes strings. A file is merely one
//! way to reuse a body of facts/axioms, resolved through a `Resolver` trait
//! (mirroring vsm-grammar's `SourceResolver` / `MemoryResolver` / `FileResolver`).
//!
//! - Atoms `(subject, predicate, object?)` are interned to dense `u32` ids and
//!   unify globally across imported sources.
//! - Sources/axioms are content-addressed (sha256, mirroring vsm-guard's CAS)
//!   purely for dedup, redefinition detection, and provenance — never for
//!   namespacing atoms.
//!
//! The actual SAT/model-finding is a separate future crate (`elenchus-solver`,
//! a no_std port of varisat); this crate stops at the canonical clause IR.
#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;
