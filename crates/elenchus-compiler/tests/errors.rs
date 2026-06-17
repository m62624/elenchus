//! Snapshot tests for compiler-level errors (Display form).

use elenchus_compiler::{MemoryResolver, compile, compile_source};

#[test]
fn axiom_redefinition() {
    let src = "AXIOM e:\n    EXCLUSIVE\n        x a\n        x b\nAXIOM e:\n    EXCLUSIVE\n        x a\n        x c\n";
    let e = compile_source("main.vrf", src).unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}

#[test]
fn parse_error_is_wrapped_with_source() {
    let e = compile_source("main.vrf", "FACT lonely\n").unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}

#[test]
fn circular_import() {
    let mut r = MemoryResolver::new();
    r.add("a.vrf", "IMPORT \"b.vrf\"\n");
    r.add("b.vrf", "IMPORT \"a.vrf\"\n");
    let e = compile("a.vrf", &r).unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}

#[test]
fn missing_import() {
    let mut r = MemoryResolver::new();
    r.add("main.vrf", "IMPORT \"ghost.vrf\"\n");
    let e = compile("main.vrf", &r).unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}
