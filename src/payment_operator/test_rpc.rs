//! A JSON-RPC node the escrow tests control.
//!
//! It answers what `EvmProvider::send_transaction_from` and the escrow reads
//! ask for, records every call, and keeps each raw transaction it is handed, so
//! a test can assert on the exact bytes that would have gone on chain -- and on
//! the absence of any, which is the whole point of a refusal.
//!
//! Contract reads (`eth_call`) are answered per `(to, selector)`; anything not
//! scripted comes back as `execution reverted`, never as a plausible value.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy::consensus::Transaction as _;
use alloy::eips::Decodable2718 as _;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, B256};
use alloy::signers::local::PrivateKeySigner;
use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::chain::evm::{EvmChain, EvmProvider};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};

/// The hash every `eth_sendRawTransaction` answers with. The receipt the node
/// serves carries the hash it is asked about, so the value itself is inert.
const SUBMITTED: [u8; 32] = [0x5e; 32];

/// What `eth_blockNumber` answers. Fixed, so the receipt watcher's heartbeat
/// has nothing to chase.
const TIP: u64 = 0x100;

/// How the node answers one scripted `eth_call`.
#[derive(Clone, Debug)]
pub(crate) enum CallAnswer {
    /// ABI-encoded return data.
    Return(Vec<u8>),
    /// A JSON-RPC error, as a node that cannot answer returns it.
    Error { code: i64, message: String },
}

/// One transaction the node was handed.
#[derive(Clone, Debug)]
pub(crate) struct SentTx {
    pub to: Option<Address>,
    pub from: Address,
    pub input: Vec<u8>,
    pub max_fee_per_gas: u128,
}

#[derive(Default)]
struct NodeState {
    chain_id: u64,
    calls: HashMap<(Address, [u8; 4]), CallAnswer>,
    /// Every `eth_call` answered, as `(to, selector)`.
    reads: Vec<(Address, [u8; 4])>,
    /// Every `eth_getCode` answered, in order.
    code_reads: Vec<Address>,
    code: HashMap<Address, Vec<u8>>,
    /// While set, every `eth_call` and `eth_getCode` fails as a rate limit.
    reads_fail: bool,
    /// While set, every receipt reports a reverted transaction.
    receipts_revert: bool,
    sent: Vec<SentTx>,
}

/// A running mock node.
#[derive(Clone)]
pub(crate) struct MockNode {
    pub url: String,
    state: Arc<Mutex<NodeState>>,
}

impl MockNode {
    /// Start a node that reports `network`'s chain id.
    pub(crate) async fn start(network: Network) -> Self {
        let chain_id = EvmChain::try_from(network)
            .expect("an EVM network")
            .chain_id;
        let state = Arc::new(Mutex::new(NodeState {
            chain_id,
            ..Default::default()
        }));
        let app = Router::new()
            .route("/", post(serve))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, state }
    }

    /// Script the answer to `eth_call(to, selector ...)`.
    pub(crate) fn on_call(&self, to: Address, selector: [u8; 4], answer: CallAnswer) {
        self.state
            .lock()
            .unwrap()
            .calls
            .insert((to, selector), answer);
    }

    /// Script the bytecode `eth_getCode(address)` answers.
    pub(crate) fn set_code(&self, address: Address, code: Vec<u8>) {
        self.state.lock().unwrap().code.insert(address, code);
    }

    /// Make every contract read fail (or succeed again).
    pub(crate) fn fail_reads(&self, fail: bool) {
        self.state.lock().unwrap().reads_fail = fail;
    }

    /// Make every receipt report a revert (or a success again).
    pub(crate) fn revert_receipts(&self, revert: bool) {
        self.state.lock().unwrap().receipts_revert = revert;
    }

    /// Every transaction handed to the node, in order.
    pub(crate) fn sent(&self) -> Vec<SentTx> {
        self.state.lock().unwrap().sent.clone()
    }

    /// Every contract read answered, as `(to, selector)`, in order.
    pub(crate) fn reads(&self) -> Vec<(Address, [u8; 4])> {
        self.state.lock().unwrap().reads.clone()
    }

    /// Every address whose code was read, in order.
    pub(crate) fn code_reads(&self) -> Vec<Address> {
        self.state.lock().unwrap().code_reads.clone()
    }

    /// Forget what was sent and read so far; the script stays.
    pub(crate) fn clear_log(&self) {
        let mut state = self.state.lock().unwrap();
        state.sent.clear();
        state.reads.clear();
        state.code_reads.clear();
    }
}

/// Two signers from fixed seeds, so the pinned signer is the same address on
/// every run and a test can tell it apart from the other one.
pub(crate) fn fixed_wallet() -> EthereumWallet {
    let first = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).expect("valid key");
    let second = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x22)).expect("valid key");
    let mut wallet = EthereumWallet::from(first);
    wallet.register_signer(second);
    wallet
}

/// An `EvmProvider` for `network` that talks to `node`.
pub(crate) async fn provider(network: Network, node: &MockNode, eip1559: bool) -> EvmProvider {
    EvmProvider::try_new(fixed_wallet(), &node.url, eip1559, network)
        .await
        .expect("provider")
}

/// A provider map holding exactly the providers a test built.
pub(crate) struct Providers(pub HashMap<Network, NetworkProvider>);

impl Providers {
    pub(crate) fn one(network: Network, provider: EvmProvider) -> Self {
        let mut map = HashMap::new();
        map.insert(network, NetworkProvider::Evm(provider));
        Self(map)
    }
}

impl ProviderMap for Providers {
    type Value = NetworkProvider;
    fn by_network<N: std::borrow::Borrow<Network>>(&self, network: N) -> Option<&NetworkProvider> {
        self.0.get(network.borrow())
    }
    fn values(&self) -> impl Iterator<Item = &NetworkProvider> + Send {
        self.0.values()
    }
}

impl HasProviderMap for Providers {
    type Map = Self;
    fn provider_map(&self) -> &Self {
        self
    }
}

/// The EVM provider inside a [`Providers`] map.
pub(crate) fn evm(providers: &Providers, network: Network) -> &EvmProvider {
    match providers.0.get(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => panic!("no EVM provider for {network}"),
    }
}

fn hex_quantity(n: u128) -> Value {
    json!(format!("{n:#x}"))
}

fn receipt_json(hash: &str, reverted: bool) -> Value {
    let zero32 = format!("0x{}", hex::encode([0u8; 32]));
    json!({
        "transactionHash": hash,
        "transactionIndex": "0x0",
        "blockHash": zero32,
        "blockNumber": hex_quantity(TIP as u128),
        "from": format!("0x{}", hex::encode([0u8; 20])),
        "to": format!("0x{}", hex::encode([0u8; 20])),
        "cumulativeGasUsed": "0x5208",
        "gasUsed": "0x5208",
        "contractAddress": null,
        "logsBloom": format!("0x{}", "0".repeat(512)),
        "status": if reverted { "0x0" } else { "0x1" },
        "type": "0x0",
        "effectiveGasPrice": "0x3b9aca00",
        "logs": []
    })
}

fn block_json(number: u64) -> Value {
    let zero32 = format!("0x{}", hex::encode([0u8; 32]));
    json!({
        "hash": zero32,
        "parentHash": zero32,
        "sha3Uncles": zero32,
        "miner": format!("0x{}", hex::encode([0u8; 20])),
        "stateRoot": zero32,
        "transactionsRoot": zero32,
        "receiptsRoot": zero32,
        "logsBloom": format!("0x{}", "0".repeat(512)),
        "difficulty": "0x0",
        "number": hex_quantity(number as u128),
        "gasLimit": "0x1c9c380",
        "gasUsed": "0x5208",
        "timestamp": "0x68c00000",
        "extraData": "0x",
        "mixHash": zero32,
        "nonce": "0x0000000000000000",
        "baseFeePerGas": "0x1",
        "totalDifficulty": "0x0",
        "size": "0x220",
        "transactions": [],
        "uncles": []
    })
}

/// The address that signed `tx`, recovered from its signature.
fn sender(tx: &alloy::consensus::TxEnvelope) -> Address {
    use alloy::consensus::TxEnvelope;
    let (signature, hash) = match tx {
        TxEnvelope::Legacy(s) => (*s.signature(), s.signature_hash()),
        TxEnvelope::Eip2930(s) => (*s.signature(), s.signature_hash()),
        TxEnvelope::Eip1559(s) => (*s.signature(), s.signature_hash()),
        TxEnvelope::Eip4844(s) => (*s.signature(), s.signature_hash()),
        TxEnvelope::Eip7702(s) => (*s.signature(), s.signature_hash()),
    };
    signature
        .recover_address_from_prehash(&hash)
        .expect("a signed transaction")
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn answer(state: &Mutex<NodeState>, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(json!(1));
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let mut state = state.lock().unwrap();
    let result = match method {
        "eth_chainId" => hex_quantity(state.chain_id as u128),
        "eth_getTransactionCount" => json!("0x0"),
        "eth_gasPrice" => json!("0x3b9aca00"),
        "eth_maxPriorityFeePerGas" => json!("0x0"),
        "eth_feeHistory" => json!({
            "oldestBlock": "0x1",
            "baseFeePerGas": ["0x3b9aca00", "0x3b9aca00"],
            "gasUsedRatio": [0.5]
        }),
        "eth_estimateGas" => json!("0x30000"),
        "eth_blockNumber" => hex_quantity(TIP as u128),
        "eth_getBlockByNumber" => {
            let number = params[0]
                .as_str()
                .and_then(|n| u64::from_str_radix(n.trim_start_matches("0x"), 16).ok())
                .unwrap_or(TIP);
            block_json(number)
        }
        "eth_getTransactionReceipt" => receipt_json(
            params[0].as_str().unwrap_or_default(),
            state.receipts_revert,
        ),
        "eth_sendRawTransaction" => {
            let raw = params[0].as_str().unwrap_or_default();
            let raw = hex::decode(raw.trim_start_matches("0x")).expect("raw tx hex");
            let tx = alloy::consensus::TxEnvelope::decode_2718(&mut raw.as_slice())
                .expect("a decodable transaction");
            state.sent.push(SentTx {
                to: tx.to(),
                from: sender(&tx),
                input: tx.input().to_vec(),
                max_fee_per_gas: tx.max_fee_per_gas(),
            });
            json!(format!("0x{}", hex::encode(SUBMITTED)))
        }
        "eth_getCode" => {
            if state.reads_fail {
                return rpc_error(id, -32005, "rate limit exceeded");
            }
            let address: Address = params[0]
                .as_str()
                .and_then(|a| a.parse().ok())
                .expect("an address");
            state.code_reads.push(address);
            let code = state.code.get(&address).cloned().unwrap_or_default();
            json!(format!("0x{}", hex::encode(code)))
        }
        "eth_call" => {
            if state.reads_fail {
                return rpc_error(id, -32005, "rate limit exceeded");
            }
            let to: Address = params[0]["to"]
                .as_str()
                .and_then(|a| a.parse().ok())
                .expect("a call target");
            let input = params[0]
                .get("input")
                .or_else(|| params[0].get("data"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = hex::decode(input.trim_start_matches("0x")).expect("call data hex");
            let mut selector = [0u8; 4];
            selector.copy_from_slice(&input[..4]);
            state.reads.push((to, selector));
            match state.calls.get(&(to, selector)).cloned() {
                Some(CallAnswer::Return(data)) => json!(format!("0x{}", hex::encode(data))),
                Some(CallAnswer::Error { code, message }) => {
                    return rpc_error(id, code, &message);
                }
                None => return rpc_error(id, 3, "execution reverted"),
            }
        }
        other => panic!("the mock node was asked for {other}, which no escrow test scripts"),
    };
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

async fn serve(State(state): State<Arc<Mutex<NodeState>>>, Json(body): Json<Value>) -> Json<Value> {
    Json(match &body {
        Value::Array(reqs) => Value::Array(reqs.iter().map(|r| answer(&state, r)).collect()),
        req => answer(&state, req),
    })
}
