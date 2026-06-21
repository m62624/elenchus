//! The central syntax-error test file. Each malformed `.vrf` locks in the exact
//! diagnostic block a small model would see: the line number, the verbatim line,
//! the caret, the message, and the keyword's syntax card.
//!
//! Sections: per-keyword cards · multi-error (all at once) · the N-error limit ·
//! the recovery invariant · a smoke test on a real large file.

use elenchus_parser::{Diagnostics, KEYWORDS, card_for, parse};

/// The diagnostics of a source expected to fail (panics if it parsed).
fn diags(src: &str) -> Diagnostics {
    match parse(src) {
        Ok(_) => panic!("expected a parse error, but it parsed:\n{src}"),
        Err(d) => d,
    }
}

/// A failed parse rendered in full (every error) — the default `Display`.
fn err(src: &str) -> String {
    diags(src).render(None, None)
}

// --- per-keyword syntax cards ----------------------------------------------

#[test]
fn fact_missing_predicate() {
    insta::assert_snapshot!(err("FACT lonely\n"));
}

#[test]
fn not_missing_predicate() {
    insta::assert_snapshot!(err("NOT lonely\n"));
}

#[test]
fn assume_missing_predicate() {
    insta::assert_snapshot!(err("ASSUME lonely\n"));
}

#[test]
fn import_unterminated_string() {
    insta::assert_snapshot!(err("IMPORT \"physics.vrf\n"));
}

#[test]
fn premise_missing_colon() {
    insta::assert_snapshot!(err(r#"
        PREMISE modes
            EXCLUSIVE
                a b
                a c
        "#));
}

#[test]
fn rule_missing_colon() {
    insta::assert_snapshot!(err(r#"
        RULE r
            WHEN x a
            THEN x b
        "#));
}

#[test]
fn premise_implication_missing_then() {
    insta::assert_snapshot!(err(r#"
        PREMISE wings_need_bone:
            WHEN Creature.A has flying
        CHECK Creature.A
        "#));
}

#[test]
fn then_without_literal() {
    insta::assert_snapshot!(err(r#"
        RULE r:
            WHEN x a
            THEN
        "#));
}

#[test]
fn and_literal_missing() {
    insta::assert_snapshot!(err(r#"
        PREMISE g:
            WHEN x a
            AND
            THEN x b
        "#));
}

#[test]
fn list_premise_needs_two_atoms() {
    insta::assert_snapshot!(err(r#"
        PREMISE modes:
            EXCLUSIVE
                Sys mode idle
        CHECK Sys
        "#));
}

#[test]
fn rule_body_not_an_implication() {
    insta::assert_snapshot!(err(r#"
        RULE r:
            EXCLUSIVE
                x a
                x b
        "#));
}

#[test]
fn reserved_word_as_subject() {
    insta::assert_snapshot!(err("FACT WHEN has flying\n"));
}

#[test]
fn trailing_text_after_fact_atom() {
    insta::assert_snapshot!(err("FACT a b c d\n"));
}

// --- a line that is not a statement at all (general card) -------------------

#[test]
fn garbage_top_level_line() {
    insta::assert_snapshot!(err(r#"
        FACT a b
        %%% not a statement
        FACT c d
        "#));
}

// --- multi-error: every error in one pass ----------------------------------

/// Three broken top-level lines among valid ones, exercising the FACT, NOT and
/// THEN cards in one block list. Reused by the limit and recovery tests.
const BROKEN: &str = "\
FACT lonely
FACT a b
NOT also_lonely
CHECK
PREMISE p:
    WHEN x y
    THEN model uses too many words
";

#[test]
fn reports_every_error_in_one_pass() {
    insta::assert_snapshot!(err(BROKEN));
}

// --- the two caps: classes and places-per-class -----------------------------

/// Four broken lines — three in the FACT class, one in NOT — so the FACT class
/// has several places (for the per-class cap) and there are two classes (for
/// the class cap).
const REPEATED: &str = "\
FACT one
FACT two
FACT three
NOT four
";

#[test]
fn max_per_class_caps_places_within_a_class() {
    // All classes shown, but at most two places each (+ "… and N more").
    insta::assert_snapshot!(diags(REPEATED).render(None, Some(2)));
}

#[test]
fn max_classes_caps_the_number_of_classes() {
    // Only the first class shown, all its places (+ "… and N more classes").
    insta::assert_snapshot!(diags(REPEATED).render(Some(1), None));
}

// --- the recovery invariant -------------------------------------------------

#[test]
fn recovery_yields_exactly_one_error_per_broken_statement() {
    // No cascade: the broken PREMISE body does not spawn extra errors on its
    // WHEN/THEN lines, so exactly three statements failed.
    assert_eq!(diags(BROKEN).len(), 3);
}

#[test]
fn recovery_does_not_swallow_following_valid_statements() {
    // A valid FACT after a broken one still parses (it is found at resync).
    let src = "FACT lonely\nFACT good one\n";
    assert_eq!(diags(src).len(), 1);
}

// --- smoke: scale & a valid example ----------------------------------------

#[test]
fn many_errors_are_grouped_without_panicking() {
    // A large pile of broken lines (all one class) stays well-behaved: every
    // error is collected, grouped under one class, and the per-class cap trims it.
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!("FACT lonely{i}\n"));
    }
    let d = diags(&src);
    assert_eq!(d.len(), 200);
    let shown = d.render(None, Some(3));
    assert!(shown.contains("FACT  (200 problems)"), "{shown}");
    assert!(shown.contains("... and 197 more FACT problems"), "{shown}");
}

#[test]
fn the_extension_plan_example_is_valid() {
    // docs/examples/extension-plan.vrf is a real, well-formed program.
    let src = include_str!("../../../docs/examples/extension-plan.vrf");
    assert!(parse(src).is_ok(), "the example should parse cleanly");
}

// --- showcase: every keyword and every failure mode in one file ------------

#[test]
fn showcase_every_failure_mode() {
    // A long, sectioned file that breaks every keyword in turn — the flagship
    // snapshot a reviewer reads to see what each card looks like end to end.
    insta::assert_snapshot!(err(include_str!("fixtures/showcase.vrf")));
}

// --- syntax-card coverage: every keyword in the single table -------------

#[test]
fn every_reserved_word_has_a_complete_card() {
    for k in KEYWORDS {
        let card = card_for(k.text).unwrap_or_else(|| panic!("no syntax card for {}", k.text));
        assert!(!card.form.is_empty(), "{}: empty form", k.text);
        assert!(!card.gloss.is_empty(), "{}: empty gloss", k.text);
        assert!(!card.example.is_empty(), "{}: empty example", k.text);
        assert!(
            card.form.contains(k.text),
            "{}: form must name the keyword",
            k.text
        );
    }
}

#[test]
fn unknown_keyword_has_no_card() {
    assert!(card_for("DEFINITELY_NOT_A_KEYWORD").is_none());
}

#[test]
fn top_level_card_examples_actually_parse() {
    // The examples a model is told to copy must themselves be valid programs.
    // A trailing newline is not required: `eol` accepts EOF too.
    for kw in [
        "FACT", "NOT", "ASSUME", "IMPORT", "CHECK", "PREMISE", "RULE",
    ] {
        let example = card_for(kw).unwrap().example;
        assert!(
            parse(example).is_ok(),
            "{kw} card example must parse:\n{example}"
        );
    }
}
