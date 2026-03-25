mod support;

use std::path::Path;

use a2ex_onboarding::{
    ClaimDisposition, InstallBootstrapRequest, OnboardingAggregateStatus,
    OnboardingChecklistItemStatus, OnboardingChecklistSourceKind, bootstrap_install,
};
use a2ex_skill_bundle::SkillBundleInterpretationStatus;
use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, params};
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

const OWNER_SETUP_WITH_SECRET_REQUIREMENT_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY

# Example Payload

Never persist raw values like sk-live-should-never-persist from operator notes.
"#;

fn persisted_onboarding_status(path: &Path, install_id: &str) -> (String, Option<String>) {
    let connection =
        Connection::open(path).expect("state db should open for onboarding status checks");
    connection
        .query_row(
            "SELECT onboarding_status, bundle_drift_json FROM onboarding_installs WHERE install_id = ?1",
            [install_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("onboarding_installs must persist onboarding_status and bundle_drift_json for the canonical install row")
}

fn persisted_checklist_row(
    path: &Path,
    install_id: &str,
    checklist_key: &str,
) -> Option<(
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
)> {
    let connection = Connection::open(path).expect("state db should open for checklist checks");
    connection
        .query_row(
            "SELECT checklist_key, source_kind, status, blocker_reason, next_action, evidence_json, completed_at
             FROM onboarding_checklist_items
             WHERE install_id = ?1 AND checklist_key = ?2",
            params![install_id, checklist_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .expect("onboarding_checklist_items table should exist and be queryable")
}

#[tokio::test]
async fn first_bootstrap_projects_persisted_checklist_metadata_with_redacted_evidence() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_WITH_SECRET_REQUIREMENT_MD),
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
        "live bootstrap should return a structured onboarding payload for reachable bundle urls",
    );

    assert_eq!(result.claim_disposition, ClaimDisposition::Claimed);
    assert_eq!(
        result.readiness.status,
        SkillBundleInterpretationStatus::NeedsSetup,
        "required secrets should still project needs_setup through the live bootstrap entrypoint"
    );
    assert_eq!(
        result.onboarding.aggregate_status,
        OnboardingAggregateStatus::NeedsAction,
        "needs_setup readiness should project aggregate onboarding status that later agents can inspect"
    );
    assert!(
        result.onboarding.checklist_items.iter().any(|item| {
            item.checklist_key == "POLYMARKET_API_KEY"
                && item.source_kind == OnboardingChecklistSourceKind::SetupRequirement
                && item.status == OnboardingChecklistItemStatus::Pending
                && item.blocker_reason.is_none()
                && item.next_action.as_deref() == Some("provide_local_secret")
                && item.completed_at.is_none()
                && item.evidence.iter().any(|evidence| {
                    evidence.document_id == "owner-setup"
                        && evidence.section_slug.as_deref() == Some("required-secrets")
                        && evidence.redacted_summary.as_deref()
                            == Some("requirement_identity:POLYMARKET_API_KEY")
                })
        }),
        "bootstrap result must expose a typed checklist item with stable key, pending status, next action, and redacted evidence provenance"
    );

    let (persisted_status, persisted_drift_json) =
        persisted_onboarding_status(&result.bootstrap.state_db_path, &result.install_id);
    assert_eq!(
        persisted_status, "needs_action",
        "canonical install row must persist aggregate onboarding status for resume and diagnostics"
    );
    assert!(
        persisted_drift_json
            .as_deref()
            .is_none_or(|value| value == "null" || value.is_empty()),
        "first bootstrap should not persist bundle drift for an unchanged bundle"
    );

    let persisted_requirement = persisted_checklist_row(
        &result.bootstrap.state_db_path,
        &result.install_id,
        "POLYMARKET_API_KEY",
    )
    .expect("required secret should persist as a durable checklist item row");
    assert_eq!(persisted_requirement.0, "POLYMARKET_API_KEY");
    assert_eq!(persisted_requirement.1, "setup_requirement");
    assert_eq!(persisted_requirement.2, "pending");
    assert_eq!(persisted_requirement.3, None);
    assert_eq!(
        persisted_requirement.4.as_deref(),
        Some("provide_local_secret")
    );
    assert_eq!(persisted_requirement.6, None);
    assert!(
        persisted_requirement.5.contains("owner-setup")
            && persisted_requirement.5.contains("required-secrets")
            && persisted_requirement.5.contains("POLYMARKET_API_KEY"),
        "persisted evidence_json must preserve safe requirement identity and provenance for future inspection"
    );
    assert!(
        !persisted_requirement
            .5
            .contains("sk-live-should-never-persist"),
        "persisted evidence_json must redact raw secret-like values instead of storing operator payloads"
    );
}
