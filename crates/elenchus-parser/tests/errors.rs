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

// Note: a single-word atom (`FACT lonely`) is no longer a *parse* error — it is a
// bare proposition (a `VAR` port reference). The "must be a declared VAR" guard
// lives in the compiler (`UndeclaredPort`), so the former `*_missing_predicate`
// parser snapshots moved there; see the compiler's port tests.

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
fn unless_without_literal() {
    insta::assert_snapshot!(err(r#"
        RULE r:
            WHEN x a
            THEN x b
            UNLESS
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
fn var_missing_name() {
    // A reserved word where the port name belongs trips the "VAR expects a name" card.
    insta::assert_snapshot!(err("VAR DEFAULT true\n"));
}

#[test]
fn var_default_without_a_value() {
    insta::assert_snapshot!(err("VAR k DEFAULT\n"));
}

#[test]
fn var_default_with_a_non_boolean() {
    insta::assert_snapshot!(err("VAR k DEFAULT maybe\n"));
}

#[test]
fn var_trailing_text() {
    insta::assert_snapshot!(err("VAR k extra\n"));
}

#[test]
fn provide_missing_colon() {
    insta::assert_snapshot!(err("PROVIDE k true\n"));
}

#[test]
fn provide_without_a_value() {
    insta::assert_snapshot!(err("PROVIDE k:\n"));
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
FACT a b c d
FACT a b
NOT a b c d
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
FACT a b c d
FACT a b c e
FACT a b c f
NOT a b c d
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
    let src = "FACT a b c d\nFACT good one\n";
    assert_eq!(diags(src).len(), 1);
}

// --- smoke: scale & a valid example ----------------------------------------

#[test]
fn many_errors_are_grouped_without_panicking() {
    // A large pile of broken lines (all one class) stays well-behaved: every
    // error is collected, grouped under one class, and the per-class cap trims it.
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!("FACT a b c d{i}\n"));
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
fn close_with_an_unknown_kind() {
    // A bogus closure kind groups under CLOSE and shows its card once.
    insta::assert_snapshot!(err("CLOSE deps SIDEWAYS\n"));
}

#[test]
fn exists_missing_in() {
    // EXISTS without `IN`/`WITNESS` groups under EXISTS and shows its card once.
    insta::assert_snapshot!(err("PREMISE p:\n    EXISTS h handlers\n        h does x\n"));
}

#[test]
fn exists_witness_missing_term() {
    // `WITNESS` with no term is committed under EXISTS and points at the header.
    insta::assert_snapshot!(err("PREMISE p:\n    EXISTS h WITNESS\n        h does x\n"));
}

#[test]
fn exists_missing_condition_line() {
    // A complete EXISTS header with no condition line under it is committed to the
    // "needs a condition line" message (covers the final EXISTS parse branch).
    insta::assert_snapshot!(err("PREMISE p:\n    EXISTS h WITNESS auth\n"));
}

#[test]
fn fact_because_missing_ground() {
    // `FACT <atom> BECAUSE` with no ground atom is committed under BECAUSE (its
    // message leads with the keyword), so it groups there and shows the BECAUSE card.
    insta::assert_snapshot!(err("FACT api healthy BECAUSE\n"));
}

#[test]
fn top_level_card_examples_actually_parse() {
    // The examples a model is told to copy must themselves be valid programs.
    // A trailing newline is not required: `eol` accepts EOF too. Drawn straight
    // from the keyword table, so every top-level statement is covered and no new
    // one can slip in untested.
    for k in KEYWORDS.iter().filter(|k| k.top_level) {
        assert!(
            parse(k.card.example).is_ok(),
            "{} card example must parse:\n{}",
            k.text,
            k.card.example
        );
    }
}
