mod support;

use a2ex_skill_bundle::{
    BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity, BundleDocumentRole,
    load_skill_bundle_from_url,
};
use reqwest::{Client, Url};
use support::skill_bundle_harness::{BundleFixture, spawn_skill_bundle};

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
  - id: operator-notes
    role: operator_notes
    path: docs/operator-notes.md
    required: false
    revision: 2026.03.09
---
# Overview

Track spread divergences and wait for explicit owner approval before acting.
"#;

const OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
- KALSHI_API_KEY
"#;

const OPERATOR_NOTES_MD: &str = r#"---
document_id: operator-notes
document_role: operator_notes
title: Operator Notes
revision: 2026.03.09
---
# Escalation Paths

Escalate any approval mismatch to the owner before resuming the bundle.
"#;

const MALFORMED_ENTRY_SKILL_MD: &str = r#"---
bundle_id: official.prediction-spread-arb
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: [2026.03.12
compatible_daemon: ">=0.1.0"
documents:
  - id: owner-setup
    role: owner_setup
    path: docs/owner-setup.md
    required: true
---
# Overview

Broken frontmatter should return a typed diagnostic.
"#;

#[tokio::test]
async fn loads_bundle_from_entry_url_and_resolves_relative_documents() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/operator-notes.md",
            BundleFixture::markdown(OPERATOR_NOTES_MD),
        ),
    ])
    .await;
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let outcome = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("local bundle harness should be reachable once the loader exists");

    assert!(
        outcome.diagnostics.is_empty(),
        "valid bundle fixture should not emit diagnostics: {:?}",
        outcome.diagnostics
    );

    let bundle = outcome
        .bundle
        .expect("valid official bundle should produce a structured bundle");
    assert_eq!(bundle.entry_document_id, "skill");
    assert_eq!(bundle.documents.len(), 3);
    assert_eq!(bundle.document_manifest.len(), 2);
    assert_eq!(
        bundle.document_manifest[0].role,
        BundleDocumentRole::OwnerSetup
    );
    assert_eq!(
        bundle.document_manifest[1].role,
        BundleDocumentRole::OperatorNotes
    );
    assert_eq!(bundle.documents[0].source_url, entry_url);
    assert_eq!(
        bundle.documents[1].source_url.as_str(),
        harness.url("/bundles/prediction-spread-arb/docs/owner-setup.md")
    );
    assert_eq!(
        bundle.documents[2].source_url.as_str(),
        harness.url("/bundles/prediction-spread-arb/docs/operator-notes.md")
    );
}

#[tokio::test]
async fn missing_required_document_emits_stable_typed_diagnostic() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/operator-notes.md",
            BundleFixture::markdown(OPERATOR_NOTES_MD),
        ),
    ])
    .await;
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");
    let missing_url = entry_url
        .join("docs/owner-setup.md")
        .expect("missing owner setup url resolves");

    let outcome = load_skill_bundle_from_url(&Client::new(), entry_url)
        .await
        .expect(
            "missing required document should surface as typed diagnostics, not transport failure",
        );

    assert!(
        outcome.bundle.is_none(),
        "required document failure should block bundle materialization"
    );
    let diagnostic = outcome
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == BundleDiagnosticCode::MissingRequiredDocument)
        .expect("missing required doc diagnostic should exist");
    assert_eq!(diagnostic.severity, BundleDiagnosticSeverity::Error);
    assert_eq!(diagnostic.phase, BundleDiagnosticPhase::LoadManifest);
    assert_eq!(diagnostic.document_id.as_deref(), Some("owner-setup"));
    assert_eq!(diagnostic.source_url.as_ref(), Some(&missing_url));
}

#[tokio::test]
async fn malformed_entry_frontmatter_emits_structured_parse_diagnostic() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(MALFORMED_ENTRY_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD),
        ),
    ])
    .await;
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");

    let outcome = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("malformed frontmatter should return diagnostics in the API shape");

    assert!(
        outcome.bundle.is_none(),
        "malformed entry frontmatter should block bundle materialization"
    );
    let diagnostic = outcome
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == BundleDiagnosticCode::MalformedFrontmatter)
        .expect("malformed frontmatter diagnostic should exist");
    assert_eq!(diagnostic.severity, BundleDiagnosticSeverity::Error);
    assert_eq!(diagnostic.phase, BundleDiagnosticPhase::ParseDocument);
    assert_eq!(diagnostic.document_id.as_deref(), Some("skill"));
    assert_eq!(diagnostic.source_url.as_ref(), Some(&entry_url));
}
