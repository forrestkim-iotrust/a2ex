mod support;

use a2ex_hyperliquid_adapter::{
    HedgeOrderRequest, HyperliquidAdapter, HyperliquidCancelCommand, HyperliquidExchangeRequest,
    HyperliquidHedgeSubmitRequest, HyperliquidInfoRequest, HyperliquidModifyCommand,
    HyperliquidOpenOrder, HyperliquidOrderCommand, HyperliquidOrderStatus, HyperliquidPosition,
    HyperliquidSyncRequest, HyperliquidUserFill,
};
use support::hyperliquid_harness::FakeHyperliquidTransport;

#[tokio::test]
async fn hyperliquid_adapter_emits_exchange_payloads_for_place_modify_cancel() {
    let harness = FakeHyperliquidTransport::default();
    let adapter = HyperliquidAdapter::with_transport(harness.transport(), 4_200);

    let placed = adapter
        .place_order(HyperliquidOrderCommand {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            asset: 7,
            is_buy: false,
            price: "2412.7".to_owned(),
            size: "0.5".to_owned(),
            reduce_only: false,
            client_order_id: Some("hedge-open".to_owned()),
            time_in_force: "Ioc".to_owned(),
        })
        .await
        .expect("place order succeeds");

    let modified = adapter
        .modify_order(HyperliquidModifyCommand {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            order_id: 91,
            asset: 7,
            is_buy: false,
            price: "2410.0".to_owned(),
            size: "0.4".to_owned(),
            reduce_only: true,
            client_order_id: Some("hedge-open".to_owned()),
            time_in_force: "Alo".to_owned(),
        })
        .await
        .expect("modify order succeeds");

    let cancelled = adapter
        .cancel_order(HyperliquidCancelCommand {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            order_id: 91,
        })
        .await
        .expect("cancel order succeeds");

    let recorded = harness.exchange_requests();
    assert_eq!(recorded.len(), 3);
    assert_eq!(placed.nonce, 4_201);
    assert_eq!(modified.nonce, 4_202);
    assert_eq!(cancelled.nonce, 4_203);
    assert!(matches!(
        &recorded[0],
        HyperliquidExchangeRequest::Place(request)
            if request.nonce == 4_201
                && request.signer_address == "0xsigner"
                && request.account_address == "0xaccount"
                && request.orders[0].asset == 7
                && !request.orders[0].is_buy
                && request.orders[0].time_in_force == "Ioc"
    ));
    assert!(matches!(
        &recorded[1],
        HyperliquidExchangeRequest::Modify(request)
            if request.nonce == 4_202
                && request.signer_address == "0xsigner"
                && request.account_address == "0xaccount"
                && request.modifies[0].order_id == 91
                && request.modifies[0].time_in_force == "Alo"
                && request.modifies[0].reduce_only
    ));
    assert!(matches!(
        &recorded[2],
        HyperliquidExchangeRequest::Cancel(request)
            if request.nonce == 4_203
                && request.signer_address == "0xsigner"
                && request.account_address == "0xaccount"
                && request.cancels[0].order_id == 91
    ));
}

#[tokio::test]
async fn hyperliquid_adapter_nonce_allocator_survives_runtime_reuse() {
    let harness = FakeHyperliquidTransport::default();
    let adapter = HyperliquidAdapter::with_transport(harness.transport(), 9_000);

    let first = adapter
        .place_order(HyperliquidOrderCommand {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            asset: 3,
            is_buy: true,
            price: "100.0".to_owned(),
            size: "1.0".to_owned(),
            reduce_only: false,
            client_order_id: Some("first".to_owned()),
            time_in_force: "Ioc".to_owned(),
        })
        .await
        .expect("first order succeeds");
    let second = adapter
        .modify_order(HyperliquidModifyCommand {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            order_id: 17,
            asset: 3,
            is_buy: true,
            price: "101.0".to_owned(),
            size: "1.2".to_owned(),
            reduce_only: false,
            client_order_id: Some("first".to_owned()),
            time_in_force: "Ioc".to_owned(),
        })
        .await
        .expect("second order succeeds");

    assert_eq!(first.nonce, 9_001);
    assert_eq!(second.nonce, 9_002);
}

#[tokio::test]
async fn hyperliquid_adapter_keeps_signer_and_account_identities_distinct() {
    let harness = FakeHyperliquidTransport::default();
    let adapter = HyperliquidAdapter::with_transport(harness.transport(), 1);

    let _ = adapter
        .place_order(HyperliquidOrderCommand {
            signer_address: "0xsigner-wallet".to_owned(),
            account_address: "0xsubaccount".to_owned(),
            asset: 11,
            is_buy: false,
            price: "88.0".to_owned(),
            size: "2.0".to_owned(),
            reduce_only: true,
            client_order_id: Some("identity-check".to_owned()),
            time_in_force: "Ioc".to_owned(),
        })
        .await
        .expect("order succeeds");

    let recorded = harness.exchange_requests();
    assert!(matches!(
        &recorded[0],
        HyperliquidExchangeRequest::Place(request)
            if request.signer_address == "0xsigner-wallet"
                && request.account_address == "0xsubaccount"
                && request.signer_address != request.account_address
    ));
}

#[tokio::test]
async fn hyperliquid_adapter_places_and_syncs_hedges() {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(vec![HyperliquidOpenOrder {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        is_buy: false,
        price: "2412.7".to_owned(),
        size: "0.5".to_owned(),
        reduce_only: false,
        status: "resting".to_owned(),
        client_order_id: Some("hedge-open".to_owned()),
    }]);
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 91,
        status: "filled".to_owned(),
        filled_size: "0.5".to_owned(),
    });
    harness.seed_user_fills(vec![HyperliquidUserFill {
        order_id: 91,
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "0.5".to_owned(),
        price: "2412.7".to_owned(),
        side: "sell".to_owned(),
        filled_at: "2026-03-11T00:00:30Z".to_owned(),
    }]);
    harness.seed_positions(vec![HyperliquidPosition {
        asset: 7,
        instrument: "TOKEN-PERP".to_owned(),
        size: "-0.5".to_owned(),
        entry_price: "2412.7".to_owned(),
        position_value: "-1206.35".to_owned(),
    }]);
    let adapter = HyperliquidAdapter::with_transport(harness.transport(), 4_200);

    let prepared = adapter.prepare_order(
        None,
        HedgeOrderRequest {
            strategy_id: "strategy-lp-1".to_owned(),
            instrument: "TOKEN-PERP".to_owned(),
            notional_usd: 1_206,
            reduce_only: false,
        },
    );
    assert_eq!(prepared.nonce, 4_201);
    assert_eq!(prepared.instrument, "TOKEN-PERP");
    assert!(!prepared.reduce_only);
    assert!(prepared.client_order_id.starts_with("hl-strategy-lp-1-"));

    let placed = adapter
        .place_hedge_order(HyperliquidHedgeSubmitRequest {
            prepared: prepared.clone(),
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            asset: 7,
            is_buy: false,
            price: "2412.7".to_owned(),
            size: "0.5".to_owned(),
            time_in_force: "Ioc".to_owned(),
        })
        .await
        .expect("place order succeeds");
    let snapshot = adapter
        .sync_state(HyperliquidSyncRequest {
            signer_address: "0xsigner".to_owned(),
            account_address: "0xaccount".to_owned(),
            order_id: placed.order_id,
            aggregate_fills: true,
        })
        .await
        .expect("sync succeeds");

    let recorded = harness.exchange_requests();
    assert!(matches!(
        &recorded[0],
        HyperliquidExchangeRequest::Place(request)
            if request.nonce == 4_201
                && request.signer_address == "0xsigner"
                && request.account_address == "0xaccount"
                && request.orders[0].client_order_id.as_deref() == Some(prepared.client_order_id.as_str())
    ));
    assert_eq!(snapshot.queried_account, "0xaccount");
    assert_eq!(snapshot.queried_signer, "0xsigner");
    assert_eq!(snapshot.open_orders.len(), 1);
    assert_eq!(
        snapshot.order_status.expect("order status").status,
        "filled"
    );
    assert_eq!(snapshot.fills.len(), 1);
    assert_eq!(snapshot.positions.len(), 1);
}

#[tokio::test]
async fn hyperliquid_adapter_sync_uses_account_address_queries() {
    let harness = FakeHyperliquidTransport::default();
    harness.seed_open_orders(Vec::new());
    harness.seed_order_status(HyperliquidOrderStatus {
        order_id: 99,
        status: "resting".to_owned(),
        filled_size: "0.0".to_owned(),
    });
    harness.seed_user_fills(Vec::new());
    harness.seed_positions(Vec::new());
    let adapter = HyperliquidAdapter::with_transport(harness.transport(), 10);

    let _ = adapter
        .sync_state(HyperliquidSyncRequest {
            signer_address: "0xsigner-wallet".to_owned(),
            account_address: "0xsubaccount".to_owned(),
            order_id: Some(99),
            aggregate_fills: true,
        })
        .await
        .expect("sync succeeds");

    let info_requests = harness.info_requests();
    assert_eq!(info_requests.len(), 4);
    assert!(info_requests.iter().all(|request| match request {
        HyperliquidInfoRequest::OpenOrders { account_address }
        | HyperliquidInfoRequest::UserFills {
            account_address, ..
        }
        | HyperliquidInfoRequest::ClearinghouseState { account_address }
        | HyperliquidInfoRequest::OrderStatus {
            account_address, ..
        } => account_address == "0xsubaccount",
    }));
    assert!(info_requests.iter().all(|request| match request {
        HyperliquidInfoRequest::OpenOrders { account_address }
        | HyperliquidInfoRequest::UserFills {
            account_address, ..
        }
        | HyperliquidInfoRequest::ClearinghouseState { account_address }
        | HyperliquidInfoRequest::OrderStatus {
            account_address, ..
        } => account_address != "0xsigner-wallet",
    }));
}
