//! Compile errors and the shared "did you mean" suggestion helpers.
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use thiserror::Error;

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
    /// A `PREMISE` carried an `UNLESS` exception. Defeasible defaulting only makes
    /// sense on a `RULE` (which *derives* a value it can withhold); a `PREMISE` is a
    /// hard constraint with nothing to defeat. Model it as a `RULE`, or drop the
    /// `UNLESS`.
    #[error(
        "UNLESS is a RULE-only defeasible exception; premise '{name}' is a hard \
         constraint with nothing to defeat — make it a RULE, or drop the UNLESS"
    )]
    PremiseException {
        /// The offending premise name.
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

/// `" — did you mean \`x\`?"` for an undeclared set name, or empty when no
/// declared set name is close enough.
pub(crate) fn nearest_set_suggestion(set: &str, sets: &BTreeMap<String, Vec<String>>) -> String {
    let names: Vec<&str> = sets.keys().map(String::as_str).collect();
    did_you_mean(set, &names)
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
pub(crate) fn nearest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
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
pub(crate) fn did_you_mean(word: &str, candidates: &[&str]) -> String {
    match nearest(word, candidates) {
        Some(s) => alloc::format!(" — did you mean `{s}`?"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

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
}
