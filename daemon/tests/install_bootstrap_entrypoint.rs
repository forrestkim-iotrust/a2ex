mod support;

use a2ex_onboarding::{
    ClaimDisposition, InstallBootstrapRequest, OnboardingAggregateStatus, bootstrap_install,
};
use a2ex_skill_bundle::{
    BundleDiagnosticCode, BundleDiagnosticPhase, BundleLifecycleClassification,
    SkillBundleInterpretationStatus,
};
use reqwest::Url;
use rusqlite::Connection;
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};
use tempfile::tempdir;

const READY_ENTRY_SKILL_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.12
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.10
---
# Overview

Track spread divergences after setup is complete.
"#;

const OWNER_SETUP_READY_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Notes

This bundle is ready once attached.
"#;

const READY_ENTRY_SKILL_V2_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.13
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.11
---
# Overview

Track spread divergences after setup is complete.
"#;

const OWNER_SETUP_NEEDS_SECRET_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.11
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

fn persisted_readiness_payloads(path: &std::path::Path) -> (String, String, String) {
    let connection =
        Connection::open(path).expect("state db should open for persisted readiness checks");
    connection
        .query_row(
            "SELECT readiness_status, readiness_blockers_json, readiness_diagnostics_json FROM onboarding_installs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("blocked reopen should leave one persisted onboarding install row")
}

#[tokio::test]
async fn live_entrypoint_attaches_bundle_identity_and_projects_ready_status() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_READY_MD),
        ),
    ])
    .await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let result = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect(
        "live bootstrap entrypoint should return a structured response for reachable install URLs",
    );

    assert_eq!(
        result.claim_disposition,
        ClaimDisposition::Claimed,
        "first live bootstrap should claim the workspace for the attached install URL"
    );
    assert_eq!(
        result.attached_bundle_url, entry_url,
        "bootstrap response must persist and return the attached bundle URL identity"
    );
    assert_eq!(
        result.readiness.status,
        SkillBundleInterpretationStatus::InterpretedReady,
        "live ready bundle should project readiness_status=interpreted_ready from the bootstrap entry surface"
    );
    assert!(
        result.readiness.blockers.is_empty(),
        "ready bundle should not return readiness blockers"
    );
    assert!(
        result.readiness.diagnostics.is_empty(),
        "ready bundle should not leak loader diagnostics into the success path"
    );
    assert!(
        !result.bootstrap.used_remote_control_plane,
        "live install bootstrap must still prove local authority with used_remote_control_plane=false"
    );
}

#[tokio::test]
async fn same_url_reopen_returns_blocked_readiness_with_typed_payloads() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_READY_MD),
        ),
    ])
    .await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");
    let missing_owner_setup_url = entry_url
        .join("docs/owner-setup.md")
        .expect("missing owner setup url resolves");

    let first = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("initial live bootstrap should claim the workspace before reopen coverage");

    harness.remove_fixture("/bundles/prediction-spread-arb/docs/owner-setup.md");

    let reopened = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect(
        "same live URL should reopen the existing workspace even when readiness becomes blocked",
    );

    assert_eq!(
        reopened.claim_disposition,
        ClaimDisposition::Reopened,
        "same install URL plus same workspace root must reopen rather than claim a duplicate install"
    );
    assert_eq!(
        reopened.workspace_id, first.workspace_id,
        "blocked reopen path must keep the canonical workspace_id stable"
    );
    assert_eq!(
        reopened.install_id, first.install_id,
        "blocked reopen path must keep the canonical install_id stable"
    );
    assert_eq!(
        reopened.readiness.status,
        SkillBundleInterpretationStatus::Blocked,
        "missing required bundle document must project blocked readiness instead of a generic bootstrap failure"
    );
    assert!(
        reopened.readiness.blockers.iter().any(|blocker| {
            blocker.blocker_key == "owner-setup:missing_required_document"
                && blocker.diagnostic_code == Some(BundleDiagnosticCode::MissingRequiredDocument)
                && blocker.diagnostic_phase == Some(BundleDiagnosticPhase::LoadManifest)
                && blocker.evidence.iter().any(|evidence| {
                    evidence.document_id == "owner-setup"
                        && evidence.source_url == missing_owner_setup_url
                })
        }),
        "blocked reopen path must surface a typed blocker with missing_required_document provenance"
    );
    assert!(
        reopened.readiness.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == BundleDiagnosticCode::MissingRequiredDocument
                && diagnostic.phase == BundleDiagnosticPhase::LoadManifest
                && diagnostic.document_id.as_deref() == Some("owner-setup")
                && diagnostic.source_url.as_ref() == Some(&missing_owner_setup_url)
        }),
        "blocked reopen path must preserve the underlying typed diagnostic payload alongside blocker projection"
    );

    let (persisted_status, persisted_blockers, persisted_diagnostics) =
        persisted_readiness_payloads(&reopened.bootstrap.state_db_path);
    assert_eq!(
        persisted_status, "blocked",
        "blocked reopen path must persist readiness_status=blocked for later onboarding resume logic"
    );
    assert!(
        persisted_blockers.contains("owner-setup:missing_required_document"),
        "persisted blocked state must keep blocker localization inspectable in the canonical install row"
    );
    assert!(
        persisted_diagnostics.contains("missing_required_document")
            && persisted_diagnostics.contains("owner-setup")
            && persisted_diagnostics.contains(missing_owner_setup_url.as_str()),
        "persisted blocked state must keep the typed diagnostic payload durable in state.db"
    );
}

#[tokio::test]
async fn same_url_reopen_refreshes_onboarding_drift_from_attached_bundle_url() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(READY_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_READY_MD),
        ),
    ])
    .await;
    let workspace_root = tempdir().expect("workspace tempdir");
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let first = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url.clone(),
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: None,
        expected_install_id: None,
    })
    .await
    .expect("initial live bootstrap should claim the workspace before reopen drift coverage");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(READY_ENTRY_SKILL_V2_MD),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_NEEDS_SECRET_MD),
    );

    let reopened = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect("same install url should reopen the canonical install and refresh onboarding drift");

    assert_eq!(reopened.claim_disposition, ClaimDisposition::Reopened);
    assert_eq!(reopened.workspace_id, first.workspace_id);
    assert_eq!(reopened.install_id, first.install_id);
    assert_eq!(
        reopened.onboarding.aggregate_status,
        OnboardingAggregateStatus::Drifted,
        "reopen drift should surface through the shipped onboarding aggregate status"
    );
    let drift = reopened
        .onboarding
        .drift
        .as_ref()
        .expect("reopen drift should be projected in the onboarding payload");
    assert_eq!(
        drift.classification,
        BundleLifecycleClassification::DocumentsChanged,
        "document revision drift should classify as documents_changed"
    );
    assert!(
        reopened.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "bundle_drift"
                && item.next_action.as_deref() == Some("review_bundle_drift")
        }),
        "reopen drift should create a durable checklist item that later agents can inspect"
    );
    assert!(
        reopened.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "POLYMARKET_API_KEY"
                && item.next_action.as_deref() == Some("provide_local_secret")
        }),
        "refreshed onboarding payload should still include newly required setup actions from the drifted bundle"
    );
}
