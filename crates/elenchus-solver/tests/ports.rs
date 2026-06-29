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
fn all_three_placeholder_states_render_in_human_and_json() {
    // One port of each kind in a single report: supplied (with origin), DEFAULT,
    // and unset (UNKNOWN) — so every branch of the human PARAM lines and the JSON
    // placeholder encoding (value + origin present/absent) is exercised.
    let r = verify_source_with(
        "d.vrf",
        "DOMAIN d\nVAR sup\nVAR def DEFAULT true\nVAR un\nFACT x a\nCHECK\n",
        &[set("sup", false)],
    )
    .unwrap();

    let human = r.render_human(true);
    assert!(
        human.contains("PARAM     sup = false   (supplied: CLI)"),
        "{human}"
    );
    assert!(
        human.contains("PARAM     def = true   (DEFAULT)"),
        "{human}"
    );
    assert!(
        human.contains("PARAM     un = UNKNOWN   (no value supplied, no DEFAULT)"),
        "{human}"
    );

    let json = r.to_json();
    assert!(
        json.contains(
            "{\"key\":\"sup\",\"status\":\"supplied\",\"value\":false,\"origin\":\"CLI\"}"
        ),
        "{json}"
    );
    assert!(
        json.contains("{\"key\":\"def\",\"status\":\"default\",\"value\":true,\"origin\":null}"),
        "{json}"
    );
    assert!(
        json.contains("{\"key\":\"un\",\"status\":\"unset\",\"value\":null,\"origin\":null}"),
        "{json}"
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
