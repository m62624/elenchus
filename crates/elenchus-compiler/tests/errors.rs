//! Snapshot tests for compiler-level errors (Display form).

use elenchus_compiler::{MemoryResolver, compile, compile_source};

#[test]
fn premise_redefinition() {
    let src = r#"
    DOMAIN m
    PREMISE e:
        EXCLUSIVE
            x a
            x b
    PREMISE e:
        EXCLUSIVE
            x a
            x c
    "#;
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
    r.add("a.vrf", "DOMAIN a\nIMPORT \"b.vrf\"\n");
    r.add("b.vrf", "DOMAIN b\nIMPORT \"a.vrf\"\n");
    let e = compile("a.vrf", &r).unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}

#[test]
fn missing_import() {
    let mut r = MemoryResolver::new();
    r.add("main.vrf", "DOMAIN main\nIMPORT \"ghost.vrf\"\n");
    let e = compile("main.vrf", &r).unwrap_err();
    insta::assert_snapshot!(format!("{e}"));
}
