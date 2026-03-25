use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Clone)]
struct RpcState {
    chain_id: u64,
    latest_block: Arc<Mutex<u64>>,
    receipts: Arc<Mutex<HashMap<String, Value>>>,
    tx_hashes: Arc<HashMap<String, String>>,
}

pub struct AnvilHarness {
    endpoint_url: String,
    server: JoinHandle<()>,
}

impl AnvilHarness {
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    pub fn confirmed_signed_bytes(&self) -> Vec<u8> {
        vec![1, 2, 3]
    }

    pub fn reverted_signed_bytes(&self) -> Vec<u8> {
        vec![4, 5, 6]
    }
}

impl Drop for AnvilHarness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub async fn spawn_anvil() -> AnvilHarness {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local rpc harness");
    let address = listener.local_addr().expect("read local addr");
    let tx_hashes = Arc::new(HashMap::from([
        ("0x010203".to_owned(), fixed_hash("11")),
        ("0x040506".to_owned(), fixed_hash("22")),
    ]));
    let state = RpcState {
        chain_id: 31337,
        latest_block: Arc::new(Mutex::new(17)),
        receipts: Arc::new(Mutex::new(HashMap::from([
            (
                tx_hashes["0x010203"].clone(),
                receipt(tx_hashes["0x010203"].clone(), 18, true),
            ),
            (
                tx_hashes["0x040506"].clone(),
                receipt(tx_hashes["0x040506"].clone(), 19, false),
            ),
        ]))),
        tx_hashes,
    };

    let app = Router::new().route("/", post(handle_rpc)).with_state(state);
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    AnvilHarness {
        endpoint_url: format!("http://{}", address),
        server,
    }
}

async fn handle_rpc(State(state): State<RpcState>, Json(request): Json<Value>) -> Json<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .expect("rpc method");
    let params = request
        .get("params")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let result = match method {
        "eth_chainId" => json!(format!("0x{:x}", state.chain_id)),
        "eth_sendRawTransaction" => {
            let raw_tx = params
                .first()
                .and_then(Value::as_str)
                .expect("raw tx bytes parameter");
            json!(
                state
                    .tx_hashes
                    .get(raw_tx)
                    .cloned()
                    .expect("known tx payload")
            )
        }
        "eth_blockNumber" => {
            let mut block = state.latest_block.lock().expect("latest block lock");
            *block += 1;
            json!(format!("0x{:x}", *block))
        }
        "eth_getBlockByNumber" => {
            let block = params.first().and_then(Value::as_str).unwrap_or("latest");
            let block_number = if block == "latest" {
                *state.latest_block.lock().expect("latest block lock")
            } else {
                parse_hex_u64(block)
            };
            json!(block_response(block_number))
        }
        "eth_getTransactionReceipt" => {
            let tx_hash = params
                .first()
                .and_then(Value::as_str)
                .expect("tx hash parameter");
            state
                .receipts
                .lock()
                .expect("receipt map lock")
                .get(tx_hash)
                .cloned()
                .unwrap_or(Value::Null)
        }
        _ => panic!("unsupported rpc method: {method}"),
    };

    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn receipt(tx_hash: String, block_number: u64, success: bool) -> Value {
    json!({
        "transactionHash": tx_hash,
        "transactionIndex": "0x0",
        "blockHash": fixed_hash(if success { "aa" } else { "bb" }),
        "blockNumber": format!("0x{:x}", block_number),
        "from": fixed_address("01"),
        "to": fixed_address("02"),
        "cumulativeGasUsed": "0x5208",
        "gasUsed": "0x5208",
        "contractAddress": Value::Null,
        "logs": [],
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "type": "0x2",
        "status": if success { "0x1" } else { "0x0" },
        "effectiveGasPrice": "0x1"
    })
}

fn block_response(block_number: u64) -> Value {
    json!({
        "number": format!("0x{:x}", block_number),
        "hash": fixed_hash("cc"),
        "parentHash": fixed_hash("dd"),
        "nonce": "0x0000000000000000",
        "sha3Uncles": fixed_hash("ee"),
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "transactionsRoot": fixed_hash("ff"),
        "stateRoot": fixed_hash("10"),
        "receiptsRoot": fixed_hash("20"),
        "miner": fixed_address("03"),
        "difficulty": "0x0",
        "totalDifficulty": "0x0",
        "extraData": "0x",
        "size": "0x1",
        "gasLimit": "0x1c9c380",
        "gasUsed": "0x0",
        "timestamp": "0x1",
        "transactions": [],
        "uncles": [],
        "baseFeePerGas": "0x1",
        "withdrawals": []
    })
}

fn fixed_hash(byte: &str) -> String {
    format!("0x{}", byte.repeat(32))
}

fn fixed_address(byte: &str) -> String {
    format!("0x{}", byte.repeat(20))
}

fn parse_hex_u64(value: &str) -> u64 {
    value
        .strip_prefix("0x")
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .expect("hex u64")
}
