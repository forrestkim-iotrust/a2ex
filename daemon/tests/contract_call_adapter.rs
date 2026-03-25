mod support;

use a2ex_evm_adapter::{
    EvmAdapter, PreparedEvmTransaction, ProviderBackedEvmAdapter, SignedTransactionBytes,
    TxLifecycleStatus,
};
use support::anvil_harness::spawn_anvil;

#[tokio::test]
async fn contract_call_adapter_tracks_tx_lifecycle() {
    let anvil = spawn_anvil().await;
    let adapter = ProviderBackedEvmAdapter::new(anvil.endpoint_url())
        .expect("provider-backed adapter builds");

    let confirmed = adapter
        .submit_and_watch(
            PreparedEvmTransaction {
                chain_id: 8453,
                to: "0xabc".to_owned(),
                value_wei: "0".to_owned(),
                calldata: vec![1, 2, 3],
            },
            SignedTransactionBytes {
                bytes: anvil.confirmed_signed_bytes(),
            },
        )
        .await
        .expect("confirmed lifecycle succeeds");

    assert_eq!(
        confirmed
            .events
            .iter()
            .map(|event| event.status.clone())
            .collect::<Vec<_>>(),
        vec![
            TxLifecycleStatus::Submitted,
            TxLifecycleStatus::Pending,
            TxLifecycleStatus::Confirmed,
        ]
    );
    let confirmed_terminal = confirmed.events.last().expect("confirmed terminal event");
    assert_eq!(
        confirmed_terminal.metadata.tx_hash,
        format!("0x{}", "11".repeat(32))
    );
    assert_eq!(confirmed_terminal.metadata.block_number, Some(18));
    assert_eq!(confirmed_terminal.metadata.receipt_status, "confirmed");
    assert_eq!(confirmed_terminal.metadata.error, None);

    let failed = adapter
        .submit_and_watch(
            PreparedEvmTransaction {
                chain_id: 8453,
                to: "0xdef".to_owned(),
                value_wei: "0".to_owned(),
                calldata: vec![4, 5, 6],
            },
            SignedTransactionBytes {
                bytes: anvil.reverted_signed_bytes(),
            },
        )
        .await
        .expect("failed lifecycle still reports");

    let failed_terminal = failed.events.last().expect("failed terminal event");
    assert_eq!(failed_terminal.status, TxLifecycleStatus::Failed);
    assert_eq!(
        failed_terminal.metadata.tx_hash,
        format!("0x{}", "22".repeat(32))
    );
    assert_eq!(failed_terminal.metadata.block_number, Some(19));
    assert_eq!(failed_terminal.metadata.receipt_status, "0x0");
    assert_eq!(
        failed_terminal.metadata.error.as_deref(),
        Some("transaction receipt status indicates failure")
    );
}
