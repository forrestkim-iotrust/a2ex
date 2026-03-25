use a2ex_skill_bundle::{
    BundleDiagnostic, BundleDiagnosticCode, BundleDiagnosticPhase, BundleDiagnosticSeverity,
    BundleLoadOutcome, BundleResult, FetchedBundleDocument, ProposalQuantitativeCompleteness,
    ProposalReadiness, SkillBundleInterpretationStatus, generate_proposal_packet,
    interpret_bundle_load_outcome, interpret_skill_bundle, parse_skill_bundle_documents,
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

# Unknown Capital Envelope

The bundle does not specify the capital envelope, minimum bankroll, or per-venue limits.
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

#[test]
fn proposal_packet_contract_requires_truthful_unknown_quantitative_sections() {
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
    .expect("bundle contract should parse for proposal packet fixtures");
    let interpretation = interpret_skill_bundle(&parsed.bundle)
        .expect("interpretation fixture should compile for proposal packet contract tests");
    let outcome = BundleLoadOutcome {
        bundle: Some(parsed.bundle.clone()),
        diagnostics: parsed.diagnostics.clone(),
    };

    let packet = expect_proposal_packet(generate_proposal_packet(&outcome, &interpretation));

    assert_eq!(
        packet.interpretation_status,
        SkillBundleInterpretationStatus::Ambiguous,
        "proposal packet must preserve the underlying interpretation status"
    );
    assert_eq!(
        packet.proposal_readiness,
        ProposalReadiness::Incomplete,
        "ambiguous interpretation should remain inspectable but not present itself as ready"
    );

    let plan_summary = packet
        .plan_summary
        .as_ref()
        .expect("proposal packet should carry the interpreted plan summary");
    assert_eq!(plan_summary.bundle_id, "official.prediction-spread-arb");
    assert!(
        plan_summary
            .overview
            .as_deref()
            .is_some_and(|overview| overview.contains("explicit owner approval")),
        "proposal packet should preserve the owner-facing plan overview"
    );

    assert_eq!(packet.setup_requirements.len(), 2);
    assert_eq!(packet.owner_override_points.len(), 2);
    assert_eq!(packet.automation_boundaries.len(), 1);
    assert_eq!(packet.risk_framing.len(), 1);
    assert_eq!(packet.ambiguities.len(), 1);
    assert!(packet.blockers.is_empty());
    assert!(
        packet.provenance.iter().any(|evidence| {
            evidence.document_id == "skill"
                && evidence.section_slug.as_deref() == Some("overview")
                && evidence.source_url == entry_url
        }),
        "proposal packet provenance must remain inspectable by future agents"
    );

    assert_eq!(
        packet.capital_profile.completeness,
        ProposalQuantitativeCompleteness::Unknown,
        "capital profile must not guess missing bankroll or venue allocation data"
    );
    assert!(
        packet
            .capital_profile
            .reason
            .contains("bundle does not specify"),
        "capital profile should explain why it is unknown"
    );
    assert_eq!(
        packet.cost_profile.completeness,
        ProposalQuantitativeCompleteness::Unknown,
        "cost profile must stay explicitly unknown when the bundle contract lacks cost data"
    );
    assert!(
        packet
            .cost_profile
            .reason
            .contains("bundle does not specify"),
        "cost profile should explain the missing quantitative evidence"
    );
}

#[test]
fn proposal_packet_contract_preserves_blocked_state_and_quantitative_blockers() {
    let entry_url = Url::parse("https://bundles.a2ex.local/skills/prediction-spread-arb/skill.md")
        .expect("entry url parses");
    let outcome = BundleLoadOutcome {
        bundle: None,
        diagnostics: vec![BundleDiagnostic {
            code: BundleDiagnosticCode::MissingRequiredDocument,
            severity: BundleDiagnosticSeverity::Error,
            phase: BundleDiagnosticPhase::LoadManifest,
            message: "required owner setup document missing".to_owned(),
            document_id: Some("owner-setup".to_owned()),
            source_url: Some(
                entry_url
                    .join("docs/owner-setup.md")
                    .expect("blocked owner setup url resolves"),
            ),
            section_slug: None,
        }],
    };
    let interpretation = interpret_bundle_load_outcome(&outcome)
        .expect("blocked outcome should still produce an inspectable interpretation");

    let packet = expect_proposal_packet(generate_proposal_packet(&outcome, &interpretation));

    assert_eq!(
        packet.interpretation_status,
        SkillBundleInterpretationStatus::Blocked
    );
    assert_eq!(packet.proposal_readiness, ProposalReadiness::Blocked);
    assert!(packet.plan_summary.is_none());
    assert_eq!(packet.blockers.len(), 1);
    assert!(
        packet.blockers[0]
            .diagnostic_code
            .as_ref()
            .is_some_and(|code| *code == BundleDiagnosticCode::MissingRequiredDocument),
        "proposal packet should preserve typed blocker diagnostics"
    );
    assert_eq!(packet.ambiguities.len(), 0);

    assert_eq!(
        packet.capital_profile.completeness,
        ProposalQuantitativeCompleteness::Blocked,
        "blocked proposal packets should mark capital profile as blocked when setup truth is missing"
    );
    assert!(
        packet.capital_profile.reason.contains("owner setup"),
        "capital blocker reason should reference the missing setup truth"
    );
    assert_eq!(
        packet.cost_profile.completeness,
        ProposalQuantitativeCompleteness::Blocked,
        "blocked proposal packets should not pretend costs are knowable"
    );
    assert!(
        packet.cost_profile.reason.contains("owner setup"),
        "cost blocker reason should stay inspectable instead of hidden in tool failure text"
    );
    assert!(
        packet
            .provenance
            .iter()
            .any(|evidence| evidence.document_id == "owner-setup"),
        "proposal packet must preserve blocker provenance for later debugging"
    );
}

fn expect_proposal_packet<T>(result: BundleResult<T>) -> T {
    result.expect(
        "proposal packet builder should exist as the canonical S04 contract target for these tests",
    )
}
