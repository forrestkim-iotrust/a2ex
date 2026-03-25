use a2ex_fast_path::{
    FastActionTemplate, FastPathPreparationInput, PreparedVenueAction, prepare_fast_action,
};
use a2ex_gateway::FastPathRoute;

#[test]
fn fast_action_templates_cover_generic_contract_call_simple_entry_and_precomputed_hedge_adjust() {
    let route = FastPathRoute {
        request_id: "req-templates".to_owned(),
        intent_id: "intent-templates".to_owned(),
        venue: "polymarket".to_owned(),
        summary: "fast path".to_owned(),
    };

    let generic = prepare_fast_action(FastPathPreparationInput {
        route: &route,
        reservation_id: "reservation-generic",
        request_id: "req-generic",
        template: FastActionTemplate::GenericContractCall {
            chain_id: 8453,
            to: "0xabc".to_owned(),
            value_wei: "0".to_owned(),
            calldata: vec![1, 2, 3],
        },
    })
    .expect("generic prepares");
    assert!(matches!(
        generic.payload,
        PreparedVenueAction::GenericContractCall { .. }
    ));

    let simple = prepare_fast_action(FastPathPreparationInput {
        route: &route,
        reservation_id: "reservation-simple",
        request_id: "req-simple",
        template: FastActionTemplate::SimpleEntry {
            venue: "polymarket".to_owned(),
            market: "market-1".to_owned(),
            side: "yes".to_owned(),
            notional_usd: 50,
        },
    })
    .expect("simple prepares");
    assert!(matches!(
        simple.payload,
        PreparedVenueAction::SimpleEntry { .. }
    ));

    let hedge = prepare_fast_action(FastPathPreparationInput {
        route: &route,
        reservation_id: "reservation-hedge",
        request_id: "req-hedge",
        template: FastActionTemplate::HedgeAdjustPrecomputed {
            venue: "hyperliquid".to_owned(),
            instrument: "btc-perp".to_owned(),
            target_delta_bps: -250,
            notional_usd: 75,
        },
    })
    .expect("hedge prepares");
    assert!(matches!(
        hedge.payload,
        PreparedVenueAction::HedgeAdjustPrecomputed { .. }
    ));
}
