mod support;

use a2ex_mcp::{
    A2exSkillMcpServer, LoadBundleRequest, ReadSessionResourceRequest, ReloadBundleRequest,
    SkillSessionLifecycleResource, SkillSessionResourceKind, SkillSessionStatusResource,
    stable_session_id,
};
use a2ex_skill_bundle::{
    BundleDocumentLifecycleChangeKind, BundleLifecycleClassification, BundleLifecycleDiagnosticCode,
};
use reqwest::Client;
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};

const ENTRY_SKILL_MD_V1: &str = r#"---
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

Track spread divergences and wait for explicit owner approval before acting.
"#;

const ENTRY_SKILL_MD_DOCUMENT_CHANGE: &str = r#"---
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

Track spread divergences and wait for explicit owner approval before acting.
"#;

const ENTRY_SKILL_MD_BLOCKING_DRIFT: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.14
compatible_daemon: ">=999.0.0"
name: Prediction Spread Arb
summary: Capture spread dislocations between prediction venues.
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
    revision: 2026.03.12
---
# Overview

Track spread divergences and wait for explicit owner approval before acting.
"#;

const OWNER_SETUP_MD_V1: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

const OWNER_SETUP_MD_V2: &str = r#"---
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
async fn reload_keeps_session_identity_but_exposes_document_change_lifecycle_state() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD_V1),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD_V1),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let server = A2exSkillMcpServer::new(Client::new());

    let first = server
        .load_bundle(LoadBundleRequest {
            entry_url: entry_url.clone(),
        })
        .await
        .expect("initial MCP load should succeed");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_DOCUMENT_CHANGE),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_MD_V2),
    );

    let reload = server
        .reload_bundle(ReloadBundleRequest {
            session_id: first.session_id.clone(),
            entry_url: entry_url.clone(),
        })
        .await
        .expect("reload should keep the stable MCP session id");

    assert_eq!(reload.session_id, first.session_id);

    let status: SkillSessionStatusResource = serde_json::from_value(
        server
            .read_resource(ReadSessionResourceRequest {
                session_id: first.session_id.clone(),
                resource: SkillSessionResourceKind::Status,
            })
            .await
            .expect("status resource should remain readable after reload"),
    )
    .expect("status payload should expose typed lifecycle summary fields");

    assert_eq!(status.session_id, first.session_id);
    assert_eq!(status.entry_url, entry_url);
    assert_eq!(status.revision, 2);
    assert_eq!(
        status.lifecycle.classification,
        BundleLifecycleClassification::DocumentsChanged
    );
    assert!(status.lifecycle.changed_documents.iter().any(|change| {
        change.document_id == "owner-setup"
            && change.kind == BundleDocumentLifecycleChangeKind::RevisionChanged
    }));

    let lifecycle: SkillSessionLifecycleResource = serde_json::from_value(
        server
            .read_resource(ReadSessionResourceRequest {
                session_id: first.session_id.clone(),
                resource: SkillSessionResourceKind::Lifecycle,
            })
            .await
            .expect("lifecycle resource should be readable once MCP exposes lifecycle truth"),
    )
    .expect("lifecycle payload should deserialize into the explicit MCP contract");

    assert_eq!(lifecycle.session_id, first.session_id);
    assert_eq!(lifecycle.entry_url, entry_url);
    assert_eq!(lifecycle.revision, 2);
    assert_eq!(
        lifecycle.lifecycle.classification,
        BundleLifecycleClassification::DocumentsChanged
    );
    assert!(lifecycle.lifecycle.changed_documents.iter().any(|change| {
        change.document_id == "owner-setup"
            && change.kind == BundleDocumentLifecycleChangeKind::RevisionChanged
            && change.previous_revision.as_deref() == Some("2026.03.10")
            && change.current_revision.as_deref() == Some("2026.03.11")
    }));
}

#[tokio::test]
async fn lifecycle_resource_surfaces_blocking_reload_diagnostics_without_changing_session_identity()
{
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD_V1),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD_V1),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let server = A2exSkillMcpServer::new(Client::new());

    let first = server
        .load_bundle(LoadBundleRequest {
            entry_url: entry_url.clone(),
        })
        .await
        .expect("initial MCP load should succeed");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_BLOCKING_DRIFT),
    );

    let reload = server
        .reload_bundle(ReloadBundleRequest {
            session_id: first.session_id.clone(),
            entry_url: entry_url.clone(),
        })
        .await
        .expect("blocking reload should stay resource-backed instead of crashing the session");

    assert_eq!(reload.session_id, first.session_id);

    let lifecycle: SkillSessionLifecycleResource = serde_json::from_value(
        server
            .read_resource(ReadSessionResourceRequest {
                session_id: first.session_id.clone(),
                resource: SkillSessionResourceKind::Lifecycle,
            })
            .await
            .expect("lifecycle resource should remain readable for blocking reloads"),
    )
    .expect("blocking lifecycle payload should deserialize into the explicit MCP contract");

    assert_eq!(lifecycle.session_id, first.session_id);
    assert_eq!(
        lifecycle.lifecycle.classification,
        BundleLifecycleClassification::BlockingDrift
    );
    assert!(lifecycle.lifecycle.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleLifecycleDiagnosticCode::IncompatibleDaemon
    }));
    assert!(lifecycle.lifecycle.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleLifecycleDiagnosticCode::ManifestDocumentRevisionMismatch
            && diagnostic.document_id.as_deref() == Some("owner-setup")
    }));

    let status: SkillSessionStatusResource = serde_json::from_value(
        server
            .read_resource(ReadSessionResourceRequest {
                session_id: first.session_id.clone(),
                resource: SkillSessionResourceKind::Status,
            })
            .await
            .expect("status resource should expose the same lifecycle classification summary"),
    )
    .expect("status payload should expose typed lifecycle summary fields");

    assert_eq!(status.session_id, first.session_id);
    assert_eq!(
        status.lifecycle.classification,
        BundleLifecycleClassification::BlockingDrift
    );
    assert_eq!(
        status.lifecycle.diagnostics,
        lifecycle.lifecycle.diagnostics
    );
}

#[tokio::test]
async fn mismatched_reload_rejects_before_registry_mutation_and_preserves_failure_diagnostics() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD_V1),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD_V1),
        ),
    ])
    .await;
    let entry_url = harness.url("/bundles/prediction-spread-arb/skill.md");
    let server = A2exSkillMcpServer::new(Client::new());

    let first = server
        .load_bundle(LoadBundleRequest {
            entry_url: entry_url.clone(),
        })
        .await
        .expect("initial MCP load should succeed");

    let mismatched_entry_url = format!("{entry_url}?identity-mismatch=1");
    let mismatch = server
        .reload_bundle(ReloadBundleRequest {
            session_id: first.session_id.clone(),
            entry_url: mismatched_entry_url.clone(),
        })
        .await;
    assert!(
        mismatch.is_err(),
        "reload with different URL text should reject instead of forking session identity"
    );

    let failures = server
        .read_resource(ReadSessionResourceRequest {
            session_id: first.session_id.clone(),
            resource: SkillSessionResourceKind::Failures,
        })
        .await
        .expect("failure diagnostics should remain readable on the existing session");
    assert_eq!(failures["session_id"], first.session_id);
    assert_eq!(
        failures["last_rejected_command"]["command"],
        "skills.reload_bundle"
    );
    assert_eq!(
        failures["last_rejected_command"]["rejection_code"],
        "session_identity_mismatch"
    );
    assert_eq!(
        failures["last_command_outcome"]["rejection_code"],
        "session_identity_mismatch"
    );

    let stray_session = server
        .read_resource(ReadSessionResourceRequest {
            session_id: stable_session_id(&mismatched_entry_url),
            resource: SkillSessionResourceKind::Status,
        })
        .await;
    assert!(
        stray_session.is_err(),
        "identity-mismatched reload should not create a second readable session root"
    );
}
