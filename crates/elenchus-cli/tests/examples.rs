//! Doc-rot guard: pin the shipped example programs (`docs/examples/*.vrf`) and the
//! SKILL.md capstone to their verdicts, so the language can never drift away from
//! its own documentation without a test going red. Exit code = verdict
//! (0 CONSISTENT / 1 WARNING|UNDERDETERMINED / 2 CONFLICT).

use std::process::Command;

fn run_file(name: &str) -> i32 {
    let path = format!("{}/../../docs/examples/{name}", env!("CARGO_MANIFEST_DIR"));
    Command::new(env!("CARGO_BIN_EXE_elenchus-cli"))
        .arg(path)
        .output()
        .expect("run elenchus")
        .status
        .code()
        .expect("exit code")
}

fn run_text(program: &str) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_elenchus-cli"))
        .args(["--text", program])
        .output()
        .expect("run elenchus")
        .status
        .code()
        .expect("exit code")
}

/// Run a `docs/examples` program with a `docs/examples` `--data` file.
fn run_file_with_data(name: &str, data: &str) -> i32 {
    let dir = format!("{}/../../docs/examples", env!("CARGO_MANIFEST_DIR"));
    Command::new(env!("CARGO_BIN_EXE_elenchus-cli"))
        .arg(format!("{dir}/{name}"))
        .args(["--data", &format!("{dir}/{data}")])
        .output()
        .expect("run elenchus")
        .status
        .code()
        .expect("exit code")
}

#[test]
fn shipped_examples_match_their_verdicts() {
    let cases = [
        ("conflict.vrf", 2),
        ("creature.vrf", 1),
        ("defeasible.vrf", 0),
        ("import-demo.vrf", 2),
        ("justification.vrf", 0),
        ("physics.vrf", 1),
        ("roles-puzzle.vrf", 0),
        ("socrates.vrf", 2),
        ("witness.vrf", 0),
    ];
    for (name, want) in cases {
        assert_eq!(run_file(name), want, "{name} changed its verdict");
    }
}

/// The SKILL.md "ship to prod?" capstone (example 8), minus the final CHECK so the
/// three stages can append their own facts. Kept byte-identical to the skill.
const CAPSTONE: &str = r#"DOMAIN ship
PREMISE one_stage:
    ONEOF
        rel in_dev
        rel in_staging
        rel in_prod
PREMISE prod_needs_deployable:
    WHEN rel in_prod
    THEN rel deployable
PREMISE deploy_gate:
    WHEN rel code_reviewed
    AND  rel tests_green
    AND  rel security_scanned
    THEN rel deployable
RULE migration_needs_backup:
    WHEN rel has_migration
    THEN rel needs_backup
PREMISE backup_gate:
    WHEN rel needs_backup
    THEN rel backup_verified
PREMISE prod_needs_safety:
    WHEN rel in_prod
    THEN rel has_rollback
    OR   rel has_feature_flag
FACT rel in_prod
FACT rel code_reviewed
FACT rel tests_green
FACT rel security_scanned
FACT rel deployable
FACT rel has_migration
FACT rel has_feature_flag
"#;

#[test]
fn deploy_gate_template_is_filled_from_its_data_file() {
    // The shipped templating example: deploy-gate.vrf declares VAR ports; standalone
    // a defaulted-false gate can't be satisfied → CONFLICT (exit 2). The companion
    // values-only data file (PROVIDE lines) supplies every gate → CONSISTENT (0).
    // Pins both the example pair and the `--data` fill path to their verdicts.
    assert_eq!(
        run_file("deploy-gate.vrf"),
        2,
        "the template alone must not pass the gate"
    );
    assert_eq!(
        run_file_with_data("deploy-gate.vrf", "deploy-gate.data.vrf"),
        0,
        "the data file must satisfy every gate"
    );
}

#[test]
fn skill_capstone_stage_a_warns_about_the_missing_backup() {
    // Believed ready, but a migration owes a backup that was never verified.
    assert_eq!(run_text(&format!("{CAPSTONE}CHECK rel\n")), 1);
}

#[test]
fn skill_capstone_stage_b_consistent_when_backup_verified() {
    assert_eq!(
        run_text(&format!("{CAPSTONE}FACT rel backup_verified\nCHECK rel\n")),
        0
    );
}

#[test]
fn skill_capstone_stage_c_conflict_when_backup_not_verified() {
    assert_eq!(
        run_text(&format!("{CAPSTONE}NOT rel backup_verified\nCHECK rel\n")),
        2
    );
}
