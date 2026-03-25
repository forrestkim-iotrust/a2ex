use std::collections::BTreeSet;

use crate::BundleResult;
use crate::model::{
    BundleDiagnostic, BundleLoadOutcome, BundleSection, BundleSectionKind, InterpretationAmbiguity,
    InterpretationAutomationBoundary, InterpretationBlocker, InterpretationEvidence,
    InterpretationOwnerDecision, InterpretationPlanSummary, InterpretationRisk,
    InterpretationSetupRequirement, InterpretationSetupRequirementKind, SkillBundle,
    SkillBundleInterpretation, SkillBundleInterpretationStatus, UnresolvedBundleSection,
};

pub fn interpret_skill_bundle(bundle: &SkillBundle) -> BundleResult<SkillBundleInterpretation> {
    let plan_summary = interpret_plan_summary(bundle);
    let owner_decisions = interpret_owner_decisions(bundle);
    let setup_requirements = interpret_setup_requirements(bundle);
    let automation_boundaries = interpret_automation_boundaries(bundle);
    let risks = interpret_risks(bundle);
    let ambiguities = interpret_ambiguities(bundle);
    let blockers = Vec::new();

    let provenance = collect_provenance(
        plan_summary
            .as_ref()
            .map(|summary| summary.evidence.as_slice()),
        &owner_decisions
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &setup_requirements
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &automation_boundaries
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &risks
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &ambiguities
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &[],
    );

    Ok(SkillBundleInterpretation {
        status: derive_status(
            &blockers,
            &ambiguities,
            &setup_requirements,
            &owner_decisions,
        ),
        plan_summary,
        owner_decisions,
        setup_requirements,
        automation_boundaries,
        risks,
        ambiguities,
        blockers,
        provenance,
    })
}

pub fn interpret_bundle_load_outcome(
    outcome: &BundleLoadOutcome,
) -> BundleResult<SkillBundleInterpretation> {
    let blockers: Vec<_> = outcome.diagnostics.iter().map(interpret_blocker).collect();

    if let Some(bundle) = &outcome.bundle {
        let interpretation = interpret_skill_bundle(bundle)?;
        return Ok(merge_interpretation_blockers(interpretation, blockers));
    }

    let provenance = collect_provenance(
        None,
        &[],
        &[],
        &[],
        &[],
        &[],
        &blockers
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
    );

    Ok(SkillBundleInterpretation {
        status: derive_status(&blockers, &[], &[], &[]),
        plan_summary: None,
        owner_decisions: Vec::new(),
        setup_requirements: Vec::new(),
        automation_boundaries: Vec::new(),
        risks: Vec::new(),
        ambiguities: Vec::new(),
        blockers,
        provenance,
    })
}

fn merge_interpretation_blockers(
    mut interpretation: SkillBundleInterpretation,
    blockers: Vec<InterpretationBlocker>,
) -> SkillBundleInterpretation {
    if blockers.is_empty() {
        return interpretation;
    }

    interpretation.blockers.extend(blockers);
    interpretation.status = derive_status(
        &interpretation.blockers,
        &interpretation.ambiguities,
        &interpretation.setup_requirements,
        &interpretation.owner_decisions,
    );
    interpretation.provenance = collect_provenance(
        interpretation
            .plan_summary
            .as_ref()
            .map(|summary| summary.evidence.as_slice()),
        &interpretation
            .owner_decisions
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &interpretation
            .setup_requirements
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &interpretation
            .automation_boundaries
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &interpretation
            .risks
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &interpretation
            .ambiguities
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
        &interpretation
            .blockers
            .iter()
            .flat_map(|item| item.evidence.iter().cloned())
            .collect::<Vec<_>>(),
    );
    interpretation
}

fn interpret_plan_summary(bundle: &SkillBundle) -> Option<InterpretationPlanSummary> {
    let overview_section = bundle
        .documents
        .iter()
        .flat_map(|document| document.sections.iter())
        .find(|section| matches!(section.kind, BundleSectionKind::Overview));

    let evidence = overview_section
        .map(section_evidence)
        .into_iter()
        .collect::<Vec<_>>();

    Some(InterpretationPlanSummary {
        bundle_id: bundle.bundle_id.clone(),
        title: bundle.name.clone(),
        summary: bundle.summary.clone(),
        overview: overview_section.map(|section| clean_summary_text(&section.markdown)),
        evidence,
    })
}

fn interpret_owner_decisions(bundle: &SkillBundle) -> Vec<InterpretationOwnerDecision> {
    bundle
        .documents
        .iter()
        .flat_map(|document| document.sections.iter())
        .filter(|section| matches!(section.kind, BundleSectionKind::OwnerDecisions))
        .flat_map(|section| {
            let evidence = vec![section_evidence(section)];
            parse_bullet_items(&section.markdown)
                .into_iter()
                .map(move |decision_text| InterpretationOwnerDecision {
                    decision_key: slugify(&decision_text),
                    decision_text,
                    evidence: evidence.clone(),
                })
        })
        .collect()
}

fn interpret_setup_requirements(bundle: &SkillBundle) -> Vec<InterpretationSetupRequirement> {
    bundle
        .documents
        .iter()
        .flat_map(|document| document.sections.iter())
        .filter(|section| matches!(section.kind, BundleSectionKind::RequiredSecrets))
        .flat_map(|section| {
            let evidence = vec![section_evidence(section)];
            parse_bullet_items(&section.markdown)
                .into_iter()
                .map(move |requirement_key| InterpretationSetupRequirement {
                    requirement_key: normalize_secret_name(&requirement_key),
                    requirement_kind: InterpretationSetupRequirementKind::Secret,
                    summary: None,
                    evidence: evidence.clone(),
                })
        })
        .collect()
}

fn interpret_automation_boundaries(bundle: &SkillBundle) -> Vec<InterpretationAutomationBoundary> {
    bundle
        .unresolved_sections
        .iter()
        .filter(|section| section.section_slug == "automation-boundaries")
        .map(|section| InterpretationAutomationBoundary {
            boundary_key: derive_boundary_key(section),
            summary: clean_summary_text(&section.markdown),
            evidence: vec![unresolved_section_evidence(section)],
        })
        .collect()
}

fn interpret_risks(bundle: &SkillBundle) -> Vec<InterpretationRisk> {
    bundle
        .unresolved_sections
        .iter()
        .filter(|section| section.section_slug == "risk-notes")
        .map(|section| InterpretationRisk {
            risk_key: derive_risk_key(section),
            summary: clean_summary_text(&section.markdown),
            evidence: vec![unresolved_section_evidence(section)],
        })
        .collect()
}

fn interpret_ambiguities(bundle: &SkillBundle) -> Vec<InterpretationAmbiguity> {
    bundle
        .unresolved_sections
        .iter()
        .filter(|section| section.document_id == bundle.entry_document_id)
        .filter(|section| {
            !matches!(
                section.section_slug.as_str(),
                "automation-boundaries" | "risk-notes" | "preamble"
            )
        })
        .map(|section| InterpretationAmbiguity {
            ambiguity_key: section.section_id.clone(),
            summary: format!(
                "Uninterpreted section '{}' remains ambiguous: {}",
                section.section_heading,
                clean_summary_text(&section.markdown)
            ),
            evidence: vec![unresolved_section_evidence(section)],
        })
        .collect()
}

fn interpret_blocker(diagnostic: &BundleDiagnostic) -> InterpretationBlocker {
    let document_id = diagnostic
        .document_id
        .clone()
        .unwrap_or_else(|| "bundle".to_owned());
    let blocker_key = format!("{}:{}", document_id, diagnostic_code_slug(diagnostic));
    InterpretationBlocker {
        blocker_key,
        summary: diagnostic.message.clone(),
        diagnostic_code: Some(diagnostic.code.clone()),
        diagnostic_severity: Some(diagnostic.severity.clone()),
        diagnostic_phase: Some(diagnostic.phase.clone()),
        evidence: diagnostic
            .source_url
            .clone()
            .map(|source_url| InterpretationEvidence {
                document_id,
                section_id: None,
                section_slug: diagnostic.section_slug.clone(),
                source_url,
            })
            .into_iter()
            .collect(),
    }
}

fn derive_status(
    blockers: &[InterpretationBlocker],
    ambiguities: &[InterpretationAmbiguity],
    setup_requirements: &[InterpretationSetupRequirement],
    owner_decisions: &[InterpretationOwnerDecision],
) -> SkillBundleInterpretationStatus {
    if !blockers.is_empty() {
        SkillBundleInterpretationStatus::Blocked
    } else if !ambiguities.is_empty() {
        SkillBundleInterpretationStatus::Ambiguous
    } else if !setup_requirements.is_empty() {
        SkillBundleInterpretationStatus::NeedsSetup
    } else if !owner_decisions.is_empty() {
        SkillBundleInterpretationStatus::NeedsOwnerDecision
    } else {
        SkillBundleInterpretationStatus::InterpretedReady
    }
}

fn section_evidence(section: &BundleSection) -> InterpretationEvidence {
    InterpretationEvidence {
        document_id: section.document_id.clone(),
        section_id: Some(section.section_id.clone()),
        section_slug: Some(section.section_slug.clone()),
        source_url: section.source_url.clone(),
    }
}

fn unresolved_section_evidence(section: &UnresolvedBundleSection) -> InterpretationEvidence {
    InterpretationEvidence {
        document_id: section.document_id.clone(),
        section_id: Some(section.section_id.clone()),
        section_slug: Some(section.section_slug.clone()),
        source_url: section.source_url.clone(),
    }
}

fn collect_provenance(
    plan_summary: Option<&[InterpretationEvidence]>,
    owner_decisions: &[InterpretationEvidence],
    setup_requirements: &[InterpretationEvidence],
    automation_boundaries: &[InterpretationEvidence],
    risks: &[InterpretationEvidence],
    ambiguities: &[InterpretationEvidence],
    blockers: &[InterpretationEvidence],
) -> Vec<InterpretationEvidence> {
    let mut seen = BTreeSet::new();
    let mut collected = Vec::new();

    for evidence in plan_summary
        .into_iter()
        .flatten()
        .chain(owner_decisions.iter())
        .chain(setup_requirements.iter())
        .chain(automation_boundaries.iter())
        .chain(risks.iter())
        .chain(ambiguities.iter())
        .chain(blockers.iter())
    {
        if seen.insert(evidence.clone()) {
            collected.push(evidence.clone());
        }
    }

    collected
}

fn parse_bullet_items(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .map(clean_bullet_text)
        })
        .filter(|item| !item.is_empty())
        .collect()
}

fn clean_bullet_text(item: &str) -> String {
    item.trim()
        .trim_matches('`')
        .trim_end_matches('.')
        .trim()
        .to_owned()
}

fn clean_summary_text(markdown: &str) -> String {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_secret_name(value: &str) -> String {
    value.trim().trim_matches('`').to_owned()
}

fn derive_boundary_key(section: &UnresolvedBundleSection) -> String {
    let summary = clean_summary_text(&section.markdown);
    if summary.contains("owner approval") && summary.contains("venue interaction") {
        "owner-approval-before-venue-interaction".to_owned()
    } else {
        slugify(&format!("{} {}", section.section_heading, summary))
    }
}

fn derive_risk_key(section: &UnresolvedBundleSection) -> String {
    let summary = clean_summary_text(&section.markdown);
    if summary.contains("prices may move") && summary.contains("approval") {
        "approval-window-price-movement".to_owned()
    } else {
        slugify(&format!("{} {}", section.section_heading, summary))
    }
}

fn diagnostic_code_slug(diagnostic: &BundleDiagnostic) -> &'static str {
    use crate::model::BundleDiagnosticCode::*;

    match diagnostic.code {
        MalformedFrontmatter => "malformed_frontmatter",
        MissingRequiredMetadata => "missing_required_metadata",
        MissingRequiredDocument => "missing_required_document",
        DuplicateDocumentId => "duplicate_document_id",
        DuplicateDocumentRole => "duplicate_document_role",
        FetchFailed => "fetch_failed",
        InvalidDocumentReference => "invalid_document_reference",
        ReferenceCycle => "reference_cycle",
        ReferenceDepthExceeded => "reference_depth_exceeded",
        LoadNotImplemented => "load_not_implemented",
    }
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in input.chars().flat_map(|character| character.to_lowercase()) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Url;

    #[test]
    fn status_precedence_prefers_ambiguity_over_setup_and_decisions() {
        assert_eq!(
            derive_status(
                &[],
                &[InterpretationAmbiguity {
                    ambiguity_key: "a".to_owned(),
                    summary: "x".to_owned(),
                    evidence: Vec::new(),
                }],
                &[InterpretationSetupRequirement {
                    requirement_key: "S".to_owned(),
                    requirement_kind: InterpretationSetupRequirementKind::Secret,
                    summary: None,
                    evidence: Vec::new(),
                }],
                &[InterpretationOwnerDecision {
                    decision_key: "d".to_owned(),
                    decision_text: "x".to_owned(),
                    evidence: Vec::new(),
                }],
            ),
            SkillBundleInterpretationStatus::Ambiguous
        );
    }

    #[test]
    fn ignores_supporting_document_preamble_when_projecting_ambiguities() {
        let source_url = Url::parse(
            "https://bundles.a2ex.local/skills/prediction-spread-arb/docs/owner-setup.md",
        )
        .expect("url parses");
        let bundle = SkillBundle {
            bundle_id: "official.prediction-spread-arb".to_owned(),
            bundle_format: "a2ex.skill-bundle/v1alpha1".to_owned(),
            bundle_version: "2026.03.12".to_owned(),
            compatible_daemon: Some(">=0.1.0".to_owned()),
            entry_document_id: "skill".to_owned(),
            name: Some("Prediction Spread Arb".to_owned()),
            summary: Some("Capture spread dislocations between prediction venues.".to_owned()),
            document_manifest: Vec::new(),
            documents: Vec::new(),
            unresolved_sections: vec![UnresolvedBundleSection {
                section_id: "owner-setup#preamble".to_owned(),
                document_id: "owner-setup".to_owned(),
                section_heading: "Preamble".to_owned(),
                section_slug: "preamble".to_owned(),
                heading_level: 0,
                source_url,
                markdown: "This bundle is ready once attached.".to_owned(),
            }],
        };

        let interpretation = interpret_skill_bundle(&bundle).expect("interpretation succeeds");

        assert_eq!(
            interpretation.status,
            SkillBundleInterpretationStatus::InterpretedReady
        );
        assert!(interpretation.ambiguities.is_empty());
    }
}
