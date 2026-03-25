mod support;

use std::path::Path;

use a2ex_onboarding::{
    ClaimDisposition, InstallBootstrapRequest, OnboardingAggregateStatus,
    OnboardingChecklistItemStatus, bootstrap_install,
};
use a2ex_skill_bundle::{BundleDocumentLifecycleChangeKind, BundleLifecycleClassification};
use reqwest::Url;
use rusqlite::{Connection, params};
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};
use tempfile::tempdir;

const ENTRY_SKILL_MD: &str = r#"---
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

Track spread divergences after local setup is complete.
"#;

const OWNER_SETUP_V1_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

const ENTRY_SKILL_V2_MD: &str = r#"---
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

Track spread divergences after local setup is complete.
"#;

const OWNER_SETUP_V2_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.11
---
# Required Secrets

- POLYMARKET_API_KEY
- KALSHI_API_KEY
"#;

fn mark_requirement_completed(path: &Path, install_id: &str, checklist_key: &str) -> usize {
    let connection = Connection::open(path).expect("state db should open for completion seeding");
    connection
        .execute(
            "UPDATE onboarding_checklist_items
             SET status = 'completed', completed_at = '2026-03-12T00:00:00Z', updated_at = '2026-03-12T00:00:00Z'
             WHERE install_id = ?1 AND checklist_key = ?2",
            params![install_id, checklist_key],
        )
        .expect("checklist table should allow completion seeding for resume coverage")
}

fn persisted_install_drift(path: &Path, install_id: &str) -> String {
    let connection = Connection::open(path).expect("state db should open for drift checks");
    connection
        .query_row(
            "SELECT bundle_drift_json FROM onboarding_installs WHERE install_id = ?1",
            [install_id],
            |row| row.get(0),
        )
        .expect("canonical install row must persist bundle_drift_json for resume diagnostics")
}

fn persisted_requirement_status(
    path: &Path,
    install_id: &str,
    checklist_key: &str,
) -> (String, Option<String>) {
    let connection =
        Connection::open(path).expect("state db should open for persisted checklist checks");
    connection
        .query_row(
            "SELECT status, completed_at FROM onboarding_checklist_items WHERE install_id = ?1 AND checklist_key = ?2",
            params![install_id, checklist_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("persisted checklist rows should remain queryable after reopen")
}

#[tokio::test]
async fn reopening_same_url_reuses_install_and_surfaces_bundle_drift_without_erasing_completion() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_V1_MD),
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
    .expect("initial live bootstrap should claim the canonical install before resume coverage");

    assert_eq!(
        mark_requirement_completed(
            &first.bootstrap.state_db_path,
            &first.install_id,
            "POLYMARKET_API_KEY",
        ),
        1,
        "resume coverage needs a persisted completed checklist item to prove unchanged keys survive bundle refresh"
    );

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_V2_MD),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_V2_MD),
    );

    let reopened = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect("same install url should reopen the existing install after bundle drift");

    assert_eq!(reopened.claim_disposition, ClaimDisposition::Reopened);
    assert_eq!(reopened.workspace_id, first.workspace_id);
    assert_eq!(reopened.install_id, first.install_id);
    assert!(
        reopened.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "POLYMARKET_API_KEY"
                && item.status == OnboardingChecklistItemStatus::Completed
                && item.completed_at.as_deref() == Some("2026-03-12T00:00:00Z")
        }),
        "resume path must preserve completion metadata for unchanged checklist keys instead of resetting progress"
    );
    assert!(
        reopened.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "KALSHI_API_KEY"
                && item.status == OnboardingChecklistItemStatus::Pending
                && item.completed_at.is_none()
        }),
        "resume path must add new checklist keys from the refreshed bundle without falsely marking them complete"
    );

    assert_eq!(
        reopened.onboarding.aggregate_status,
        OnboardingAggregateStatus::Drifted,
        "resume drift should project a drifted aggregate onboarding state for later inspection"
    );
    assert!(
        reopened.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "bundle_drift"
                && item.status == OnboardingChecklistItemStatus::Drifted
                && item.next_action.as_deref() == Some("review_bundle_drift")
        }),
        "resume path must add a readable drift checklist item instead of hiding changed bundle requirements"
    );

    let drift = reopened.onboarding.drift.as_ref().expect(
        "resume path must expose bundle drift explicitly instead of silently replacing progress",
    );
    assert_eq!(
        drift.classification,
        BundleLifecycleClassification::DocumentsChanged,
        "changed required setup document should classify as documents_changed drift"
    );
    assert!(
        drift.changed_documents.iter().any(|change| {
            change.document_id == "owner-setup"
                && matches!(
                    change.kind,
                    BundleDocumentLifecycleChangeKind::RevisionChanged
                        | BundleDocumentLifecycleChangeKind::ContentChanged
                )
        }),
        "drift payload must expose which document changed so later agents can localize the resume mismatch"
    );

    let persisted_poly = persisted_requirement_status(
        &reopened.bootstrap.state_db_path,
        &reopened.install_id,
        "POLYMARKET_API_KEY",
    );
    assert_eq!(persisted_poly.0, "completed");
    assert_eq!(persisted_poly.1.as_deref(), Some("2026-03-12T00:00:00Z"));

    let persisted_kalshi = persisted_requirement_status(
        &reopened.bootstrap.state_db_path,
        &reopened.install_id,
        "KALSHI_API_KEY",
    );
    assert_eq!(persisted_kalshi.0, "pending");
    assert_eq!(persisted_kalshi.1, None);

    let persisted_drift =
        persisted_install_drift(&reopened.bootstrap.state_db_path, &reopened.install_id);
    assert!(
        persisted_drift.contains("documents_changed")
            && persisted_drift.contains("owner-setup")
            && persisted_drift.contains("revision_changed"),
        "persisted install row must keep drift classification and changed-document evidence inspectable in state.db"
    );
}
