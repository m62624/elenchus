//! Black-box tests of the `elenchus` binary (exit codes, formats, errors).

use std::io::Write;
use std::process::{Command, Stdio};

fn elenchus(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_elenchus-cli"))
        .args(args)
        .output()
        .expect("run elenchus")
}

fn elenchus_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_elenchus-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn elenchus");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait elenchus")
}

#[test]
fn no_args_prints_help_instead_of_waiting_for_stdin() {
    let out = elenchus(&[]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stderr.contains("no input provided"));
    assert!(stdout.contains("Usage: elenchus"));
}

#[test]
fn dash_reads_stdin() {
    let out = elenchus_with_stdin(
        &["-"],
        r#"
        FACT x a
        CHECK x
        "#,
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("CONSISTENT"));
}

#[test]
fn text_consistent_exits_0() {
    let out = elenchus(&[
        "--text",
        r#"
        FACT x a
        CHECK x
        "#,
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("CONSISTENT"));
}

#[test]
fn text_warning_exits_1() {
    let out = elenchus(&[
        "--text",
        r#"
        FACT x a
        PREMISE w:
            WHEN x a
            THEN x b
        CHECK x
        "#,
    ]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn text_conflict_exits_2() {
    let out = elenchus(&[
        "--text",
        r#"
        FACT x a
        NOT x a
        CHECK x
        "#,
    ]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_format_is_emitted() {
    let out = elenchus(&[
        "--text",
        r#"
        FACT x a
        CHECK x
        "#,
        "--format",
        "json",
    ]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"status\":\"CONSISTENT\""));
    assert!(s.contains("\"exit_code\":0"));
}

#[test]
fn whitespace_is_cosmetic_indented_equals_flat() {
    // The same program written fully indented vs. flat at column 0. Indentation
    // is cosmetic everywhere, so both must produce a byte-identical report.
    let pretty = r#"
        FACT svc built
        PREMISE gate:
            WHEN svc built
            THEN svc ready
        FACT svc ready
        CHECK svc
        "#;
    let flat = r#"
FACT svc built
PREMISE gate:
WHEN svc built
THEN svc ready
FACT svc ready
CHECK svc
"#;
    let a = elenchus(&["--text", pretty]);
    let b = elenchus(&["--text", flat]);
    assert_eq!(
        a.status.code(),
        Some(0),
        "indented form should be CONSISTENT"
    );
    assert_eq!(a.status.code(), b.status.code());
    assert_eq!(a.stdout, b.stdout, "indentation must not change the report");
}

#[test]
fn help_points_agents_at_the_skill() {
    let out = elenchus(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("skill"), "help should mention the skill: {s}");
    assert!(
        s.contains("github.com/m62624/elenchus"),
        "help should link the project"
    );
}

#[test]
fn parse_error_exits_2_with_message() {
    let out = elenchus(&["--text", "FACT lonely\n"]);
    assert_eq!(out.status.code(), Some(2));
    // A syntax error prints the diagnostic block (header + caret + card), not a
    // bare `elenchus:` one-liner.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("RESULT: 1 syntax error"),
        "stderr = {stderr}"
    );
    assert!(stderr.contains("FACT expects an atom"), "stderr = {stderr}");
}

#[test]
fn max_errors_caps_the_block_count() {
    // Three broken lines; --max-errors 1 shows one block and summarises the rest.
    let out = elenchus(&[
        "--text",
        "FACT lonely\nNOT lonely2\nFACT also bad words here\n",
        "--max-errors",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("RESULT: 3 syntax errors"),
        "stderr = {stderr}"
    );
    assert!(stderr.contains("(showing 1 of 3"), "stderr = {stderr}");
}

#[test]
fn all_syntax_errors_shown_by_default() {
    // No --max-errors → every block is rendered, no footer.
    let out = elenchus(&[
        "--text",
        "FACT lonely\nNOT lonely2\nFACT also bad words here\n",
    ]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("RESULT: 3 syntax errors"),
        "stderr = {stderr}"
    );
    assert_eq!(
        stderr.matches("problem :").count(),
        3,
        "all three blocks shown: {stderr}"
    );
    assert!(
        !stderr.contains("showing"),
        "no footer when all shown: {stderr}"
    );
}

#[test]
fn consequent_or_is_satisfied_by_one_disjunct() {
    // gateway prod ⇒ (auth staging ∨ api staging); auth staging holds ⇒ CONSISTENT.
    // (Would be CONFLICT if OR were wrongly treated as AND.)
    let out = elenchus(&[
        "--text",
        r#"
        PREMISE gw:
            WHEN gateway is_prod
            THEN auth is_staging
            OR api is_staging
        FACT gateway is_prod
        FACT auth is_staging
        NOT api is_staging
        CHECK
        "#,
    ]);
    assert_eq!(out.status.code(), Some(0), "one disjunct true → CONSISTENT");
}

#[test]
fn consequent_or_conflicts_when_all_disjuncts_false() {
    // Both disjuncts false while the antecedent holds ⇒ CONFLICT.
    let out = elenchus(&[
        "--text",
        r#"
        PREMISE gw:
            WHEN gateway is_prod
            THEN auth is_staging
            OR api is_staging
        FACT gateway is_prod
        NOT auth is_staging
        NOT api is_staging
        CHECK
        "#,
    ]);
    assert_eq!(out.status.code(), Some(2), "all disjuncts false → CONFLICT");
}

#[test]
fn antecedent_or_fires_on_any_disjunct() {
    // (x a ∨ x b) ⇒ x c ; x b holds but NOT x c ⇒ CONFLICT.
    // (Would be CONSISTENT if OR were wrongly treated as AND, since x a is UNKNOWN.)
    let out = elenchus(&[
        "--text",
        r#"
        PREMISE r:
            WHEN x a
            OR x b
            THEN x c
        FACT x b
        NOT x c
        CHECK
        "#,
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "OR antecedent fires on x b → CONFLICT"
    );
}

#[test]
fn clashing_assumptions_exit_2_and_print_retract() {
    // Facts/premises are consistent; the ASSUME guesses can't all hold. The CLI
    // exits 2 (CONFLICT) and the human report leads with a RETRACT block naming
    // the assumptions — clear enough for a small model to act on.
    let out = elenchus(&[
        "--text",
        r#"
        FACT rel reviewed
        PREMISE prod_needs_safety:
            WHEN rel in_prod
            THEN rel has_rollback
            OR   rel has_feature_flag
        ASSUME rel in_prod
        ASSUME NOT rel has_rollback
        ASSUME NOT rel has_feature_flag
        CHECK rel
        "#,
    ]);
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("RETRACT"), "{stdout}");
    assert!(stdout.contains("ASSUME rel in_prod"), "{stdout}");
}

#[test]
fn compatible_assumption_exits_0() {
    let out = elenchus(&["--text", "ASSUME x a\nFACT y b\nCHECK\n"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("CONSISTENT"));
}

#[test]
fn file_with_imports_is_resolved() {
    // import-demo.vrf imports physics.vrf relative to itself — a deliberate conflict.
    let path = format!(
        "{}/../../docs/examples/import-demo.vrf",
        env!("CARGO_MANIFEST_DIR")
    );
    let out = elenchus(&[&path]);
    assert_eq!(out.status.code(), Some(2));
}
