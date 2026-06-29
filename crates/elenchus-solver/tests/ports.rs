//! Port reporting through the solver: the PLACEHOLDERS section (human + JSON),
//! `--hide-params` suppression, and that a supplied-but-unused port is not an
//! ORPHAN.

use elenchus_solver::{PortBinding, verify_source, verify_source_with};

fn set(name: &str, value: bool) -> (String, PortBinding) {
    (
        name.to_string(),
        PortBinding {
            value,
            origin: "CLI".to_string(),
        },
    )
}

#[test]
fn placeholders_render_in_human_and_json() {
    let r = verify_source("d.vrf", "DOMAIN d\nVAR k DEFAULT true\nFACT x a\nCHECK\n").unwrap();

    // Shown by default (and via `render_human(true)`).
    let shown = r.render_human(true);
    assert!(shown.contains("PARAM     k = true   (DEFAULT)"), "{shown}");

    // Suppressed by `--hide-params` (`render_human(false)`).
    let hidden = r.render_human(false);
    assert!(!hidden.contains("PARAM"), "{hidden}");

    // JSON always carries the section.
    let json = r.to_json();
    assert!(
        json.contains("\"placeholders\":[{\"key\":\"k\",\"status\":\"default\",\"value\":true,\"origin\":null}]"),
        "{json}"
    );
}

#[test]
fn supplied_value_is_reported_with_origin() {
    let r = verify_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k\nFACT x a\nCHECK\n",
        &[set("k", false)],
    )
    .unwrap();
    let shown = r.render_human(true);
    assert!(
        shown.contains("PARAM     k = false   (supplied: CLI)"),
        "{shown}"
    );
}

#[test]
fn supplied_but_unused_port_is_not_an_orphan() {
    // `k` is supplied but referenced by no premise/rule; it must NOT be flagged as
    // an ORPHAN (ports are registered as consumed). The plain `FACT x a` still is.
    let r = verify_source_with(
        "d.vrf",
        "DOMAIN d\nVAR k\nFACT x a\nCHECK\n",
        &[set("k", true)],
    )
    .unwrap();
    assert!(
        !r.orphans.iter().any(|o| o.atom == "d.k"),
        "port should not be an orphan: {:?}",
        r.orphans
    );
}
