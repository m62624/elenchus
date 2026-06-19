//! Snapshot tests for the human-facing parse errors (`^--- here` rendering).
//! Each case is a deliberately malformed `.vrf`; the snapshot locks in the
//! exact line/column/message/caret a small model would see.

use elenchus_parser::parse;

/// Render a parse error to its `Display` form (or panic if it unexpectedly parsed).
fn err(src: &str) -> String {
    match parse(src) {
        Ok(_) => panic!("expected a parse error, but it parsed:\n{src}"),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn premise_implication_missing_then() {
    insta::assert_snapshot!(err(
        r#"
        PREMISE wings_need_bone:
            WHEN Creature.A has flying
        CHECK Creature.A
        "#
    ));
}

#[test]
fn list_premise_needs_two_atoms() {
    insta::assert_snapshot!(err(
        r#"
        PREMISE modes:
            EXCLUSIVE
                Sys mode idle
        CHECK Sys
        "#
    ));
}

#[test]
fn premise_missing_colon() {
    insta::assert_snapshot!(err(
        r#"
        PREMISE modes
            EXCLUSIVE
                a b
                a c
        "#
    ));
}

#[test]
fn fact_missing_predicate() {
    insta::assert_snapshot!(err("FACT lonely\n"));
}

#[test]
fn unterminated_import_string() {
    insta::assert_snapshot!(err("IMPORT \"physics.vrf\n"));
}

#[test]
fn reserved_word_as_subject() {
    insta::assert_snapshot!(err("FACT WHEN has flying\n"));
}

#[test]
fn garbage_top_level_line() {
    insta::assert_snapshot!(err(r#"
        FACT a b
        %%% not a statement
        FACT c d
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
fn rule_body_not_an_implication() {
    insta::assert_snapshot!(err(r#"
        RULE r:
            EXCLUSIVE
                x a
                x b
        "#));
}

#[test]
fn trailing_garbage_after_valid_program() {
    insta::assert_snapshot!(err(r#"
        FACT a b
        CHECK a
        ??? leftover
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
