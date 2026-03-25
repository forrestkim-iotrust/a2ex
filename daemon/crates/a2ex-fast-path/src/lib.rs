use a2ex_compiler::CompiledIntent;
use a2ex_gateway::FastPathRoute;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FastActionTemplate {
    GenericContractCall {
        chain_id: u64,
        to: String,
        value_wei: String,
        calldata: Vec<u8>,
    },
    SimpleEntry {
        venue: String,
        market: String,
        side: String,
        notional_usd: u64,
    },
    HedgeAdjustPrecomputed {
        venue: String,
        instrument: String,
        target_delta_bps: i64,
        notional_usd: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedFastAction {
    pub action_id: String,
    pub request_id: String,
    pub reservation_id: String,
    pub action_kind: String,
    pub venue: String,
    pub payload: PreparedVenueAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreparedVenueAction {
    GenericContractCall {
        chain_id: u64,
        to: String,
        value_wei: String,
        calldata: Vec<u8>,
        reservation_amount_usd: u64,
    },
    SimpleEntry {
        venue: String,
        market: String,
        side: String,
        notional_usd: u64,
    },
    HedgeAdjustPrecomputed {
        venue: String,
        instrument: String,
        target_delta_bps: i64,
        notional_usd: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastPathPreparationInput<'a> {
    pub route: &'a FastPathRoute,
    pub reservation_id: &'a str,
    pub template: FastActionTemplate,
    pub request_id: &'a str,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FastPathError {
    #[error("request {request_id} does not have a fast-path venue")]
    MissingVenue { request_id: String },
    #[error("request {request_id} does not produce a supported deterministic fast action")]
    UnsupportedIntent { request_id: String },
}

pub fn template_from_compiled_intent(
    intent: &CompiledIntent,
    route: &FastPathRoute,
) -> Result<FastActionTemplate, FastPathError> {
    let notional = intent.objective.target_notional_usd;

    if intent.intent_type.contains("contract") || intent.objective.domain.contains("contract") {
        return Ok(FastActionTemplate::GenericContractCall {
            chain_id: 8453,
            to: format!("0x{}", deterministic_hex(&intent.intent_id, 20)),
            value_wei: "0".to_owned(),
            calldata: deterministic_bytes(&intent.audit.request_id),
        });
    }

    if route.venue == "hyperliquid" {
        return Ok(FastActionTemplate::HedgeAdjustPrecomputed {
            venue: route.venue.clone(),
            instrument: intent.objective.target_market.clone(),
            target_delta_bps: 0,
            notional_usd: notional,
        });
    }

    if !route.venue.is_empty() {
        return Ok(FastActionTemplate::SimpleEntry {
            venue: route.venue.clone(),
            market: intent.objective.target_market.clone(),
            side: intent.objective.side.clone(),
            notional_usd: notional,
        });
    }

    Err(FastPathError::MissingVenue {
        request_id: intent.audit.request_id.clone(),
    })
}

pub fn prepare_fast_action(
    input: FastPathPreparationInput<'_>,
) -> Result<PreparedFastAction, FastPathError> {
    let (action_kind, venue, payload, seed) = match input.template {
        FastActionTemplate::GenericContractCall {
            chain_id,
            to,
            value_wei,
            calldata,
        } => {
            let seed = format!(
                "generic:{chain_id}:{to}:{value_wei}:{}",
                hex::encode(&calldata)
            );
            (
                "generic_contract_call".to_owned(),
                input.route.venue.clone(),
                PreparedVenueAction::GenericContractCall {
                    chain_id,
                    to,
                    value_wei,
                    reservation_amount_usd: reservation_amount_from_request(input.request_id),
                    calldata,
                },
                seed,
            )
        }
        FastActionTemplate::SimpleEntry {
            venue,
            market,
            side,
            notional_usd,
        } => {
            let seed = format!("simple:{venue}:{market}:{side}:{notional_usd}");
            (
                "simple_entry".to_owned(),
                venue.clone(),
                PreparedVenueAction::SimpleEntry {
                    venue,
                    market,
                    side,
                    notional_usd,
                },
                seed,
            )
        }
        FastActionTemplate::HedgeAdjustPrecomputed {
            venue,
            instrument,
            target_delta_bps,
            notional_usd,
        } => {
            let seed = format!("hedge:{venue}:{instrument}:{target_delta_bps}:{notional_usd}");
            (
                "hedge_adjust_precomputed".to_owned(),
                venue.clone(),
                PreparedVenueAction::HedgeAdjustPrecomputed {
                    venue,
                    instrument,
                    target_delta_bps,
                    notional_usd,
                },
                seed,
            )
        }
    };

    Ok(PreparedFastAction {
        action_id: format!(
            "fp-{}",
            deterministic_hex(
                &format!("{}:{}:{}", input.request_id, input.reservation_id, seed),
                12
            )
        ),
        request_id: input.request_id.to_owned(),
        reservation_id: input.reservation_id.to_owned(),
        action_kind,
        venue,
        payload,
    })
}

fn reservation_amount_from_request(request_id: &str) -> u64 {
    let _ = request_id;
    25
}

fn deterministic_bytes(seed: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.finalize()[..16].to_vec()
}

fn deterministic_hex(seed: &str, bytes: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hex::encode(&hasher.finalize()[..bytes])
}
