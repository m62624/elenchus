//! Black-box tests of the `elenchus` binary (exit codes, formats, errors).

use std::io::Write;
use std::process::{Command, Stdio};

fn elenchus(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_elenchus"))
        .args(args)
        .output()
        .expect("run elenchus")
}

fn elenchus_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_elenchus"))
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
    let out = elenchus_with_stdin(&["-"], "FACT x a\nCHECK x\n");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("CONSISTENT"));
}

#[test]
fn text_consistent_exits_0() {
    let out = elenchus(&["--text", "FACT x a\nCHECK x\n"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("CONSISTENT"));
}

#[test]
fn text_warning_exits_1() {
    let out = elenchus(&[
        "--text",
        "FACT x a\nPREMISE w:\n WHEN x a\n THEN x b\nCHECK x\n",
    ]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn text_conflict_exits_2() {
    let out = elenchus(&["--text", "FACT x a\nNOT x a\nCHECK x\n"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_format_is_emitted() {
    let out = elenchus(&["--text", "FACT x a\nCHECK x\n", "--format", "json"]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("\"status\":\"CONSISTENT\""));
    assert!(s.contains("\"exit_code\":0"));
}

#[test]
fn whitespace_is_cosmetic_indented_equals_flat() {
    // A multi-line program written the readable way: indented WHEN/THEN block.
    let pretty = "FACT svc built\n\
                  PREMISE gate:\n    \
                      WHEN svc built\n    \
                      THEN svc ready\n\
                  FACT svc ready\n\
                  CHECK svc\n";
    // The exact same program with no indentation. The parser is line-oriented
    // (newlines required) but ignores leading/extra spaces, so this must be
    // byte-for-byte identical output.
    let flat = "FACT svc built\n\
                PREMISE gate:\n\
                WHEN svc built\n\
                THEN svc ready\n\
                FACT svc ready\n\
                CHECK svc\n";
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
    assert!(String::from_utf8_lossy(&out.stderr).contains("elenchus:"));
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
