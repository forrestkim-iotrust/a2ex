use a2ex_skill_bundle::{BundleDocumentRole, FetchedBundleDocument, parse_skill_bundle_documents};
use reqwest::Url;

const SKILL_MD: &str = r#"---
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

# Owner Decisions

- Approve max spread budget.
- Choose target venues.

# Unknown Policy Surface

This heading is not interpreted in S01 and must survive as structured unresolved content.
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

#[test]
fn parses_official_bundle_contract_and_preserves_unresolved_sections() {
    let entry_url = Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
        .expect("entry url parses");
    let owner_setup_url = entry_url
        .join("docs/owner-setup.md")
        .expect("owner setup url resolves");
    let operator_notes_url = entry_url
        .join("docs/operator-notes.md")
        .expect("operator notes url resolves");

    let parsed = parse_skill_bundle_documents(vec![
        FetchedBundleDocument {
            document_id: "skill".to_owned(),
            source_url: entry_url.clone(),
            body_markdown: SKILL_MD.to_owned(),
        },
        FetchedBundleDocument {
            document_id: "owner-setup".to_owned(),
            source_url: owner_setup_url.clone(),
            body_markdown: OWNER_SETUP_MD.to_owned(),
        },
        FetchedBundleDocument {
            document_id: "operator-notes".to_owned(),
            source_url: operator_notes_url.clone(),
            body_markdown: OPERATOR_NOTES_MD.to_owned(),
        },
    ])
    .expect("bundle contract should parse once the bundle crate exists");

    assert!(
        parsed.diagnostics.is_empty(),
        "valid bundle fixture should not emit diagnostics: {:?}",
        parsed.diagnostics
    );

    let bundle = parsed.bundle;
    assert_eq!(bundle.bundle_id, "official.prediction-spread-arb");
    assert_eq!(bundle.bundle_format, "a2ex.skill-bundle/v1alpha1");
    assert_eq!(bundle.bundle_version, "2026.03.12");
    assert_eq!(bundle.compatible_daemon.as_deref(), Some(">=0.1.0"));
    assert_eq!(bundle.entry_document_id, "skill");
    assert_eq!(bundle.documents.len(), 3);

    let manifest = &bundle.document_manifest;
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest[0].document_id, "owner-setup");
    assert_eq!(manifest[0].role, BundleDocumentRole::OwnerSetup);
    assert!(manifest[0].required);
    assert_eq!(manifest[0].relative_path, "docs/owner-setup.md");
    assert_eq!(manifest[0].revision.as_deref(), Some("2026.03.10"));
    assert_eq!(manifest[1].document_id, "operator-notes");
    assert_eq!(manifest[1].role, BundleDocumentRole::OperatorNotes);
    assert!(!manifest[1].required);
    assert_eq!(manifest[1].revision.as_deref(), Some("2026.03.09"));

    let owner_setup = bundle
        .documents
        .iter()
        .find(|document| document.document_id == "owner-setup")
        .expect("owner setup document exists");
    assert_eq!(owner_setup.role, BundleDocumentRole::OwnerSetup);
    assert_eq!(owner_setup.source_url, owner_setup_url);
    assert_eq!(owner_setup.revision.as_deref(), Some("2026.03.10"));

    let operator_unresolved = bundle
        .unresolved_sections
        .iter()
        .find(|section| section.document_id == "operator-notes")
        .expect("unrecognized operator heading should survive as an unresolved section");
    assert_eq!(operator_unresolved.section_heading, "Escalation Paths");
    assert_eq!(operator_unresolved.section_slug, "escalation-paths");
    assert_eq!(operator_unresolved.source_url, operator_notes_url);
    assert!(operator_unresolved.markdown.contains("approval mismatch"));

    let entry_unresolved = bundle
        .unresolved_sections
        .iter()
        .find(|section| section.document_id == "skill")
        .expect("unknown entry heading should survive as an unresolved section");
    assert_eq!(entry_unresolved.section_heading, "Unknown Policy Surface");
    assert_eq!(entry_unresolved.source_url, entry_url);
}
