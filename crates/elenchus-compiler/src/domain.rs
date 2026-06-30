//! Per-file domain context: resolve a `domain.` prefix to a canonical domain,
//! and build an owned [`AtomKey`] from a borrowed parser atom.
use crate::error::CompileError;
use crate::ir::AtomKey;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use elenchus_parser::Atom;

/// The domain context of one file being compiled: its own declared domain (where
/// bare atoms fall) and the local names — aliases or imported domain names — it
/// may reference other domains by. Resolving an atom's optional `domain.` prefix
/// against this context yields its canonical [`AtomKey`] domain.
pub(crate) struct DomainCtx {
    /// The file's own declared domain (the target for unqualified atoms).
    pub(crate) current: String,
    /// `local name -> canonical domain` for every name visible in this file
    /// (always includes `current -> current`, plus one entry per `IMPORT`).
    pub(crate) aliases: BTreeMap<String, String>,
}

impl DomainCtx {
    /// Resolve an atom's optional `domain.` prefix to a canonical domain name.
    /// `None` → the file's own domain; a prefix not imported here is an error.
    pub(crate) fn resolve(&self, prefix: Option<&str>) -> Result<String, CompileError> {
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
    pub(crate) fn key(&self, a: &Atom) -> Result<AtomKey, CompileError> {
        Ok(AtomKey {
            domain: self.resolve(a.domain)?,
            subject: a.subject.to_string(),
            predicate: a.predicate.map(|p| p.to_string()),
            object: a.object.map(|o| o.to_string()),
        })
    }
}
