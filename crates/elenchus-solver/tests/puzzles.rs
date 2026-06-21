//! Hard, riddle-style scenarios. These exercise the SAT backward pass: a
//! constraint web where most atoms are UNKNOWN and the verdict only emerges from
//! case analysis the forward pass cannot do on its own.

use elenchus_solver::{Status, verify_source};

/// A 3×3 assignment puzzle: each of alice/bob/carol gets exactly one of
/// lead/dev/qa, and each role goes to exactly one person — a permutation.
/// Six ONEOF premises (~24 clauses), nine atoms, none asserted by default.
const ROLES: &str = "\
DOMAIN roles
PREMISE alice_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
PREMISE bob_role:
    ONEOF
        bob is lead
        bob is dev
        bob is qa
PREMISE carol_role:
    ONEOF
        carol is lead
        carol is dev
        carol is qa
PREMISE lead_one:
    ONEOF
        alice is lead
        bob is lead
        carol is lead
PREMISE dev_one:
    ONEOF
        alice is dev
        bob is dev
        carol is dev
PREMISE qa_one:
    ONEOF
        alice is qa
        bob is qa
        carol is qa
";

fn puzzle(givens_and_check: &str) -> Status {
    let mut src = String::from(ROLES);
    src.push_str(givens_and_check);
    verify_source("roles.vrf", &src).unwrap().status
}

#[test]
fn well_posed_puzzle_has_a_unique_solution() {
    // alice = lead, and bob is not qa.  Deductions: lead_one ⇒ bob,carol ≠ lead;
    // bob ≠ qa ⇒ bob = dev; then carol = qa. Exactly one full model → CONSISTENT.
    let s = puzzle(
        r#"
        FACT alice is lead
        NOT bob is qa
        CHECK alice BIDIRECTIONAL
        "#,
    );
    assert_eq!(s, Status::Consistent);
}

#[test]
fn under_clued_puzzle_is_underdetermined() {
    // Drop the "bob is not qa" clue: bob/carol may swap dev↔qa → two models.
    let s = puzzle(
        r#"
        FACT alice is lead
        CHECK alice BIDIRECTIONAL
        "#,
    );
    assert_eq!(s, Status::Underdetermined);
}

#[test]
fn over_clued_puzzle_is_conflict() {
    // Two people as lead violates lead_one (at most one) — the forward pass
    // already catches it (a pairwise EXCLUSIVE clause goes all-TRUE).
    let s = puzzle(
        r#"
        FACT alice is lead
        FACT bob is lead
        CHECK alice
        "#,
    );
    assert_eq!(s, Status::Conflict);
}

#[test]
fn no_givens_is_underdetermined() {
    // A bare permutation has 3! = 6 solutions.
    let s = puzzle(
        r#"
        CHECK alice BIDIRECTIONAL
        "#,
    );
    assert_eq!(s, Status::Underdetermined);
}

/// A ~20-condition deployment gate: a long implication chain where the first
/// links fire but the rest are blocked on UNKNOWN data — a forward WARNING web.
const DEPLOY: &str = "\
DOMAIN deploy
FACT svc built
FACT svc unit_tested
NOT  svc deprecated

PREMISE build_implies_artifact:
    WHEN svc built
    THEN svc has_artifact

PREMISE tested_chain:
    WHEN svc unit_tested
    THEN svc integration_tested

PREMISE ready_needs_all:
    WHEN svc has_artifact
    AND  svc integration_tested
    AND  svc security_scanned
    THEN svc release_ready

PREMISE deploy_needs_ready:
    WHEN svc release_ready
    AND  NOT svc deprecated
    THEN svc can_deploy

PREMISE exclusive_env:
    EXCLUSIVE
        svc env staging
        svc env prod

RULE artifact_rule:
    WHEN svc built
    THEN svc has_artifact
";

#[test]
fn deploy_chain_is_warning_when_data_is_missing() {
    // built + unit_tested are known, so the first implications are live, but
    // security_scanned (and onwards) is UNKNOWN → the chain is blocked → WARNING.
    let r = verify_source("deploy.vrf", &format!("{DEPLOY}CHECK svc\n")).unwrap();
    assert_eq!(r.status, Status::Warning);
    assert!(r.conflicts.is_empty());
    assert!(!r.warnings.is_empty());
    // the RULE fires on `built`, deriving the artifact.
    assert!(
        r.derived
            .iter()
            .any(|d| d.atom == "deploy.svc has_artifact")
    );
}

#[test]
fn deploy_chain_completed_is_consistent() {
    // Supply the missing facts; every implication is satisfied → CONSISTENT.
    let extra = r#"
        FACT svc has_artifact
        FACT svc integration_tested
        FACT svc security_scanned
        FACT svc release_ready
        FACT svc can_deploy
        CHECK svc
        "#;
    let r = verify_source("deploy.vrf", &format!("{DEPLOY}{extra}")).unwrap();
    assert_eq!(r.status, Status::Consistent);
}

#[test]
fn deploy_chain_violation_is_conflict() {
    // release_ready holds and not deprecated, but can_deploy is denied →
    // deploy_needs_ready is violated.
    let extra = r#"
        FACT svc release_ready
        NOT svc can_deploy
        CHECK svc
        "#;
    let r = verify_source("deploy.vrf", &format!("{DEPLOY}{extra}")).unwrap();
    assert_eq!(r.status, Status::Conflict);
}
