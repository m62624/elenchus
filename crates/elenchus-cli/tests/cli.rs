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
