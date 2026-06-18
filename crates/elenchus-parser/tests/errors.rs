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
fn axiom_implication_missing_then() {
    insta::assert_snapshot!(err(
        "AXIOM wings_need_bone:\n    WHEN Creature.A has flying\nCHECK Creature.A\n"
    ));
}

#[test]
fn list_axiom_needs_two_atoms() {
    insta::assert_snapshot!(err(
        "AXIOM modes:\n    EXCLUSIVE\n        Sys mode idle\nCHECK Sys\n"
    ));
}

#[test]
fn axiom_missing_colon() {
    insta::assert_snapshot!(err(
        "AXIOM modes\n    EXCLUSIVE\n        a b\n        a c\n"
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
    insta::assert_snapshot!(err("FACT a b\n%%% not a statement\nFACT c d\n"));
}

#[test]
fn then_without_literal() {
    insta::assert_snapshot!(err("RULE r:\n    WHEN x a\n    THEN\n"));
}

#[test]
fn rule_body_not_an_implication() {
    insta::assert_snapshot!(err("RULE r:\n    EXCLUSIVE\n        x a\n        x b\n"));
}

#[test]
fn trailing_garbage_after_valid_program() {
    insta::assert_snapshot!(err("FACT a b\nCHECK a\n??? leftover\n"));
}

#[test]
fn and_literal_missing() {
    insta::assert_snapshot!(err("AXIOM g:\n    WHEN x a\n    AND\n    THEN x b\n"));
}
