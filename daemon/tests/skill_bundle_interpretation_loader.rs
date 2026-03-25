mod support;

use a2ex_skill_bundle::{
    BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    SkillBundleInterpretationStatus, interpret_bundle_load_outcome, load_skill_bundle_from_url,
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
---
# Overview

Track spread divergences and wait for explicit owner approval before acting.

# Owner Decisions

- Approve max spread budget.
"#;

const ENTRY_WITH_OPTIONAL_NOTES_SKILL_MD: &str = r#"---
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
"#;

const OWNER_SETUP_MD: &str = r#"---
document_id: owner-setup
document_role: owner_setup
title: Owner Setup
revision: 2026.03.10
---
# Required Secrets

- POLYMARKET_API_KEY
"#;

#[tokio::test]
async fn interprets_loaded_bundle_using_same_public_contract() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_SKILL_MD),
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
        .expect("local bundle harness should be reachable once the loader exists");

    assert!(outcome.diagnostics.is_empty());
    assert!(outcome.bundle.is_some());

    let interpretation = interpret_bundle_load_outcome(&outcome)
        .expect("loaded-bundle interpretation entrypoint should compile before behavior exists");

    assert_eq!(
        interpretation.status,
        SkillBundleInterpretationStatus::NeedsSetup,
        "loaded bundle with required secrets should surface setup requirements rather than executable requests"
    );
    assert!(interpretation.blockers.is_empty());
    assert!(
        interpretation.plan_summary.is_some(),
        "successful load outcome should still expose typed plan summary"
    );
    assert!(
        interpretation.setup_requirements.iter().any(|requirement| {
            requirement.requirement_key == "POLYMARKET_API_KEY"
                && requirement.evidence.iter().any(|evidence| {
                    evidence.document_id == "owner-setup"
                        && evidence.section_id.as_deref() == Some("owner-setup#required-secrets")
                        && evidence.section_slug.as_deref() == Some("required-secrets")
                })
        }),
        "loaded interpretation must preserve setup requirement provenance"
    );
    assert!(
        interpretation.owner_decisions.iter().any(|decision| {
            decision.decision_key == "approve-max-spread-budget"
                && decision.decision_text.contains("Approve max spread budget")
        }),
        "loaded interpretation should expose typed owner decisions"
    );
    assert!(
        interpretation.provenance.iter().any(|evidence| {
            evidence.document_id == "skill"
                && evidence.section_slug.as_deref() == Some("overview")
                && evidence.source_url == entry_url
        }),
        "top-level provenance should remain inspectable after live loading"
    );
}

#[tokio::test]
async fn loader_diagnostics_become_interpretation_blockers() {
    let harness = spawn_skill_bundle([(
        "/bundles/prediction-spread-arb/skill.md",
        BundleFixture::markdown(ENTRY_SKILL_MD),
    )])
    .await;
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");
    let missing_url = entry_url
        .join("docs/owner-setup.md")
        .expect("missing owner setup url resolves");

    let outcome = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("missing required document should surface diagnostics in the API shape");

    assert!(outcome.bundle.is_none());
    assert!(outcome.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleDiagnosticCode::MissingRequiredDocument
            && diagnostic.severity == BundleDiagnosticSeverity::Error
            && diagnostic.phase == BundleDiagnosticPhase::LoadManifest
            && diagnostic.document_id.as_deref() == Some("owner-setup")
            && diagnostic.source_url.as_ref() == Some(&missing_url)
    }));

    let interpretation = interpret_bundle_load_outcome(&outcome).expect(
        "blocked loaded-bundle interpretation entrypoint should compile before behavior exists",
    );

    assert_eq!(
        interpretation.status,
        SkillBundleInterpretationStatus::Blocked
    );
    assert!(interpretation.plan_summary.is_none());
    assert!(interpretation.owner_decisions.is_empty());
    assert!(interpretation.setup_requirements.is_empty());
    assert!(interpretation.blockers.iter().any(|blocker| {
        blocker.blocker_key == "owner-setup:missing_required_document"
            && blocker.summary.contains("owner-setup")
            && blocker.diagnostic_code == Some(BundleDiagnosticCode::MissingRequiredDocument)
            && blocker.diagnostic_severity == Some(BundleDiagnosticSeverity::Error)
            && blocker.diagnostic_phase == Some(BundleDiagnosticPhase::LoadManifest)
            && blocker.evidence.iter().any(|evidence| {
                evidence.document_id == "owner-setup"
                    && evidence.section_id.is_none()
                    && evidence.section_slug.is_none()
                    && evidence.source_url == missing_url
            })
    }));
}

#[tokio::test]
async fn partial_live_bundle_with_loader_diagnostic_stays_blocked_and_inspectable() {
    let harness = spawn_skill_bundle([
        (
            "/bundles/prediction-spread-arb/skill.md",
            BundleFixture::markdown(ENTRY_WITH_OPTIONAL_NOTES_SKILL_MD),
        ),
        (
            "/bundles/prediction-spread-arb/docs/owner-setup.md",
            BundleFixture::markdown(OWNER_SETUP_MD),
        ),
    ])
    .await;
    let entry_url = Url::parse(&harness.url("/bundles/prediction-spread-arb/skill.md"))
        .expect("entry url parses");
    let missing_url = entry_url
        .join("docs/operator-notes.md")
        .expect("missing operator notes url resolves");

    let outcome = load_skill_bundle_from_url(&Client::new(), entry_url.clone())
        .await
        .expect("optional missing document should remain in the load outcome diagnostics");

    assert!(outcome.bundle.is_some());
    assert!(outcome.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == BundleDiagnosticCode::FetchFailed
            && diagnostic.severity == BundleDiagnosticSeverity::Warning
            && diagnostic.phase == BundleDiagnosticPhase::FetchDocument
            && diagnostic.document_id.as_deref() == Some("operator-notes")
            && diagnostic.source_url.as_ref() == Some(&missing_url)
    }));

    let interpretation = interpret_bundle_load_outcome(&outcome)
        .expect("partial loaded bundle should still produce an inspectable interpretation");

    assert_eq!(
        interpretation.status,
        SkillBundleInterpretationStatus::Blocked,
        "loader diagnostics must outrank otherwise actionable bundle content"
    );
    assert!(
        interpretation.plan_summary.is_some(),
        "bundle-backed blocked interpretations should remain partially inspectable"
    );
    assert!(interpretation.owner_decisions.iter().any(|decision| {
        decision.decision_key == "approve-max-spread-budget"
            && decision.decision_text.contains("Approve max spread budget")
    }));
    assert!(
        interpretation
            .setup_requirements
            .iter()
            .any(|requirement| { requirement.requirement_key == "POLYMARKET_API_KEY" })
    );
    assert!(interpretation.blockers.iter().any(|blocker| {
        blocker.blocker_key == "operator-notes:fetch_failed"
            && blocker.summary.contains("operator-notes")
            && blocker.diagnostic_code == Some(BundleDiagnosticCode::FetchFailed)
            && blocker.diagnostic_severity == Some(BundleDiagnosticSeverity::Warning)
            && blocker.diagnostic_phase == Some(BundleDiagnosticPhase::FetchDocument)
            && blocker.evidence.iter().any(|evidence| {
                evidence.document_id == "operator-notes"
                    && evidence.section_id.is_none()
                    && evidence.section_slug.is_none()
                    && evidence.source_url == missing_url
            })
    }));
    assert!(interpretation.provenance.iter().any(|evidence| {
        evidence.document_id == "skill"
            && evidence.section_slug.as_deref() == Some("overview")
            && evidence.source_url == entry_url
    }));
    assert!(interpretation.provenance.iter().any(|evidence| {
        evidence.document_id == "operator-notes"
            && evidence.section_id.is_none()
            && evidence.section_slug.is_none()
            && evidence.source_url == missing_url
    }));
}
