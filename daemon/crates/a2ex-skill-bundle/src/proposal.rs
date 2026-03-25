use std::collections::BTreeSet;

use crate::BundleResult;
use crate::model::{
    BundleLoadOutcome, InterpretationEvidence, ProposalQuantitativeCompleteness,
    ProposalQuantitativeProfile, ProposalReadiness, SkillBundleInterpretation,
    SkillBundleInterpretationStatus, SkillProposalPacket,
};

pub fn generate_proposal_packet(
    outcome: &BundleLoadOutcome,
    interpretation: &SkillBundleInterpretation,
) -> BundleResult<SkillProposalPacket> {
    Ok(SkillProposalPacket {
        interpretation_status: interpretation.status.clone(),
        proposal_readiness: derive_proposal_readiness(&interpretation.status),
        plan_summary: interpretation.plan_summary.clone(),
        setup_requirements: interpretation.setup_requirements.clone(),
        owner_override_points: interpretation.owner_decisions.clone(),
        automation_boundaries: interpretation.automation_boundaries.clone(),
        risk_framing: interpretation.risks.clone(),
        capital_profile: build_quantitative_profile(
            QuantitativeField::Capital,
            outcome,
            interpretation,
        ),
        cost_profile: build_quantitative_profile(QuantitativeField::Cost, outcome, interpretation),
        ambiguities: interpretation.ambiguities.clone(),
        blockers: interpretation.blockers.clone(),
        provenance: interpretation.provenance.clone(),
    })
}

fn derive_proposal_readiness(status: &SkillBundleInterpretationStatus) -> ProposalReadiness {
    match status {
        SkillBundleInterpretationStatus::Blocked => ProposalReadiness::Blocked,
        SkillBundleInterpretationStatus::InterpretedReady => ProposalReadiness::Ready,
        SkillBundleInterpretationStatus::NeedsSetup
        | SkillBundleInterpretationStatus::NeedsOwnerDecision
        | SkillBundleInterpretationStatus::Ambiguous => ProposalReadiness::Incomplete,
    }
}

fn build_quantitative_profile(
    field: QuantitativeField,
    outcome: &BundleLoadOutcome,
    interpretation: &SkillBundleInterpretation,
) -> ProposalQuantitativeProfile {
    match interpretation.status {
        SkillBundleInterpretationStatus::Blocked => ProposalQuantitativeProfile {
            completeness: ProposalQuantitativeCompleteness::Blocked,
            summary: format!(
                "{} is blocked pending load and interpretation blockers.",
                field.label()
            ),
            reason: format!(
                "{} is blocked because owner setup or other required bundle truth is missing: {}.",
                field.title_case_label(),
                interpretation
                    .blockers
                    .first()
                    .map(|blocker| blocker.summary.as_str())
                    .unwrap_or("bundle blockers remain unresolved")
            ),
            evidence: collect_unique_evidence(
                interpretation
                    .blockers
                    .iter()
                    .flat_map(|blocker| blocker.evidence.iter())
                    .chain(interpretation.provenance.iter())
                    .chain(fallback_outcome_evidence(outcome).iter()),
            ),
        },
        SkillBundleInterpretationStatus::NeedsOwnerDecision => ProposalQuantitativeProfile {
            completeness: ProposalQuantitativeCompleteness::RequiresOwnerInput,
            summary: format!(
                "{} depends on owner choices that are still unresolved.",
                field.title_case_label()
            ),
            reason: format!(
                "{} requires owner input before it can be stated truthfully because the bundle still asks the owner to decide: {}.",
                field.title_case_label(),
                join_with_semicolons(
                    interpretation
                        .owner_decisions
                        .iter()
                        .map(|decision| decision.decision_text.as_str()),
                )
            ),
            evidence: collect_unique_evidence(
                interpretation
                    .owner_decisions
                    .iter()
                    .flat_map(|decision| decision.evidence.iter())
                    .chain(plan_summary_evidence(interpretation).iter())
                    .chain(interpretation.provenance.iter())
                    .chain(fallback_outcome_evidence(outcome).iter()),
            ),
        },
        SkillBundleInterpretationStatus::InterpretedReady => ProposalQuantitativeProfile {
            completeness: ProposalQuantitativeCompleteness::NotInBundleContract,
            summary: format!(
                "{} is not specified by the current bundle contract.",
                field.title_case_label()
            ),
            reason: format!(
                "The bundle does not specify {} in the current contract, so the proposal packet leaves this field incomplete instead of inventing quantitative values.",
                field.contract_phrase()
            ),
            evidence: collect_unique_evidence(
                plan_summary_evidence(interpretation)
                    .iter()
                    .chain(interpretation.provenance.iter())
                    .chain(fallback_outcome_evidence(outcome).iter()),
            ),
        },
        SkillBundleInterpretationStatus::Ambiguous
        | SkillBundleInterpretationStatus::NeedsSetup => ProposalQuantitativeProfile {
            completeness: ProposalQuantitativeCompleteness::Unknown,
            summary: format!("{} is currently unknown.", field.title_case_label()),
            reason: format!(
                "The bundle does not specify {} and the current interpretation remains {}. The proposal packet keeps this quantitative field explicit and incomplete rather than guessing.",
                field.contract_phrase(),
                interpretation_status_reason(&interpretation.status)
            ),
            evidence: collect_unique_evidence(
                interpretation
                    .ambiguities
                    .iter()
                    .flat_map(|ambiguity| ambiguity.evidence.iter())
                    .chain(
                        interpretation
                            .setup_requirements
                            .iter()
                            .flat_map(|requirement| requirement.evidence.iter()),
                    )
                    .chain(plan_summary_evidence(interpretation).iter())
                    .chain(interpretation.provenance.iter())
                    .chain(fallback_outcome_evidence(outcome).iter()),
            ),
        },
    }
}

fn interpretation_status_reason(status: &SkillBundleInterpretationStatus) -> &'static str {
    match status {
        SkillBundleInterpretationStatus::InterpretedReady => "ready",
        SkillBundleInterpretationStatus::NeedsSetup => "waiting on setup requirements",
        SkillBundleInterpretationStatus::NeedsOwnerDecision => "waiting on owner decisions",
        SkillBundleInterpretationStatus::Ambiguous => "ambiguous",
        SkillBundleInterpretationStatus::Blocked => "blocked",
    }
}

fn collect_unique_evidence<'a>(
    evidence: impl IntoIterator<Item = &'a InterpretationEvidence>,
) -> Vec<InterpretationEvidence> {
    let mut seen = BTreeSet::new();
    let mut collected = Vec::new();

    for item in evidence {
        if seen.insert(item.clone()) {
            collected.push(item.clone());
        }
    }

    collected
}

fn plan_summary_evidence(
    interpretation: &SkillBundleInterpretation,
) -> Vec<InterpretationEvidence> {
    interpretation
        .plan_summary
        .as_ref()
        .map(|summary| summary.evidence.clone())
        .unwrap_or_default()
}

fn fallback_outcome_evidence(outcome: &BundleLoadOutcome) -> Vec<InterpretationEvidence> {
    outcome
        .diagnostics
        .iter()
        .filter_map(|diagnostic| {
            diagnostic
                .source_url
                .clone()
                .map(|source_url| InterpretationEvidence {
                    document_id: diagnostic
                        .document_id
                        .clone()
                        .unwrap_or_else(|| "bundle".to_owned()),
                    section_id: None,
                    section_slug: diagnostic.section_slug.clone(),
                    source_url,
                })
        })
        .collect()
}

fn join_with_semicolons<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let joined = values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("; ");

    if joined.is_empty() {
        "owner decisions remain unresolved".to_owned()
    } else {
        joined
    }
}

#[derive(Clone, Copy)]
enum QuantitativeField {
    Capital,
    Cost,
}

impl QuantitativeField {
    fn label(self) -> &'static str {
        match self {
            Self::Capital => "capital profile",
            Self::Cost => "cost profile",
        }
    }

    fn title_case_label(self) -> &'static str {
        match self {
            Self::Capital => "Capital profile",
            Self::Cost => "Cost profile",
        }
    }

    fn contract_phrase(self) -> &'static str {
        match self {
            Self::Capital => "capital requirements, bankroll sizing, or venue allocation limits",
            Self::Cost => "cost estimates, fee ranges, or execution expenses",
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    use super::*;
    use crate::model::{
        InterpretationEvidence, InterpretationOwnerDecision, InterpretationPlanSummary,
    };

    #[test]
    fn ready_packets_mark_quantitative_sections_as_not_in_bundle_contract() {
        let interpretation = SkillBundleInterpretation {
            status: SkillBundleInterpretationStatus::InterpretedReady,
            plan_summary: Some(InterpretationPlanSummary {
                bundle_id: "official.demo".to_owned(),
                title: Some("Demo".to_owned()),
                summary: Some("Summary".to_owned()),
                overview: Some("Overview".to_owned()),
                evidence: vec![sample_evidence()],
            }),
            owner_decisions: Vec::new(),
            setup_requirements: Vec::new(),
            automation_boundaries: Vec::new(),
            risks: Vec::new(),
            ambiguities: Vec::new(),
            blockers: Vec::new(),
            provenance: vec![sample_evidence()],
        };
        let outcome = BundleLoadOutcome {
            bundle: None,
            diagnostics: Vec::new(),
        };

        let packet = generate_proposal_packet(&outcome, &interpretation).expect("packet builds");

        assert_eq!(packet.proposal_readiness, ProposalReadiness::Ready);
        assert_eq!(
            packet.capital_profile.completeness,
            ProposalQuantitativeCompleteness::NotInBundleContract
        );
        assert!(packet.capital_profile.reason.contains("does not specify"));
        assert_eq!(
            packet.cost_profile.completeness,
            ProposalQuantitativeCompleteness::NotInBundleContract
        );
        assert!(packet.cost_profile.reason.contains("does not specify"));
    }

    #[test]
    fn incomplete_packets_can_require_owner_input_for_quantitative_truth() {
        let decision_evidence = sample_evidence();
        let interpretation = SkillBundleInterpretation {
            status: SkillBundleInterpretationStatus::NeedsOwnerDecision,
            plan_summary: Some(InterpretationPlanSummary {
                bundle_id: "official.demo".to_owned(),
                title: Some("Demo".to_owned()),
                summary: None,
                overview: Some("Overview".to_owned()),
                evidence: vec![decision_evidence.clone()],
            }),
            owner_decisions: vec![InterpretationOwnerDecision {
                decision_key: "choose-target-venues".to_owned(),
                decision_text: "Choose target venues".to_owned(),
                evidence: vec![decision_evidence.clone()],
            }],
            setup_requirements: Vec::new(),
            automation_boundaries: Vec::new(),
            risks: Vec::new(),
            ambiguities: Vec::new(),
            blockers: Vec::new(),
            provenance: vec![decision_evidence.clone()],
        };
        let outcome = BundleLoadOutcome {
            bundle: None,
            diagnostics: Vec::new(),
        };

        let packet = generate_proposal_packet(&outcome, &interpretation).expect("packet builds");

        assert_eq!(packet.proposal_readiness, ProposalReadiness::Incomplete);
        assert_eq!(
            packet.capital_profile.completeness,
            ProposalQuantitativeCompleteness::RequiresOwnerInput
        );
        assert!(
            packet
                .capital_profile
                .reason
                .contains("Choose target venues")
        );
        assert_eq!(
            packet.cost_profile.completeness,
            ProposalQuantitativeCompleteness::RequiresOwnerInput
        );
    }

    fn sample_evidence() -> InterpretationEvidence {
        InterpretationEvidence {
            document_id: "skill".to_owned(),
            section_id: Some("skill#overview".to_owned()),
            section_slug: Some("overview".to_owned()),
            source_url: Url::parse("https://bundles.a2ex.local/skills/demo/skill.md")
                .expect("sample url"),
        }
    }
}
