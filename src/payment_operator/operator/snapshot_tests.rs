//! Non-regression snapshot of the operator calls on the networks that already
//! settle escrow.
//!
//! `tests/fixtures/escrow/operator-calldata-snapshot.json` was recorded from
//! this harness running the code of release 2.40.0, before any other network
//! joined the escrow scheme, and is never re-recorded: a change that moves a
//! byte of what Base or SKALE are sent fails here, whatever else it fixes.
//!
//! Nothing is re-derived by hand. Each request goes through the same parse and
//! flow functions `/settle` uses, into an `EvmProvider` pointed at a node this
//! test runs, and what is compared is the raw transaction that node was handed:
//! target, sender and calldata.
//!
//! - `calldata`: `authorize`, `release` and `refundInEscrow` for one fixed
//!   payment on Base (the legacy operator ABI) and on SKALE Base (CREATE3).
//! - `generation`: which ABI `release` / `refundInEscrow` speak, for every
//!   escrow network other than Arc crossed with {that network's own escrow,
//!   the CREATE3 escrow}, read off the selectors actually sent.
//!
//! Calldata is stored as 32-byte words without a `0x` prefix: a prefixed
//! 64-hex-digit run is the shape the pre-commit hook guards against, and
//! nothing here is a secret -- it is the encoding of the fixed payment below.

use alloy::primitives::{address, Address};
use serde_json::{json, Value};

use super::{
    execute_authorize_flow, execute_refund_in_escrow_flow, execute_release_flow,
    parse_escrow_request, ParsedEscrowRequest,
};
use crate::network::Network;
use crate::payment_operator::addresses::{
    create3, escrow_for_network, token_collector_for_network, OperatorAddresses, ESCROW_NETWORKS,
};
use crate::payment_operator::errors::OperatorError;
use crate::payment_operator::test_rpc::{self, MockNode, Providers, SentTx};
use crate::types::SettleResponse;

const FIXTURE: &str =
    include_str!("../../../tests/fixtures/escrow/operator-calldata-snapshot.json");

const PAYER: Address = address!("1111111111111111111111111111111111111111");
const RECEIVER: Address = address!("2222222222222222222222222222222222222222");
const TOKEN: Address = address!("3333333333333333333333333333333333333333");
const SALT: u64 = 0x3039;
const NONCE: u64 = 0x4242;

/// The flows `settle_escrow` dispatches to, minus its feature-flag check, so
/// the test does not depend on (or race over) `ENABLE_PAYMENT_OPERATOR`.
pub(super) async fn settle(
    body: &Value,
    facilitator: &Providers,
) -> Result<SettleResponse, OperatorError> {
    match parse_escrow_request(&body.to_string())? {
        ParsedEscrowRequest::Authorize {
            network,
            payload,
            extra,
        } => execute_authorize_flow(network, &payload, &extra, facilitator).await,
        ParsedEscrowRequest::Release {
            network,
            lifecycle,
            extra,
        } => execute_release_flow(network, &lifecycle, &extra, facilitator).await,
        ParsedEscrowRequest::RefundInEscrow {
            network,
            lifecycle,
            extra,
        } => execute_refund_in_escrow_flow(network, &lifecycle, &extra, facilitator).await,
    }
}

fn word(n: u64) -> String {
    format!("0x{n:064x}")
}

pub(super) fn payment_info(operator: Address) -> Value {
    json!({
        "operator": operator,
        "receiver": RECEIVER,
        "token": TOKEN,
        "maxAmount": "1000000",
        "preApprovalExpiry": 1_900_000_000u64,
        "authorizationExpiry": 1_900_086_400u64,
        "refundExpiry": 1_902_678_400u64,
        "minFeeBps": 0,
        "maxFeeBps": 1300,
        "feeReceiver": operator,
        "salt": word(SALT),
    })
}

fn requirements(network: Network, escrow: Address, operator: Address, collector: Address) -> Value {
    json!({
        "scheme": "escrow",
        "network": network.to_caip2(),
        "extra": {
            "escrowAddress": escrow,
            "operatorAddress": operator,
            "tokenCollector": collector,
        }
    })
}

pub(super) fn authorize_body(
    network: Network,
    escrow: Address,
    operator: Address,
    collector: Address,
) -> Value {
    json!({
        "x402Version": 2,
        "scheme": "escrow",
        "payload": {
            "authorization": {
                "from": PAYER,
                "to": collector,
                "value": "1000000",
                "validAfter": "0",
                "validBefore": "1900000000",
                "nonce": word(NONCE),
            },
            "signature": "0xabcdef1234567890",
            "paymentInfo": payment_info(operator),
        },
        "paymentRequirements": requirements(network, escrow, operator, collector),
    })
}

pub(super) fn lifecycle_body(
    action: &str,
    amount: &str,
    network: Network,
    escrow: Address,
    operator: Address,
    collector: Address,
) -> Value {
    json!({
        "x402Version": 2,
        "scheme": "escrow",
        "action": action,
        "payload": {
            "paymentInfo": payment_info(operator),
            "payer": PAYER,
            "amount": amount,
        },
        "paymentRequirements": requirements(network, escrow, operator, collector),
    })
}

fn calldata_entry(network: Network, action: &str, sent: &SentTx, pinned: Address) -> Value {
    let (selector, args) = sent.input.split_at(4);
    let sender = if sent.from == pinned {
        "pinned"
    } else {
        "not-pinned"
    };
    json!({
        "network": network.to_string(),
        "action": action,
        "to": sent.to,
        "sender": sender,
        "selector": hex::encode(selector),
        "args": args.chunks(32).map(hex::encode).collect::<Vec<_>>(),
    })
}

fn abi_name(release: &str, refund: &str) -> &'static str {
    match (release, refund) {
        ("ecf39b0a", "e2b8996f") => "legacy",
        ("c602dd4a", "4e04eaa0") => "create3",
        _ => "unrecognised",
    }
}

/// Run one request and return the single transaction it produced.
async fn one_tx(node: &MockNode, providers: &Providers, body: Value) -> SentTx {
    node.clear_log();
    let result = settle(&body, providers).await;
    assert!(result.is_ok(), "{body}: {result:?}");
    let sent = node.sent();
    assert_eq!(sent.len(), 1, "exactly one transaction for {body}");
    sent.into_iter().next().unwrap()
}

async fn node_for(network: Network) -> (MockNode, Providers) {
    crate::writer_lease::set_writer_for_test(true);
    let node = MockNode::start(network).await;
    let eip1559 = !matches!(network, Network::SkaleBase);
    let provider = test_rpc::provider(network, &node, eip1559).await;
    (node, Providers::one(network, provider))
}

async fn observe_calldata() -> Value {
    let mut entries = Vec::new();
    for network in [Network::Base, Network::SkaleBase] {
        let (node, providers) = node_for(network).await;
        let pinned = test_rpc::evm(&providers, network).pinned_signer();
        let addrs = OperatorAddresses::for_network(network).unwrap();
        let operator = addrs.payment_operators[0];
        let (escrow, collector) = (addrs.escrow, addrs.token_collector);

        let sent = one_tx(
            &node,
            &providers,
            authorize_body(network, escrow, operator, collector),
        )
        .await;
        entries.push(calldata_entry(network, "authorize", &sent, pinned));
        for (action, amount) in [("release", "1000000"), ("refundInEscrow", "500000")] {
            let body = lifecycle_body(action, amount, network, escrow, operator, collector);
            let sent = one_tx(&node, &providers, body).await;
            entries.push(calldata_entry(network, action, &sent, pinned));
        }
    }
    Value::Array(entries)
}

/// Every escrow network this snapshot covers: all of them but Arc, which
/// joined the scheme after it was recorded.
fn snapshot_networks() -> Vec<Network> {
    ESCROW_NETWORKS
        .iter()
        .copied()
        .filter(|n| !matches!(n, Network::Arc | Network::ArcTestnet))
        .collect()
}

async fn observe_generation() -> Value {
    let mut rows = Vec::new();
    for network in snapshot_networks() {
        let (node, providers) = node_for(network).await;
        let operator = OperatorAddresses::for_network(network)
            .unwrap()
            .payment_operators[0];
        let pairs = [
            (
                escrow_for_network(network).unwrap(),
                token_collector_for_network(network).unwrap(),
            ),
            (create3::ESCROW, create3::TOKEN_COLLECTOR),
        ];
        for (escrow, collector) in pairs {
            let mut selectors = Vec::new();
            for (action, amount) in [("release", "1000000"), ("refundInEscrow", "500000")] {
                let body = lifecycle_body(action, amount, network, escrow, operator, collector);
                let sent = one_tx(&node, &providers, body).await;
                selectors.push(hex::encode(&sent.input[..4]));
            }
            rows.push(json!({
                "network": network.to_string(),
                "escrow": escrow,
                "tokenCollector": collector,
                "release": selectors[0],
                "refundInEscrow": selectors[1],
                "abi": abi_name(&selectors[0], &selectors[1]),
            }));
        }
    }
    Value::Array(rows)
}

/// Compare `observed` with the fixture's `key`. On a mismatch the whole
/// observed document is written next to the other temp files, for a diff.
fn assert_matches_fixture(key: &str, observed: Value) {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    if fixture.get(key) != Some(&observed) {
        let path = std::env::temp_dir().join(format!("escrow-snapshot-{key}.observed.json"));
        let doc = json!({ key: observed });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        panic!(
            "operator `{key}` differs from the recorded snapshot; observed written to {}",
            path.display()
        );
    }
}

#[tokio::test]
async fn release_refund_calldata_snapshot_base_and_skale() {
    let observed = observe_calldata().await;
    assert_matches_fixture("calldata", observed);
}

#[tokio::test]
async fn generation_is_unchanged_for_every_existing_escrow_network() {
    let networks = snapshot_networks();
    assert_eq!(
        networks.len(),
        11,
        "the snapshot covers the 11 pre-Arc networks"
    );
    let observed = observe_generation().await;
    assert_matches_fixture("generation", observed);
}
