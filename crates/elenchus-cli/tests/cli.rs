//! Black-box tests of the `elenchus` binary (exit codes, formats, errors).

use std::process::Command;

fn elenchus(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_elenchus"))
        .args(args)
        .output()
        .expect("run elenchus")
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
        "FACT x a\nAXIOM w:\n WHEN x a\n THEN x b\nCHECK x\n",
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
