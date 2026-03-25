mod support;

use a2ex_skill_bundle::{
    BundleDocumentLifecycleChangeKind, BundleLifecycleClassification,
    BundleLifecycleDiagnosticCode, load_skill_bundle_from_url,
};
use reqwest::{Client, Url};
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

const ENTRY_SKILL_MD_METADATA_ONLY: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.13
compatible_daemon: ">=0.1.0"
name: Prediction Spread Arb Reloaded
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
async fn lifecycle_marks_reloading_the_same_bundle_without_drift_as_no_change() {
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
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let first = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("initial bundle load should succeed");
    let second = load_skill_bundle_from_url(&Client::new(), entry_url)
        .await
        .expect("reload bundle load should succeed");

    let lifecycle = second.lifecycle_change_from(Some(&first));

    assert_eq!(
        lifecycle.classification,
        BundleLifecycleClassification::NoChange
    );
    assert!(
        lifecycle.changed_documents.is_empty(),
        "no-change reload should not report changed documents: {:?}",
        lifecycle.changed_documents
    );
    assert!(
        lifecycle.diagnostics.is_empty(),
        "no-change reload should not report lifecycle diagnostics: {:?}",
        lifecycle.diagnostics
    );
}

#[tokio::test]
async fn lifecycle_distinguishes_metadata_only_reload_from_document_drift() {
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
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let first = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("initial bundle load should succeed");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_METADATA_ONLY),
    );

    let second = load_skill_bundle_from_url(&Client::new(), entry_url)
        .await
        .expect("metadata-only reload should still load");
    let lifecycle = second.lifecycle_change_from(Some(&first));

    assert_eq!(
        lifecycle.classification,
        BundleLifecycleClassification::MetadataChanged,
        "bundle version/name drift must stay separate from document edits"
    );
    assert!(
        lifecycle.changed_documents.is_empty(),
        "metadata-only reload must not invent document changes: {:?}",
        lifecycle.changed_documents
    );
}

#[tokio::test]
async fn lifecycle_reports_document_level_revision_and_content_changes() {
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
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let first = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("initial bundle load should succeed");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_DOCUMENT_CHANGE),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_MD_V2),
    );

    let second = load_skill_bundle_from_url(&Client::new(), entry_url)
        .await
        .expect("document-change reload should still load");
    let lifecycle = second.lifecycle_change_from(Some(&first));

    assert_eq!(
        lifecycle.classification,
        BundleLifecycleClassification::DocumentsChanged
    );
    assert!(lifecycle.changed_documents.iter().any(|change| {
        change.document_id == "owner-setup"
            && change.kind == BundleDocumentLifecycleChangeKind::RevisionChanged
            && change.previous_revision.as_deref() == Some("2026.03.10")
            && change.current_revision.as_deref() == Some("2026.03.11")
    }));
}

#[tokio::test]
async fn lifecycle_escalates_incompatible_daemon_and_manifest_revision_drift_to_blocking() {
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
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let first = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("initial bundle load should succeed");

    harness.set_fixture(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD_BLOCKING_DRIFT),
    );
    harness.set_fixture(
        "/bundles/prediction-spread-arb/docs/owner-setup.md",
        BundleFixture::markdown(OWNER_SETUP_MD_V1),
    );

    let second = load_skill_bundle_from_url(&Client::new(), entry_url)
        .await
        .expect("blocking drift should remain inspectable as a load outcome");
    let lifecycle = second.lifecycle_change_from(Some(&first));

    assert_eq!(
        lifecycle.classification,
        BundleLifecycleClassification::BlockingDrift
    );
    assert!(lifecycle.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleLifecycleDiagnosticCode::IncompatibleDaemon
    }));
    assert!(lifecycle.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleLifecycleDiagnosticCode::ManifestDocumentRevisionMismatch
            && diagnostic.document_id.as_deref() == Some("owner-setup")
    }));
}
