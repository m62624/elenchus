//! The single source of truth for the language's keywords.
//!
//! Every keyword's CAPS spelling lives exactly once, in [`kw`]. The [`KEYWORDS`]
//! table attaches each one's role (may it begin a statement?) and its syntax card
//! (canonical form, one-line gloss, a copy-paste example). Everything else in the
//! workspace that needs to know a keyword — the reserved-word check, recovery
//! resync, the diagnostic cards, the parser's `tag(...)` calls, and the compiler's
//! provenance `kind` strings — derives from this table or references a [`kw`]
//! constant, so a keyword string is never written twice.

/// The CAPS spelling of every keyword — the one place each literal appears.
pub mod kw {
    #![allow(missing_docs)] // each constant is its own keyword; the name says it
    pub const DOMAIN: &str = "DOMAIN";
    pub const IMPORT: &str = "IMPORT";
    pub const AS: &str = "AS";
    pub const FACT: &str = "FACT";
    pub const NOT: &str = "NOT";
    pub const ASSUME: &str = "ASSUME";
    pub const PREMISE: &str = "PREMISE";
    pub const RULE: &str = "RULE";
    pub const CHECK: &str = "CHECK";
    pub const BIDIRECTIONAL: &str = "BIDIRECTIONAL";
    pub const WHEN: &str = "WHEN";
    pub const THEN: &str = "THEN";
    pub const UNLESS: &str = "UNLESS";
    pub const AND: &str = "AND";
    pub const OR: &str = "OR";
    pub const EXCLUSIVE: &str = "EXCLUSIVE";
    pub const FORBIDS: &str = "FORBIDS";
    pub const ONEOF: &str = "ONEOF";
    pub const ATLEAST: &str = "ATLEAST";
    pub const SET: &str = "SET";
    pub const FOR: &str = "FOR";
    pub const EACH: &str = "EACH";
    pub const IN: &str = "IN";
    pub const CLOSE: &str = "CLOSE";
    pub const TRANSITIVE: &str = "TRANSITIVE";
    pub const SYMMETRIC: &str = "SYMMETRIC";
    pub const REFLEXIVE: &str = "REFLEXIVE";
    pub const EQUIVALENCE: &str = "EQUIVALENCE";
    pub const SCC: &str = "SCC";
    pub const EXISTS: &str = "EXISTS";
    pub const WITNESS: &str = "WITNESS";
    pub const BECAUSE: &str = "BECAUSE";
    pub const VAR: &str = "VAR";
    pub const DEFAULT: &str = "DEFAULT";
    pub const PROVIDE: &str = "PROVIDE";
}

/// The correct-syntax reference for one keyword: its canonical written form (with
/// `<slots>` and `[optional]` parts), a one-line plain meaning, and a real, valid
/// example a model can copy. Mirrors `docs/SPEC.md`, "DSL: keywords".
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// The canonical written form.
    pub form: &'static str,
    /// A one-line plain meaning.
    pub gloss: &'static str,
    /// A real, valid example line.
    pub example: &'static str,
}

/// One keyword: its spelling, whether it may begin a top-level statement, and its
/// syntax card.
pub struct Keyword {
    /// The CAPS spelling (a [`kw`] constant).
    pub text: &'static str,
    /// Whether it may begin a top-level statement — used by recovery resync and
    /// the "expected one of these statements" menu.
    pub top_level: bool,
    /// The keyword's syntax card.
    pub card: Card,
}

/// Shorthand for one [`Card`].
const fn card(form: &'static str, gloss: &'static str, example: &'static str) -> Card {
    Card {
        form,
        gloss,
        example,
    }
}

/// Every keyword, in teaching order. The top-level statements come first (so
/// filtering `top_level` yields the menu order), then `IMPORT`/`AS`, then the
/// body and modifier words.
pub const KEYWORDS: &[Keyword] = &[
    Keyword {
        text: kw::DOMAIN,
        top_level: true,
        card: card(
            "DOMAIN <name>",
            "declare this file's domain (required, once, first) — the namespace bare atoms fall into",
            "DOMAIN physics",
        ),
    },
    Keyword {
        text: kw::FACT,
        top_level: true,
        card: card(
            "FACT [<domain>.]<Subject> <predicate> [<object>]",
            "assert an atom is TRUE",
            "FACT socrates is human",
        ),
    },
    Keyword {
        text: kw::NOT,
        top_level: true,
        card: card(
            "NOT <Subject> <predicate> [<object>]",
            "assert an atom is FALSE",
            "NOT socrates is immortal",
        ),
    },
    Keyword {
        text: kw::ASSUME,
        top_level: true,
        card: card(
            "ASSUME [NOT] <Subject> <predicate> [<object>]",
            "a soft, retractable hypothesis (the solver may ask you to drop it)",
            "ASSUME release is_ready",
        ),
    },
    Keyword {
        text: kw::PREMISE,
        top_level: true,
        card: card(
            "PREMISE <name>:  then a list body or a WHEN ... THEN implication",
            "a checked first principle (a constraint that must hold)",
            "PREMISE wings:\n    WHEN bird has feathers\n    THEN bird can_fly",
        ),
    },
    Keyword {
        text: kw::RULE,
        top_level: true,
        card: card(
            "RULE <name>:  then a WHEN ... THEN implication",
            "an inference rule: when the WHEN holds, it derives the THEN as a fact",
            "RULE mortal:\n    WHEN x is human\n    THEN x is mortal",
        ),
    },
    Keyword {
        text: kw::CHECK,
        top_level: true,
        card: card(
            "CHECK [<subject>] [BIDIRECTIONAL]",
            "run the query; an optional subject narrows the report",
            "CHECK socrates",
        ),
    },
    Keyword {
        text: kw::SET,
        top_level: true,
        card: card(
            "SET <name>  then one element per line (>= 1)",
            "declare a finite set of elements to quantify a PREMISE/RULE over (FOR EACH ... IN)",
            "SET tasks\n    deploy\n    backup",
        ),
    },
    Keyword {
        text: kw::CLOSE,
        top_level: true,
        card: card(
            "CLOSE <relation> TRANSITIVE",
            "make a relation transitive at compile time (a->b, b->c implies a->c); a cycle is an error",
            "CLOSE depends_on TRANSITIVE",
        ),
    },
    Keyword {
        text: kw::VAR,
        top_level: true,
        card: card(
            "VAR <name> [DEFAULT true|false]",
            "declare an external boolean port (a bare proposition); its value comes from CLI/API/data, else DEFAULT, else UNKNOWN",
            "VAR db_ready DEFAULT false",
        ),
    },
    Keyword {
        text: kw::PROVIDE,
        top_level: true,
        card: card(
            "PROVIDE [<domain>.]<port|atom>: true|false",
            "bind an external value (a VAR port, or an atom); a domain. prefix disambiguates across imports. In a --data file or alongside the program",
            "PROVIDE db_ready: true",
        ),
    },
    Keyword {
        text: kw::IMPORT,
        top_level: true,
        card: card(
            "IMPORT \"<path>\" [AS <alias>]",
            "pull in another .vrf source for reuse; its atoms are named <domain>.<atom>",
            "IMPORT \"physics.vrf\"",
        ),
    },
    Keyword {
        text: kw::AS,
        top_level: false,
        card: card(
            "IMPORT \"<path>\" AS <alias>",
            "give the imported domain a local name to reference it by",
            "IMPORT \"physics.vrf\" AS phys",
        ),
    },
    Keyword {
        text: kw::FOR,
        top_level: false,
        card: card(
            "PREMISE <name> FOR EACH <binder> IN <set>:  then the usual body",
            "quantify a PREMISE/RULE: instantiate its body once per element of <set>, binding <binder>",
            "PREMISE colored FOR EACH n IN nodes:\n    ONEOF\n        n is red\n        n is blue",
        ),
    },
    Keyword {
        text: kw::EACH,
        top_level: false,
        card: card(
            "FOR EACH <binder> IN <set>",
            "part of the FOR EACH ... IN quantifier on a PREMISE/RULE header",
            "FOR EACH t IN tasks",
        ),
    },
    Keyword {
        text: kw::IN,
        top_level: false,
        card: card(
            "FOR EACH <binder> IN <set>",
            "names the declared SET a FOR EACH quantifier ranges over",
            "FOR EACH t IN tasks",
        ),
    },
    Keyword {
        text: kw::WHEN,
        top_level: false,
        card: card(
            "WHEN <literal>   (literal = [NOT] <Subject> <predicate> [<object>])",
            "the condition (if-part) of an implication; continue with AND/OR",
            "WHEN motor over_100",
        ),
    },
    Keyword {
        text: kw::THEN,
        top_level: false,
        card: card(
            "THEN <literal>",
            "the conclusion (then-part) of an implication; continue with AND/OR",
            "THEN motor uses fast_path",
        ),
    },
    Keyword {
        text: kw::UNLESS,
        top_level: false,
        card: card(
            "RULE … THEN <literal> UNLESS <literal>  (one UNLESS per line, repeatable)",
            "a defeasible exception on a RULE: the rule derives its THEN by default, but is suppressed when an UNLESS literal is established TRUE (FALSE/UNKNOWN lets the default stand)",
            "RULE fly:\n    WHEN x is bird\n    THEN x can_fly\n    UNLESS x is penguin",
        ),
    },
    Keyword {
        text: kw::AND,
        top_level: false,
        card: card(
            "AND <literal>",
            "extend the current WHEN or THEN group (all must hold); do not mix with OR",
            "AND motor is reviewed",
        ),
    },
    Keyword {
        text: kw::OR,
        top_level: false,
        card: card(
            "OR <literal>",
            "extend the current WHEN or THEN group (at least one holds); do not mix with AND",
            "OR motor is hotfixed",
        ),
    },
    Keyword {
        text: kw::EXCLUSIVE,
        top_level: false,
        card: card(
            "EXCLUSIVE  then one atom per line (>= 2 atoms)",
            "at most one of the listed atoms may be TRUE",
            "EXCLUSIVE\n    light is on\n    light is off",
        ),
    },
    Keyword {
        text: kw::FORBIDS,
        top_level: false,
        card: card(
            "FORBIDS  then one atom per line (>= 2 atoms)",
            "at most one may be TRUE (a synonym of EXCLUSIVE)",
            "FORBIDS\n    door is open\n    door is locked",
        ),
    },
    Keyword {
        text: kw::ONEOF,
        top_level: false,
        card: card(
            "ONEOF  then one atom per line (>= 2 atoms)",
            "exactly one of the listed atoms is TRUE",
            "ONEOF\n    mode is idle\n    mode is run",
        ),
    },
    Keyword {
        text: kw::ATLEAST,
        top_level: false,
        card: card(
            "ATLEAST  then one atom per line (>= 2 atoms)",
            "at least one of the listed atoms is TRUE",
            "ATLEAST\n    plan has tests\n    plan has review",
        ),
    },
    Keyword {
        text: kw::EXISTS,
        top_level: false,
        card: card(
            "EXISTS <binder> IN <set> (or WITNESS <term>)  then one condition line using the binder",
            "at least one element satisfies the condition (the existential; the dual of FOR EACH)",
            "EXISTS h IN handlers\n    h handles request",
        ),
    },
    Keyword {
        text: kw::WITNESS,
        top_level: false,
        card: card(
            "EXISTS <binder> WITNESS <term>  then one condition line using the binder",
            "prove EXISTS by naming the one element that satisfies it — needs no SET; grounds to a single atom",
            "EXISTS h WITNESS auth_service\n    h is ready",
        ),
    },
    Keyword {
        text: kw::BECAUSE,
        top_level: false,
        card: card(
            "FACT <atom> BECAUSE <atom>",
            "name the ground a FACT rests on; the engine checks it holds (ground FALSE -> CONFLICT, UNKNOWN -> WARNING)",
            "FACT api healthy BECAUSE db reachable",
        ),
    },
    Keyword {
        text: kw::BIDIRECTIONAL,
        top_level: false,
        card: card(
            "CHECK [<subject>] BIDIRECTIONAL",
            "a CHECK modifier: also run the backward pass (finds UNDERDETERMINED)",
            "CHECK BIDIRECTIONAL",
        ),
    },
    Keyword {
        text: kw::TRANSITIVE,
        top_level: false,
        card: card(
            "CLOSE <relation> TRANSITIVE",
            "the closure kind for CLOSE: a->b and b->c implies a->c (a cycle is an error)",
            "CLOSE depends_on TRANSITIVE",
        ),
    },
    Keyword {
        text: kw::SYMMETRIC,
        top_level: false,
        card: card(
            "CLOSE <relation> SYMMETRIC",
            "a closure kind for CLOSE: a->b implies b->a (a two-way relation)",
            "CLOSE conflicts_with SYMMETRIC",
        ),
    },
    Keyword {
        text: kw::REFLEXIVE,
        top_level: false,
        card: card(
            "CLOSE <relation> REFLEXIVE",
            "a closure kind for CLOSE: add x->x for every node the relation mentions",
            "CLOSE compatible_with REFLEXIVE",
        ),
    },
    Keyword {
        text: kw::EQUIVALENCE,
        top_level: false,
        card: card(
            "CLOSE <relation> EQUIVALENCE",
            "a closure kind for CLOSE: reflexive + symmetric + transitive — groups nodes into classes",
            "CLOSE same_team EQUIVALENCE",
        ),
    },
    Keyword {
        text: kw::SCC,
        top_level: false,
        card: card(
            "CLOSE <relation> SCC",
            "a closure kind for CLOSE: group nodes that reach each other (mutual reachability; isolates directed cycles)",
            "CLOSE depends_on SCC",
        ),
    },
    Keyword {
        text: kw::DEFAULT,
        top_level: false,
        card: card(
            "VAR <name> DEFAULT true|false",
            "the fallback value of a VAR port when no external value is supplied",
            "VAR db_ready DEFAULT false",
        ),
    },
];

/// Whether `word` is a reserved keyword.
pub fn is_reserved(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.text == word)
}

/// Whether `word` can begin a top-level statement (used by recovery resync).
pub fn is_top_level(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.top_level && k.text == word)
}

/// The syntax card for `keyword`, or `None` if it is not a known keyword.
pub fn card_for(keyword: &str) -> Option<&'static Card> {
    KEYWORDS.iter().find(|k| k.text == keyword).map(|k| &k.card)
}

/// The keywords that may begin a top-level statement, in teaching order — shown
/// as the menu when an error is not tied to one specific keyword.
pub fn top_level_forms() -> impl Iterator<Item = &'static Keyword> {
    KEYWORDS.iter().filter(|k| k.top_level)
}

/// The top-level keywords joined as an English menu — `"A, B, …, or Z"`, in
/// teaching order. The single source for the "expected a statement" diagnostic,
/// so that menu can never drift from [`KEYWORDS`] as keywords are added.
pub fn top_level_menu() -> alloc::string::String {
    use alloc::string::String;
    let words: alloc::vec::Vec<&'static str> = top_level_forms().map(|k| k.text).collect();
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
            if i + 1 == words.len() {
                out.push_str("or ");
            }
        }
        out.push_str(w);
    }
    out
}

/// The first keyword named anywhere in `message` (selects its card in a
/// diagnostic). Splits on non-alphabetic characters and returns the first token
/// that is a keyword.
pub fn keyword_in(message: &str) -> Option<&'static str> {
    message
        .split(|c: char| !c.is_ascii_alphabetic())
        .find_map(|w| KEYWORDS.iter().map(|k| k.text).find(|t| *t == w))
}
