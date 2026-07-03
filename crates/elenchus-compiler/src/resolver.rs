//! Source-agnostic import resolution: the [`Resolver`] trait, in-memory and file
//! backings, path normalization, and the iterative import-graph traversal.
use crate::domain::DomainCtx;
use crate::error::CompileError;
use crate::hash_hex;
use crate::ir::UnusedImport;
use crate::subst::collect_prefixes;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use elenchus_parser::Statement;

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
pub(crate) struct ResolvedFile {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) ctx: DomainCtx,
}

/// One `IMPORT` edge: the optional local alias, the resolved child path, and the
/// `IMPORT` line (for the unused-import advisory).
pub(crate) struct ImportEdge {
    pub(crate) alias: Option<String>,
    pub(crate) child_path: String,
    pub(crate) line: u32,
}

/// A discovered source during graph resolution: its first-seen path, raw text,
/// declared domain, import edges, and the set of domain prefixes its atoms use
/// (`None` = its own domain; `Some(p)` = a `p.` prefix) — used to flag imports
/// that the file never references.
pub(crate) struct DiscoveredFile {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) domain: String,
    pub(crate) edges: Vec<ImportEdge>,
    pub(crate) used_prefixes: BTreeSet<Option<String>>,
}

/// Resolve the whole import graph reachable from `root` into a flat list of
/// [`ResolvedFile`]s, each distinct source appearing once.
///
/// Iterative depth-first traversal with an explicit work stack (`Enter`/`Exit`
/// frames) — no native recursion, so depth is unbounded without risking a stack
/// overflow. Memoized by content hash (a diamond/repeat is visited once); a hash
/// re-encountered while still on the active path is a [`CompileError::CircularImport`].
pub(crate) fn resolve_graph<R: Resolver>(
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
    // `UnusedImport` derives `Ord` over every field, so two entries that compare
    // equal are fully identical — an unstable sort can never show a different
    // order than a stable one here.
    unused.sort_unstable();

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

/// Parse one source, tagging any syntax [`Diagnostics`] with its file label so a
/// `CompileError::Parse` names the right file. The single spelling of "parse, and
/// on failure attach the file" — shared by the inline, resolved, and import paths.
pub(crate) fn parse_tagged<'a>(
    file: &str,
    content: &'a str,
) -> Result<elenchus_parser::Program<'a>, CompileError> {
    elenchus_parser::parse(content).map_err(|mut diag| {
        diag.set_file(file);
        CompileError::Parse(diag)
    })
}

/// The single `DOMAIN` a source declares, or an error if it has none or several.
pub(crate) fn extract_domain(
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
