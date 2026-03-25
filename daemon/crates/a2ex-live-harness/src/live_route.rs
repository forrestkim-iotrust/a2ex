use serde::{Deserialize, Serialize};

use crate::waiaas::DEFAULT_WALLET_BOUNDARY;

pub const S02_ROUTE_ID: &str = "across-mainnet-to-base-usdc-smoke";
pub const S02_ROUTE_SUCCESS_SIGNAL: &str = "confirmed destination-chain USDC receipt on Base for the same run_id/install_id/proposal_id/selection_id";
pub const S02_ROUTE_RISK_ENVELOPE: &str = "10 USDC max";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRouteContract {
    pub route_id: String,
    pub venue: String,
    pub source_chain: String,
    pub destination_chain: String,
    pub asset: String,
    pub wallet_boundary: String,
    pub risk_envelope: LiveRiskEnvelope,
    pub success_criteria: LiveSuccessCriteria,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRiskEnvelope {
    pub max_notional: String,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSuccessCriteria {
    pub decisive_signal: String,
    pub required_evidence_fields: Vec<RequiredEvidenceField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredEvidenceField {
    pub evidence_key: String,
    pub description: String,
}

pub fn s02_live_route_contract() -> LiveRouteContract {
    LiveRouteContract {
        route_id: S02_ROUTE_ID.to_owned(),
        venue: "Across".to_owned(),
        source_chain: "Ethereum mainnet".to_owned(),
        destination_chain: "Base".to_owned(),
        asset: "USDC".to_owned(),
        wallet_boundary: DEFAULT_WALLET_BOUNDARY.to_owned(),
        risk_envelope: LiveRiskEnvelope {
            max_notional: S02_ROUTE_RISK_ENVELOPE.to_owned(),
            rationale:
                "bounded smoke path for one Across Ethereum mainnet → Base USDC transfer; no dynamic route expansion"
                    .to_owned(),
        },
        success_criteria: LiveSuccessCriteria {
            decisive_signal: S02_ROUTE_SUCCESS_SIGNAL.to_owned(),
            required_evidence_fields: vec![
                RequiredEvidenceField {
                    evidence_key: "waiaas_authority.evidence_ref".to_owned(),
                    description:
                        "waiaas-authority.json proving the governing WAIaaS session, policy, authority_decision, and wallet_boundary"
                            .to_owned(),
                },
                RequiredEvidenceField {
                    evidence_key: "live_route_evidence.destination_chain_receipt_ref".to_owned(),
                    description:
                        "live-route-evidence.json with the decisive destination-chain USDC receipt on Base"
                            .to_owned(),
                },
                RequiredEvidenceField {
                    evidence_key: "final_classification.decisive_evidence_ref".to_owned(),
                    description:
                        "the final verdict must point at the decisive evidence instead of relying on approval or mutation receipts alone"
                            .to_owned(),
                },
            ],
        },
    }
}
