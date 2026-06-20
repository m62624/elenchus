//! Per-keyword syntax cards: the "how is this written correctly" reference a
//! diagnostic shows under a broken line.
//!
//! Each keyword gets a zero-sized type implementing [`KeywordSyntax`]; the data
//! is the single source of truth for the canonical form, a one-line meaning, and
//! a real valid example (mirrors `docs/SPEC.md`, "DSL: keywords"). [`syntax_for`]
//! maps a keyword string to its card; [`TOP_LEVEL_FORMS`] lists the statements
//! valid at the top level (shown when no specific keyword is implicated).

/// The correct-syntax reference for one keyword.
pub trait KeywordSyntax {
    /// The canonical written form, with `<slots>` in angle brackets and
    /// `[optional]` parts in square brackets.
    fn form(&self) -> &'static str;
    /// A one-line plain meaning of the keyword.
    fn gloss(&self) -> &'static str;
    /// A real, valid example line a model can copy.
    fn example(&self) -> &'static str;
}

/// Define a zero-sized card type per keyword and the [`syntax_for`] dispatcher
/// from one data table — the table stays the single source of truth.
macro_rules! cards {
    ($($ty:ident => $kw:literal, $form:expr, $gloss:expr, $ex:expr;)*) => {
        $(
            struct $ty;
            impl KeywordSyntax for $ty {
                fn form(&self) -> &'static str { $form }
                fn gloss(&self) -> &'static str { $gloss }
                fn example(&self) -> &'static str { $ex }
            }
        )*

        /// The syntax card for `keyword`, or `None` if it is not a known keyword.
        pub fn syntax_for(keyword: &str) -> Option<&'static dyn KeywordSyntax> {
            Some(match keyword {
                $( $kw => &$ty, )*
                _ => return None,
            })
        }
    };
}

cards! {
    Fact => "FACT",
        "FACT <Subject> <predicate> [<object>]",
        "assert an atom is TRUE",
        "FACT socrates is human";
    Not => "NOT",
        "NOT <Subject> <predicate> [<object>]",
        "assert an atom is FALSE",
        "NOT socrates is immortal";
    Assume => "ASSUME",
        "ASSUME [NOT] <Subject> <predicate> [<object>]",
        "a soft, retractable hypothesis (the solver may ask you to drop it)",
        "ASSUME release is_ready";
    Premise => "PREMISE",
        "PREMISE <name>:  then a list body or a WHEN ... THEN implication",
        "a checked first principle (a constraint that must hold)",
        "PREMISE wings:\n    WHEN bird has feathers\n    THEN bird can_fly";
    Rule => "RULE",
        "RULE <name>:  then a WHEN ... THEN implication",
        "an inference rule: when the WHEN holds, it derives the THEN as a fact",
        "RULE mortal:\n    WHEN x is human\n    THEN x is mortal";
    Check => "CHECK",
        "CHECK [<subject>] [BIDIRECTIONAL]",
        "run the query; an optional subject narrows the report",
        "CHECK socrates";
    Import => "IMPORT",
        "IMPORT \"<path>\"",
        "pull in another .vrf source for reuse",
        "IMPORT \"physics.vrf\"";
    When => "WHEN",
        "WHEN <literal>   (literal = [NOT] <Subject> <predicate> [<object>])",
        "the condition (if-part) of an implication; continue with AND/OR",
        "WHEN motor over_100";
    Then => "THEN",
        "THEN <literal>",
        "the conclusion (then-part) of an implication; continue with AND/OR",
        "THEN motor uses fast_path";
    And => "AND",
        "AND <literal>",
        "extend the current WHEN or THEN group (all must hold); do not mix with OR",
        "AND motor is reviewed";
    Or => "OR",
        "OR <literal>",
        "extend the current WHEN or THEN group (at least one holds); do not mix with AND",
        "OR motor is hotfixed";
    Exclusive => "EXCLUSIVE",
        "EXCLUSIVE  then one atom per line (>= 2 atoms)",
        "at most one of the listed atoms may be TRUE",
        "EXCLUSIVE\n    light is on\n    light is off";
    Forbids => "FORBIDS",
        "FORBIDS  then one atom per line (>= 2 atoms)",
        "at most one may be TRUE (a synonym of EXCLUSIVE)",
        "FORBIDS\n    door is open\n    door is locked";
    OneOf => "ONEOF",
        "ONEOF  then one atom per line (>= 2 atoms)",
        "exactly one of the listed atoms is TRUE",
        "ONEOF\n    mode is idle\n    mode is run";
    AtLeast => "ATLEAST",
        "ATLEAST  then one atom per line (>= 2 atoms)",
        "at least one of the listed atoms is TRUE",
        "ATLEAST\n    plan has tests\n    plan has review";
    Bidirectional => "BIDIRECTIONAL",
        "CHECK [<subject>] BIDIRECTIONAL",
        "a CHECK modifier: also run the backward pass (finds UNDERDETERMINED)",
        "CHECK BIDIRECTIONAL";
}

/// Keywords that may begin a top-level statement, in teaching order. Shown as a
/// general card when an error is not tied to one specific keyword (e.g. a line
/// that starts with none of them).
pub const TOP_LEVEL_FORMS: &[&str] = &[
    "FACT", "NOT", "ASSUME", "PREMISE", "RULE", "CHECK", "IMPORT",
];
