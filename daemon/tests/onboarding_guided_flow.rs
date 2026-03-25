mod support;

use a2ex_onboarding::{
    ClaimDisposition, GuidedOnboardingAction, GuidedOnboardingActionKind,
    GuidedOnboardingActionRequest, GuidedOnboardingError, GuidedOnboardingStateRequest,
    InstallBootstrapRequest, OnboardingAggregateStatus, apply_guided_onboarding_action,
    bootstrap_install, read_guided_onboarding,
};
use a2ex_skill_bundle::BundleLifecycleClassification;
use reqwest::Url;
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};
use tempfile::tempdir;

const GUIDED_ENTRY_SKILL_MD: &str = r#"---
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

# Owner Decisions

- Approve max spread budget.
"#;

const OWNER_SETUP_WITH_SECRET_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

const GUIDED_ENTRY_SKILL_V2_MD: &str = r#"---
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

# Owner Decisions

- Approve max spread budget.
"#;

const OWNER_SETUP_WITH_TWO_SECRETS_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.11
---
# Required Secrets

- POLYMARKET_API_KEY
- KALSHI_API_KEY
"#;

#[tokio::test]
async fn guided_flow_orders_steps_exposes_one_recommended_action_and_reopens_same_install() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(GUIDED_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_WITH_SECRET_MD),
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
    .expect("bootstrap should claim the canonical install before guided-flow assertions run");

    assert_eq!(first.claim_disposition, ClaimDisposition::Claimed);
    assert_eq!(
        first.onboarding.aggregate_status,
        OnboardingAggregateStatus::NeedsAction
    );

    let guided = read_guided_onboarding(GuidedOnboardingStateRequest {
        state_db_path: first.bootstrap.state_db_path.clone(),
        install_id: first.install_id.clone(),
    })
    .await
    .expect("S03 direct onboarding must expose ordered steps with one current step and one recommended next action");

    assert_eq!(guided.install_id, first.install_id);
    assert_eq!(guided.workspace_id, first.workspace_id);
    assert_eq!(
        guided.aggregate_status,
        OnboardingAggregateStatus::NeedsAction
    );
    assert_eq!(
        guided.ordered_steps.len(),
        2,
        "guided flow should show the setup requirement before the owner decision for this bundle"
    );
    assert_eq!(guided.ordered_steps[0].step_key, "POLYMARKET_API_KEY");
    assert_eq!(
        guided.ordered_steps[1].step_key,
        "approve-max-spread-budget"
    );
    assert_eq!(
        guided.current_step_key.as_deref(),
        Some("POLYMARKET_API_KEY")
    );
    assert_eq!(
        guided.recommended_action.as_ref().map(|action| action.kind),
        Some(GuidedOnboardingActionKind::CompleteStep),
        "guided flow must expose one recommended action for the current step rather than leaving the owner to infer the next move"
    );
    assert_eq!(
        guided
            .recommended_action
            .as_ref()
            .and_then(|action| action.step_key.as_deref()),
        Some("POLYMARKET_API_KEY")
    );

    let action = apply_guided_onboarding_action(GuidedOnboardingActionRequest {
        state_db_path: first.bootstrap.state_db_path.clone(),
        install_id: first.install_id.clone(),
        action: GuidedOnboardingAction::CompleteStep {
            step_key: "POLYMARKET_API_KEY".to_owned(),
        },
    })
    .await
    .expect("S03 direct onboarding must let supported actions mutate canonical install state instead of requiring manual SQLite edits");

    assert_eq!(action.install_id, first.install_id);
    assert_eq!(
        action.aggregate_status,
        OnboardingAggregateStatus::NeedsAction
    );
    assert_eq!(
        action.current_step_key.as_deref(),
        Some("approve-max-spread-budget")
    );
    assert_eq!(
        action
            .recommended_action
            .as_ref()
            .map(|candidate| candidate.kind),
        Some(GuidedOnboardingActionKind::ResolveOwnerDecision),
        "after the local setup step is completed, guided onboarding should advance to the owner decision as the next recommended action"
    );

    let reopened = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect("same install reopen should succeed after guided action mutation");

    assert_eq!(reopened.claim_disposition, ClaimDisposition::Reopened);
    assert_eq!(reopened.workspace_id, first.workspace_id);
    assert_eq!(reopened.install_id, first.install_id);

    let resumed = read_guided_onboarding(GuidedOnboardingStateRequest {
        state_db_path: reopened.bootstrap.state_db_path,
        install_id: reopened.install_id.clone(),
    })
    .await
    .expect("guided onboarding should reconnect to the same install on reopen and preserve current-step progress");

    assert_eq!(resumed.install_id, reopened.install_id);
    assert_eq!(
        resumed.current_step_key.as_deref(),
        Some("approve-max-spread-budget")
    );
    assert_eq!(
        resumed
            .recommended_action
            .as_ref()
            .map(|candidate| candidate.kind),
        Some(GuidedOnboardingActionKind::ResolveOwnerDecision)
    );
}

#[tokio::test]
async fn guided_flow_surfaces_drift_diagnostics_and_rejects_out_of_order_actions() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(GUIDED_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_WITH_SECRET_MD),
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
    .expect("bootstrap should claim the canonical install before drift coverage runs");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(GUIDED_ENTRY_SKILL_V2_MD),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_WITH_TWO_SECRETS_MD),
    );

    let reopened = bootstrap_install(InstallBootstrapRequest {
        install_url: entry_url,
        workspace_root: workspace_root.path().to_path_buf(),
        expected_workspace_id: Some(first.workspace_id.clone()),
        expected_install_id: Some(first.install_id.clone()),
    })
    .await
    .expect("same install reopen should refresh the canonical onboarding row before guided drift assertions");

    let guided = read_guided_onboarding(GuidedOnboardingStateRequest {
        state_db_path: reopened.bootstrap.state_db_path.clone(),
        install_id: reopened.install_id.clone(),
    })
    .await
    .expect("S03 direct onboarding must keep drifted installs inspectable through a guided state instead of hiding them behind raw checklist rows");

    assert_eq!(guided.aggregate_status, OnboardingAggregateStatus::Drifted);
    assert_eq!(guided.current_step_key.as_deref(), Some("bundle_drift"));
    assert_eq!(
        guided.recommended_action.as_ref().map(|action| action.kind),
        Some(GuidedOnboardingActionKind::AcknowledgeBundleDrift),
        "drifted installs should recommend bundle-drift acknowledgement before later steps can proceed"
    );
    let drift = guided
        .drift
        .as_ref()
        .expect("guided state should expose drift classification and changed-document evidence");
    assert_eq!(
        drift.classification,
        BundleLifecycleClassification::DocumentsChanged
    );
    assert!(
        drift
            .changed_documents
            .iter()
            .any(|change| change.document_id == "owner-setup"),
        "guided drift diagnostics should name the changed document so a future agent can localize the review"
    );

    let rejection = apply_guided_onboarding_action(GuidedOnboardingActionRequest {
        state_db_path: reopened.bootstrap.state_db_path,
        install_id: reopened.install_id,
        action: GuidedOnboardingAction::ResolveOwnerDecision {
            step_key: "approve-max-spread-budget".to_owned(),
            resolution: "approved".to_owned(),
        },
    })
    .await
    .expect_err("guided onboarding should reject out-of-order owner-decision actions while bundle drift review is still pending");

    assert!(
        matches!(
            rejection,
            GuidedOnboardingError::ActionRejected { ref code, .. }
                if code == "bundle_drift_review_required"
        ),
        "action rejection should stay typed and readable so blocked/drift failures remain inspectable without raw logs"
    );
}
