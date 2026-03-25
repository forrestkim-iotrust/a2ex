use a2ex_skill_bundle::{
    BundleDocumentRole, FetchedBundleDocument, InterpretationSetupRequirementKind,
    SkillBundleInterpretationStatus, interpret_skill_bundle, parse_skill_bundle_documents,
};
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
# Automation Boundaries

Wait for owner approval before any venue interaction or signer flow.

# Risk Notes

Venue prices may move while approval is pending.
"#;

const OWNER_DECISION_ONLY_SKILL_MD: &str = r#"---
bundle_id: official.owner-decision-only
bundle_format: a2ex.skill-bundle/v1alpha1
bundle_version: 2026.03.12
name: Owner Decision Only
summary: Requires a venue choice before proposal details are complete.
---
# Overview

Wait for the owner to choose the target venue before acting.

# Owner Decisions

- Choose target venue.
"#;

#[test]
fn interprets_parsed_bundle_into_typed_contract_with_provenance() {
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

    assert!(parsed.diagnostics.is_empty());
    assert_eq!(parsed.bundle.documents.len(), 3);
    assert_eq!(parsed.bundle.document_manifest.len(), 2);
    assert_eq!(
        parsed.bundle.document_manifest[0].role,
        BundleDocumentRole::OwnerSetup
    );
    assert_eq!(
        parsed.bundle.document_manifest[1].role,
        BundleDocumentRole::OperatorNotes
    );

    let interpretation = interpret_skill_bundle(&parsed.bundle)
        .expect("interpretation contract should compile before behavior exists");

    assert_eq!(
        interpretation.status,
        SkillBundleInterpretationStatus::Ambiguous
    );

    let plan_summary = interpretation
        .plan_summary
        .as_ref()
        .expect("plan summary should be present for a parsed valid bundle");
    assert_eq!(plan_summary.bundle_id, "official.prediction-spread-arb");
    assert_eq!(plan_summary.title.as_deref(), Some("Prediction Spread Arb"));
    assert_eq!(
        plan_summary.summary.as_deref(),
        Some("Capture spread dislocations between prediction venues.")
    );
    assert!(
        plan_summary
            .overview
            .as_deref()
            .is_some_and(|overview| overview.contains("explicit owner approval")),
        "overview should preserve the interpreted high-level plan"
    );
    assert!(
        plan_summary.evidence.iter().any(|evidence| {
            evidence.document_id == "skill"
                && evidence.section_id.as_deref() == Some("skill#overview")
                && evidence.section_slug.as_deref() == Some("overview")
                && evidence.source_url == entry_url
        }),
        "plan summary must retain entry section provenance"
    );

    assert_eq!(interpretation.owner_decisions.len(), 2);
    assert!(interpretation.owner_decisions.iter().any(|decision| {
        decision.decision_key == "approve-max-spread-budget"
            && decision.decision_text.contains("Approve max spread budget")
            && decision.evidence.iter().any(|evidence| {
                evidence.document_id == "skill"
                    && evidence.section_id.as_deref() == Some("skill#owner-decisions")
                    && evidence.section_slug.as_deref() == Some("owner-decisions")
                    && evidence.source_url == entry_url
            })
    }));
    assert!(interpretation.owner_decisions.iter().any(|decision| {
        decision.decision_key == "choose-target-venues"
            && decision.decision_text.contains("Choose target venues")
    }));

    assert_eq!(interpretation.setup_requirements.len(), 2);
    assert!(interpretation.setup_requirements.iter().any(|requirement| {
        requirement.requirement_key == "POLYMARKET_API_KEY"
            && requirement.requirement_kind == InterpretationSetupRequirementKind::Secret
            && requirement.evidence.iter().any(|evidence| {
                evidence.document_id == "owner-setup"
                    && evidence.section_id.as_deref() == Some("owner-setup#required-secrets")
                    && evidence.section_slug.as_deref() == Some("required-secrets")
                    && evidence.source_url == owner_setup_url
            })
    }));
    assert!(interpretation.setup_requirements.iter().any(|requirement| {
        requirement.requirement_key == "KALSHI_API_KEY"
            && requirement.requirement_kind == InterpretationSetupRequirementKind::Secret
    }));

    assert!(interpretation.automation_boundaries.iter().any(|boundary| {
        boundary.boundary_key == "owner-approval-before-venue-interaction"
            && boundary.summary.contains("owner approval")
            && boundary.evidence.iter().any(|evidence| {
                evidence.document_id == "operator-notes"
                    && evidence.section_id.as_deref()
                        == Some("operator-notes#automation-boundaries")
                    && evidence.section_slug.as_deref() == Some("automation-boundaries")
                    && evidence.source_url == operator_notes_url
            })
    }));

    assert!(interpretation.risks.iter().any(|risk| {
        risk.risk_key == "approval-window-price-movement"
            && risk.summary.contains("prices may move")
            && risk.evidence.iter().any(|evidence| {
                evidence.document_id == "operator-notes"
                    && evidence.section_id.as_deref() == Some("operator-notes#risk-notes")
                    && evidence.section_slug.as_deref() == Some("risk-notes")
                    && evidence.source_url == operator_notes_url
            })
    }));

    assert!(interpretation.ambiguities.iter().any(|ambiguity| {
        ambiguity.ambiguity_key == "skill#unknown-policy-surface"
            && ambiguity.summary.contains("Unknown Policy Surface")
            && ambiguity.evidence.iter().any(|evidence| {
                evidence.document_id == "skill"
                    && evidence.section_id.as_deref() == Some("skill#unknown-policy-surface")
                    && evidence.section_slug.as_deref() == Some("unknown-policy-surface")
                    && evidence.source_url == entry_url
            })
    }));

    assert!(interpretation.blockers.is_empty());
    assert!(
        interpretation.provenance.iter().any(|evidence| {
            evidence.document_id == "skill"
                && evidence.section_slug.as_deref() == Some("overview")
                && evidence.source_url == entry_url
        }),
        "top-level provenance should expose stable evidence surfaces for future inspectors"
    );
}

#[test]
fn interprets_owner_decision_only_bundle_as_needing_owner_decision() {
    let entry_url = Url::parse("https://bundles.a2ex.local/skills/owner-decision-only/skill.md")
        .expect("entry url parses");
    let parsed = parse_skill_bundle_documents(vec![FetchedBundleDocument {
        document_id: "skill".to_owned(),
        source_url: entry_url.clone(),
        body_markdown: OWNER_DECISION_ONLY_SKILL_MD.to_owned(),
    }])
    .expect("bundle contract should parse for owner-decision fixture");

    let interpretation = interpret_skill_bundle(&parsed.bundle)
        .expect("owner-decision fixture should compile into interpretation");

    assert_eq!(
        interpretation.status,
        SkillBundleInterpretationStatus::NeedsOwnerDecision
    );
    assert_eq!(interpretation.owner_decisions.len(), 1);
    assert!(interpretation.setup_requirements.is_empty());
    assert!(interpretation.ambiguities.is_empty());
}
