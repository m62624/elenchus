//! A syntax error surfaces through `verify_source` as `CompileError::Parse`,
//! carrying the full diagnostics (rendered with the file label in the header).

use elenchus_solver::{CompileError, verify_source};

#[test]
fn syntax_error_propagates_as_parse_diagnostics() {
    // Trailing text after a FACT atom is a real syntax error (a single-word
    // `FACT lonely` now parses as a bare proposition).
    let err = verify_source("demo.vrf", "FACT a b c d\n").unwrap_err();
    match err {
        CompileError::Parse(diag) => {
            let shown = diag.render(None, None);
            assert!(
                shown.contains("RESULT: 1 syntax error in demo.vrf"),
                "shown = {shown}"
            );
            assert!(
                shown.contains("unexpected text after the FACT atom"),
                "shown = {shown}"
            );
        }
        other => panic!("expected a Parse error, got {other:?}"),
    }
}
