//! Reserved keywords and the subset that may begin a top-level statement.
//!
//! Keywords are ALWAYS CAPS (ASCII); an identifier may not equal any of them.

/// Reserved words — always CAPS, in full. An identifier may not equal any of these.
pub const RESERVED: &[&str] = &[
    "DOMAIN",
    "IMPORT",
    "AS",
    "FACT",
    "NOT",
    "ASSUME",
    "PREMISE",
    "RULE",
    "CHECK",
    "BIDIRECTIONAL",
    "WHEN",
    "AND",
    "OR",
    "THEN",
    "EXCLUSIVE",
    "FORBIDS",
    "ONEOF",
    "ATLEAST",
];

/// Keywords that may begin a *top-level* statement. Used by error recovery to
/// resynchronise: after a failed statement the parser skips lines until one
/// starts (after cosmetic indentation) with one of these, so a broken PREMISE
/// body never cascades into spurious errors on its `WHEN`/`THEN`/atom lines.
pub const TOP_LEVEL: &[&str] = &[
    "DOMAIN", "IMPORT", "FACT", "NOT", "ASSUME", "PREMISE", "RULE", "CHECK",
];

/// Whether `word` is a reserved keyword.
pub fn is_reserved(word: &str) -> bool {
    RESERVED.contains(&word)
}

/// Whether `word` can begin a top-level statement (used by recovery resync).
pub fn is_top_level(word: &str) -> bool {
    TOP_LEVEL.contains(&word)
}
