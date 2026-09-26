//! Arc and Arc testnet on the canonical v1 escrow set.
//!
//! Every request goes through the same parse and flow functions `/settle`
//! uses, into an `EvmProvider` pointed at a node the test runs; what is
//! asserted is what that node received -- the raw transactions, and the reads
//! that came before them -- or that it received nothing at all.

use alloy::primitives::{address, Address, Bytes, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy::sol_types::{SolCall, SolValue};
use serde_json::{json, Value};

use super::snapshot_tests::{authorize_body, lifecycle_body, settle};
use super::{
    query_escrow_state_enabled, resolve_operator_abi, validate_addresses, verify_escrow_enabled,
    OperatorAbi,
};
use crate::chain::evm::EvmChain;
use crate::network::Network;
use crate::payment_operator::abi::{EscrowContract, OperatorContract, OperatorV3Contract};
use crate::payment_operator::addresses::{canonical_v1, create3, OperatorAddresses};
use crate::payment_operator::autoverify::{self, Verdict};
use crate::payment_operator::errors::OperatorError;
use crate::payment_operator::test_rpc::{self, CallAnswer, MockNode, Providers};
use crate::payment_operator::types::{ContractPaymentInfo, EscrowExtra};

const ONE: u128 = 1_000_000;

/// The Arc operator the code declares.
fn operator(network: Network) -> Address {
    OperatorAddresses::for_network(network)
        .unwrap()
        .payment_operators[0]
}

async fn arc(network: Network) -> (MockNode, Providers) {
    crate::writer_lease::set_writer_for_test(true);
    let node = MockNode::start(network).await;
    let provider = test_rpc::provider(network, &node, true).await;
    (node, Providers::one(network, provider))
}

fn release(network: Network, amount: u128) -> Value {
    lifecycle_body(
        "release",
        &amount.to_string(),
        network,
        canonical_v1::ESCROW,
        operator(network),
        canonical_v1::TOKEN_COLLECTOR,
    )
}

fn refund(network: Network, amount: u128) -> Value {
    lifecycle_body(
        "refundInEscrow",
        &amount.to_string(),
        network,
        canonical_v1::ESCROW,
        operator(network),
        canonical_v1::TOKEN_COLLECTOR,
    )
}

/// Script the escrow's answer for the payment the bodies above describe.
fn escrow_holds(node: &MockNode, capturable: u128) {
    node.on_call(
        canonical_v1::ESCROW,
        EscrowContract::getHashCall::SELECTOR,
        CallAnswer::Return(B256::repeat_byte(0xab).abi_encode()),
    );
    node.on_call(
        canonical_v1::ESCROW,
        EscrowContract::paymentStateCall::SELECTOR,
        CallAnswer::Return((true, U256::from(capturable), U256::ZERO).abi_encode()),
    );
}

/// Bytecode that pushes every v3 selector, and an `ESCROW()` naming `escrow`:
/// what a deployed v3 operator looks like to the self-check.
fn deploy_v3_operator(node: &MockNode, operator: Address, escrow: Address) {
    let mut code = Vec::new();
    for (_, selector) in autoverify::v3_selectors() {
        code.push(0x63);
        code.extend_from_slice(&selector);
    }
    node.set_code(operator, code);
    node.on_call(
        operator,
        OperatorV3Contract::ESCROWCall::SELECTOR,
        CallAnswer::Return(escrow.abi_encode()),
    );
}

/// The declared operator on `network`, deployed: v3 bytecode bound to the
/// canonical escrow. A release or a refund needs code where it is sent.
fn deployed(node: &MockNode, network: Network) {
    deploy_v3_operator(node, operator(network), canonical_v1::ESCROW);
}

/// What `/settle` answers for `err`: status, JSON body, `Retry-After`.
async fn http(err: &OperatorError) -> (u16, Value, Option<String>) {
    let (response, _) =
        crate::handlers::escrow_settle_typed_response(err).expect("a typed escrow answer");
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap(), retry_after)
}

// ---------------------------------------------------------------------------
// release -> capture, refundInEscrow -> void
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc_release_encodes_capture() {
    for network in [Network::Arc, Network::ArcTestnet] {
        let (node, providers) = arc(network).await;
        deployed(&node, network);
        super::forget_operator_code_for_test(network, operator(network));
        let pinned = test_rpc::evm(&providers, network).pinned_signer();

        settle(&release(network, ONE), &providers)
            .await
            .expect("release settles");

        let sent = node.sent();
        assert_eq!(sent.len(), 1);
        let tx = &sent[0];
        assert_eq!(tx.to, Some(operator(network)));
        assert_eq!(
            tx.from, pinned,
            "lifecycle writes leave from the pinned signer"
        );
        assert_eq!(tx.input[..4], [0xf1, 0x2b, 0x86, 0xf6], "capture selector");
        let call = OperatorV3Contract::captureCall::abi_decode(&tx.input).unwrap();
        assert_eq!(call.amount, U256::from(ONE));
        assert!(call.data.is_empty());
        assert_eq!(call.paymentInfo.operator, operator(network));
        assert_eq!(call.paymentInfo.salt, U256::from(0x3039u64));
        // Arc's documented minimum maxFeePerGas: the escrow write is priced by
        // the same floor as an `exact` settle on this chain.
        assert!(
            tx.max_fee_per_gas >= 20_000_000_000,
            "{}",
            tx.max_fee_per_gas
        );
        // Read first: the operator's code, and nothing else -- no self-check,
        // no state read on a release.
        assert_eq!(node.code_reads(), vec![operator(network)]);
        assert!(node.reads().is_empty(), "{:?}", node.reads());
    }
}

#[tokio::test]
async fn arc_refund_encodes_void() {
    for network in [Network::Arc, Network::ArcTestnet] {
        let (node, providers) = arc(network).await;
        escrow_holds(&node, ONE);
        deployed(&node, network);
        let pinned = test_rpc::evm(&providers, network).pinned_signer();

        // The request names the whole capturable amount: that is a void.
        for amount in [ONE] {
            node.clear_log();
            settle(&refund(network, amount), &providers)
                .await
                .expect("refund settles");
            let sent = node.sent();
            assert_eq!(sent.len(), 1);
            let tx = &sent[0];
            assert_eq!(tx.to, Some(operator(network)));
            assert_eq!(tx.from, pinned);
            assert_eq!(tx.input[..4], [0xc3, 0xc5, 0x09, 0x0e], "void selector");
            let call = OperatorV3Contract::voidCall::abi_decode(&tx.input).unwrap();
            assert!(call.data.is_empty());
            assert_eq!(
                call.paymentInfo.payer,
                address!("1111111111111111111111111111111111111111")
            );
            // The state is read from the canonical escrow before the void.
            assert_eq!(
                node.reads(),
                vec![
                    (canonical_v1::ESCROW, EscrowContract::getHashCall::SELECTOR),
                    (
                        canonical_v1::ESCROW,
                        EscrowContract::paymentStateCall::SELECTOR
                    ),
                ]
            );
        }
    }
}

#[tokio::test]
async fn arc_partial_refund_is_rejected_without_tx() {
    let (node, providers) = arc(Network::Arc).await;
    escrow_holds(&node, ONE);
    deployed(&node, Network::Arc);

    for amount in [ONE / 2, ONE + 1] {
        node.clear_log();
        let err = settle(&refund(Network::Arc, amount), &providers)
            .await
            .expect_err("a partial refund cannot be a void");
        assert!(
            matches!(err, OperatorError::PartialRefundUnsupported { requested, capturable }
                if requested == amount && capturable == ONE),
            "{err:?}"
        );
        assert!(node.sent().is_empty(), "no transaction");
        let (status, body, retry_after) = http(&err).await;
        assert_eq!(status, 422);
        assert!((400..500).contains(&status));
        assert_eq!(
            body["errorReason"],
            "partial_refund_unsupported_on_generation"
        );
        assert_eq!(body["retryable"], false);
        assert_eq!(retry_after, None);
    }
}

#[tokio::test]
async fn arc_zero_amount_refund_is_rejected_without_tx() {
    let (node, providers) = arc(Network::Arc).await;
    escrow_holds(&node, ONE);
    deployed(&node, Network::Arc);

    // 0 is a missing amount, never "all of it": voiding the whole
    // authorization takes the caller naming the capturable amount.
    let err = settle(&refund(Network::Arc, 0), &providers)
        .await
        .expect_err("an amount of 0 is not a void of everything");
    assert!(
        matches!(err, OperatorError::AmountRequired { capturable } if capturable == ONE),
        "{err:?}"
    );
    assert!(node.sent().is_empty(), "no transaction");
    let (status, body, retry_after) = http(&err).await;
    assert_eq!(status, 422);
    assert_eq!(body["errorReason"], "amount_required_on_generation");
    assert_eq!(body["retryable"], false);
    assert_eq!(retry_after, None);

    // Nothing capturable is answered first, whatever the amount.
    node.clear_log();
    escrow_holds(&node, 0);
    let err = settle(&refund(Network::Arc, 0), &providers)
        .await
        .expect_err("nothing left to void");
    assert!(matches!(err, OperatorError::NothingToVoid), "{err:?}");
    assert!(node.sent().is_empty());
}

#[tokio::test]
async fn arc_void_with_zero_capturable_is_not_labeled_partial() {
    let (node, providers) = arc(Network::Arc).await;
    escrow_holds(&node, 0);
    deployed(&node, Network::Arc);

    // The retry of a void that already went through: the amount it names is
    // no longer capturable, and that is not a partial refund.
    let err = settle(&refund(Network::Arc, ONE), &providers)
        .await
        .expect_err("nothing left to void");
    assert!(matches!(err, OperatorError::NothingToVoid), "{err:?}");
    assert!(node.sent().is_empty(), "no transaction");
    let (status, body, _) = http(&err).await;
    assert_eq!(status, 409);
    assert_eq!(body["errorReason"], "nothing_to_void");
    assert_ne!(
        body["errorReason"],
        "partial_refund_unsupported_on_generation"
    );
    assert_eq!(body["retryable"], false);
}

#[tokio::test]
async fn arc_transient_rpc_failure_on_refund_is_retryable_5xx() {
    let (node, providers) = arc(Network::Arc).await;
    escrow_holds(&node, ONE);
    deployed(&node, Network::Arc);

    // A rate limit, and a node error that is not one: neither is a verdict.
    for failure in [None, Some((-32603i64, "internal error"))] {
        node.clear_log();
        match failure {
            None => node.fail_reads(true),
            Some((code, message)) => {
                node.fail_reads(false);
                node.on_call(
                    canonical_v1::ESCROW,
                    EscrowContract::paymentStateCall::SELECTOR,
                    CallAnswer::Error {
                        code,
                        message: message.to_string(),
                    },
                );
            }
        }
        let err = settle(&refund(Network::Arc, ONE), &providers)
            .await
            .expect_err("the state could not be read");
        assert!(
            matches!(err, OperatorError::ChainReadUnavailable(_)),
            "{err:?}"
        );
        assert!(node.sent().is_empty(), "no transaction without the state");
        let (status, body, retry_after) = http(&err).await;
        assert!((500..600).contains(&status), "{status}");
        assert_eq!(body["retryable"], true);
        assert_eq!(body["errorReason"], "chain_read_unavailable");
        assert!(
            body.get("transaction").is_none(),
            "no hash: nothing was sent"
        );
        assert!(retry_after.is_some());
    }
}

#[tokio::test]
async fn arc_autoverify_failure_does_not_block_release_or_refund() {
    let network = Network::Arc;
    let (node, providers) = arc(network).await;
    escrow_holds(&node, ONE);
    let op = operator(network);
    // The operator exists; only the self-check's cached verdict says otherwise.
    deployed(&node, network);

    for verdict in [
        Verdict::Mismatch("no code at the operator address".into()),
        Verdict::Unreachable("rate limit exceeded".into()),
    ] {
        autoverify::set_for_test(network, op, Some(verdict.clone()));

        node.clear_log();
        settle(&release(network, ONE), &providers)
            .await
            .unwrap_or_else(|e| panic!("release under {verdict:?}: {e:?}"));
        assert_eq!(
            node.sent()[0].input[..4],
            OperatorV3Contract::captureCall::SELECTOR
        );

        node.clear_log();
        settle(&refund(network, ONE), &providers)
            .await
            .unwrap_or_else(|e| panic!("refund under {verdict:?}: {e:?}"));
        assert_eq!(
            node.sent()[0].input[..4],
            OperatorV3Contract::voidCall::SELECTOR
        );

        // ... and neither called the operator: the self-check (`ESCROW()`) is
        // not on their path.
        assert!(
            node.reads().iter().all(|(to, _)| *to != op),
            "{:?}",
            node.reads()
        );
    }
    autoverify::set_for_test(network, op, None);
}

// ---------------------------------------------------------------------------
// A v3 write goes to paymentInfo.operator, and that address has code
// ---------------------------------------------------------------------------

/// An operator a merchant brings: not declared, never self-checked.
const UNDECLARED: Address = address!("4444444444444444444444444444444444444444");

/// The three v3 writes, all to `op`.
fn v3_writes(
    network: Network,
    op: Address,
    payer: &PrivateKeySigner,
) -> [(&'static str, Value); 3] {
    let lifecycle = |action| {
        lifecycle_body(
            action,
            "1000000",
            network,
            canonical_v1::ESCROW,
            op,
            canonical_v1::TOKEN_COLLECTOR,
        )
    };
    [
        (
            "authorize",
            signed_authorize_to(network, op, network, payer, payer),
        ),
        ("release", lifecycle("release")),
        ("refundInEscrow", lifecycle("refundInEscrow")),
    ]
}

#[tokio::test]
async fn arc_v3_writes_require_operator_code() {
    let network = Network::Arc;
    let (node, providers) = arc(network).await;
    escrow_holds(&node, ONE);
    let payer = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x66)).unwrap();

    // The declared operator before it is deployed. A call to an address with
    // no code is mined as a no-op with a successful receipt: release and refund
    // to it must be refused, not reported as settled.
    let declared = operator(network);
    super::forget_operator_code_for_test(network, declared);
    for body in [release(network, ONE), refund(network, ONE)] {
        node.clear_log();
        let err = settle(&body, &providers).await.expect_err("no code");
        assert!(
            matches!(err, OperatorError::OperatorHasNoCode { operator, .. } if operator == declared),
            "{err:?}"
        );
        assert!(node.sent().is_empty(), "no transaction");
        let (status, answer, retry_after) = http(&err).await;
        assert_eq!(status, 422);
        assert_eq!(answer["errorReason"], "operator_has_no_code");
        assert_eq!(answer["retryable"], false);
        assert_eq!(retry_after, None);
    }

    // An operator the merchant brings, for all three writes.
    super::forget_operator_code_for_test(network, UNDECLARED);
    for (action, body) in v3_writes(network, UNDECLARED, &payer) {
        node.clear_log();
        let err = settle(&body, &providers).await.expect_err(action);
        assert!(
            matches!(err, OperatorError::OperatorHasNoCode { .. }),
            "{action}: {err:?}"
        );
        assert!(node.sent().is_empty(), "{action}: no transaction");
        assert_eq!(node.code_reads(), vec![UNDECLARED], "{action}");
    }

    // The node cannot say: retryable 5xx, nothing sent.
    node.fail_reads(true);
    for (action, body) in v3_writes(network, UNDECLARED, &payer) {
        node.clear_log();
        let err = settle(&body, &providers).await.expect_err(action);
        assert!(
            matches!(err, OperatorError::ChainReadUnavailable(_)),
            "{action}: {err:?}"
        );
        assert!(node.sent().is_empty(), "{action}: no transaction");
        let (status, answer, retry_after) = http(&err).await;
        assert!((500..600).contains(&status), "{action}: {status}");
        assert_eq!(answer["retryable"], true);
        assert!(retry_after.is_some());
    }
    node.fail_reads(false);

    // With code there, each write goes out to it.
    node.set_code(UNDECLARED, vec![0x60, 0x00]);
    for (action, body) in v3_writes(network, UNDECLARED, &payer) {
        node.clear_log();
        settle(&body, &providers)
            .await
            .unwrap_or_else(|e| panic!("{action}: {e:?}"));
        let sent = node.sent();
        assert_eq!(sent.len(), 1, "{action}");
        assert_eq!(sent[0].to, Some(UNDECLARED), "{action}");
    }
    // Code once seen is not read again.
    node.clear_log();
    settle(&release(network, ONE), &providers)
        .await
        .expect_err("the declared operator still has no code");
    settle(&v3_writes(network, UNDECLARED, &payer)[1].1, &providers)
        .await
        .expect("release to the deployed operator");
    assert_eq!(
        node.code_reads(),
        vec![declared],
        "only the one never seen with code"
    );
    super::forget_operator_code_for_test(network, UNDECLARED);
}

#[tokio::test]
async fn arc_v3_write_target_must_be_the_payment_operator() {
    let network = Network::ArcTestnet;
    let (node, providers) = arc(network).await;
    escrow_holds(&node, ONE);
    deployed(&node, network);
    // Even a contract: the escrow only takes calls from paymentInfo.operator.
    node.set_code(UNDECLARED, vec![0x60, 0x00]);
    let payer = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x77)).unwrap();
    let op = operator(network);

    for (action, body) in v3_writes(network, op, &payer) {
        for key in ["operatorAddress", "authorizeAddress"] {
            let mut body = body.clone();
            body["paymentRequirements"]["extra"][key] = json!(UNDECLARED);
            node.clear_log();
            let err = settle(&body, &providers).await.expect_err(action);
            assert!(
                matches!(err, OperatorError::OperatorMismatch { expected, actual }
                    if expected == op && actual == UNDECLARED),
                "{action} via {key}: {err:?}"
            );
            assert!(node.sent().is_empty(), "{action} via {key}: no transaction");
            assert!(
                node.reads().is_empty() && node.code_reads().is_empty(),
                "{action} via {key}: refused before any read"
            );
            let (status, answer, _) = http(&err).await;
            assert_eq!(status, 400);
            assert_eq!(answer["errorReason"], "operator_mismatch");
        }
    }
}

// ---------------------------------------------------------------------------
// /escrow/state
// ---------------------------------------------------------------------------

fn state_query(network: Network, escrow: Address, collector: Address) -> String {
    let body = lifecycle_body(
        "release",
        "0",
        network,
        escrow,
        operator(network),
        collector,
    );
    json!({
        "paymentInfo": body["payload"]["paymentInfo"],
        "payer": body["payload"]["payer"],
        "network": network.to_caip2(),
        "extra": body["paymentRequirements"]["extra"],
    })
    .to_string()
}

#[tokio::test]
async fn arc_escrow_state_query_uses_generation_d() {
    for network in [Network::Arc, Network::ArcTestnet] {
        let (node, providers) = arc(network).await;
        node.on_call(
            canonical_v1::ESCROW,
            EscrowContract::getHashCall::SELECTOR,
            CallAnswer::Return(B256::repeat_byte(0xcd).abi_encode()),
        );
        node.on_call(
            canonical_v1::ESCROW,
            EscrowContract::paymentStateCall::SELECTOR,
            CallAnswer::Return((true, U256::from(700_000u64), U256::from(300_000u64)).abi_encode()),
        );
        // The self-check says nothing here, and must not matter.
        autoverify::set_for_test(
            network,
            operator(network),
            Some(Verdict::Unreachable("x".into())),
        );

        let state = query_escrow_state_enabled(
            &state_query(network, canonical_v1::ESCROW, canonical_v1::TOKEN_COLLECTOR),
            &providers,
        )
        .await
        .expect("state of an Arc escrow payment");
        assert!(state.has_collected_payment);
        assert_eq!(state.capturable_amount, 700_000);
        assert_eq!(state.refundable_amount, 300_000);
        assert_eq!(state.payment_info_hash, format!("0x{}", "cd".repeat(32)));
        assert_eq!(state.network, network.to_caip2());
        assert_eq!(
            node.reads(),
            vec![
                (canonical_v1::ESCROW, EscrowContract::getHashCall::SELECTOR),
                (
                    canonical_v1::ESCROW,
                    EscrowContract::paymentStateCall::SELECTOR
                ),
            ],
            "read from the canonical escrow, nothing else"
        );

        // The CREATE3 escrow is refused before any read.
        node.clear_log();
        let err = query_escrow_state_enabled(
            &state_query(network, create3::ESCROW, create3::TOKEN_COLLECTOR),
            &providers,
        )
        .await
        .expect_err("CREATE3 is not deployed on Arc");
        assert!(
            matches!(err, OperatorError::PaymentInfoInvalid(_)),
            "{err:?}"
        );
        assert!(node.reads().is_empty());

        // A read that fails is retryable, never an answer about the query.
        node.fail_reads(true);
        let err = query_escrow_state_enabled(
            &state_query(network, canonical_v1::ESCROW, canonical_v1::TOKEN_COLLECTOR),
            &providers,
        )
        .await
        .expect_err("the node is rate-limiting");
        assert!(
            matches!(err, OperatorError::ChainReadUnavailable(_)),
            "{err:?}"
        );
        assert_eq!(err.http_answer().unwrap().status, 502);
        autoverify::set_for_test(network, operator(network), None);
    }
}

// ---------------------------------------------------------------------------
// Only the canonical v1 set on Arc
// ---------------------------------------------------------------------------

/// Addresses of another x402r deployment set. Never to be accepted on Arc;
/// named here and nowhere else in `src/`.
const GENERATION_C: [Address; 5] = [
    address!("F8211868187974a7Fb9d99b8fFB171AD70665Dc6"),
    address!("0308703621160b894cF045E555686d99ee8bd94E"),
    address!("7561DC178D9aD5bc5fb103C01f448A510d2A36D0"),
    address!("D8490609d2da0ee626b0e676941b225cbc1A8C08"),
    address!("15f36140bC1d444f917D306d0f5be223F55709B6"),
];

/// Every address of the CREATE3 module.
fn create3_set() -> Vec<Address> {
    vec![
        create3::ESCROW,
        create3::TOKEN_COLLECTOR,
        create3::PROTOCOL_FEE_CONFIG,
        create3::FACTORY_PAYMENT_OPERATOR,
        create3::FACTORY_REFUND_REQUEST,
        create3::FACTORY_REFUND_REQUEST_EVIDENCE,
        create3::FACTORY_ESCROW_PERIOD,
        create3::FACTORY_FREEZE,
        create3::FACTORY_STATIC_FEE_CALCULATOR,
        create3::FACTORY_STATIC_ADDRESS_CONDITION,
        create3::FACTORY_AND_CONDITION,
        create3::FACTORY_OR_CONDITION,
        create3::FACTORY_NOT_CONDITION,
        create3::FACTORY_RECORDER_COMBINATOR,
        create3::FACTORY_SIGNATURE_CONDITION,
        create3::USDC_TVL_LIMIT,
        create3::ARBITER_REGISTRY,
        create3::RECEIVER_REFUND_COLLECTOR,
        create3::CONDITION_PAYER,
        create3::CONDITION_RECEIVER,
        create3::CONDITION_ALWAYS_TRUE,
    ]
}

fn extra(escrow: Address, operator: Address, collector: Address) -> EscrowExtra {
    serde_json::from_value(json!({
        "escrowAddress": escrow,
        "operatorAddress": operator,
        "tokenCollector": collector,
    }))
    .unwrap()
}

#[tokio::test]
async fn arc_rejects_create3_and_generation_c_addresses() {
    let forbidden: Vec<Address> = create3_set().into_iter().chain(GENERATION_C).collect();

    for network in [Network::Arc, Network::ArcTestnet] {
        // Nothing the code declares for Arc is one of them.
        let addrs = OperatorAddresses::for_network(network).unwrap();
        let mut declared = vec![
            addrs.escrow,
            addrs.factory,
            addrs.token_collector,
            addrs.protocol_fee_config,
            addrs.refund_request,
        ];
        declared.extend(&addrs.payment_operators);
        for a in &declared {
            assert!(!forbidden.contains(a), "{network}: {a} is declared");
        }
        let announced = crate::facilitator_local::escrow_supported_kinds(|_, _| true);
        for kind in announced.iter().filter(|k| k.network == network.to_caip2()) {
            let info = kind.extra.as_ref().unwrap().escrow.as_ref().unwrap();
            for a in [
                info.escrow_address.0,
                info.operator_address.0,
                info.token_collector.0,
            ] {
                assert!(!forbidden.contains(&a), "{network}: {a} is announced");
            }
        }

        // And none of them is accepted as escrow or collector, alone or paired
        // with the canonical other half.
        let op = operator(network);
        for &bad in &forbidden {
            for x in [
                extra(bad, op, canonical_v1::TOKEN_COLLECTOR),
                extra(canonical_v1::ESCROW, op, bad),
                extra(bad, op, bad),
            ] {
                assert!(
                    validate_addresses(network, &x, &addrs, false).is_err(),
                    "{network}: {bad}"
                );
                assert!(
                    resolve_operator_abi(network, &x).is_err(),
                    "{network}: {bad}"
                );
            }
        }
        assert!(validate_addresses(
            network,
            &extra(create3::ESCROW, op, create3::TOKEN_COLLECTOR),
            &addrs,
            false
        )
        .is_err());
        assert_eq!(
            resolve_operator_abi(
                network,
                &extra(canonical_v1::ESCROW, op, canonical_v1::TOKEN_COLLECTOR)
            )
            .unwrap(),
            OperatorAbi::V3
        );

        // End to end: a release naming the CREATE3 escrow sends nothing.
        let (node, providers) = arc(network).await;
        let body = lifecycle_body(
            "release",
            "1000000",
            network,
            create3::ESCROW,
            op,
            create3::TOKEN_COLLECTOR,
        );
        let err = settle(&body, &providers).await.expect_err("refused");
        assert!(
            matches!(err, OperatorError::PaymentInfoInvalid(_)),
            "{err:?}"
        );
        assert!(node.sent().is_empty() && node.reads().is_empty());
    }
}

// ---------------------------------------------------------------------------
// authorize: the payer's EOA signature, and a verified operator
// ---------------------------------------------------------------------------

/// An authorization of Arc USDC on `network` by `payer`, signed by `signer`
/// under the domain of `signed_for` (normally the network it is sent to).
fn signed_authorize(
    network: Network,
    signed_for: Network,
    payer: &PrivateKeySigner,
    signer: &PrivateKeySigner,
) -> Value {
    signed_authorize_to(network, operator(network), signed_for, payer, signer)
}

/// [`signed_authorize`] against the operator `op`.
fn signed_authorize_to(
    network: Network,
    op: Address,
    signed_for: Network,
    payer: &PrivateKeySigner,
    signer: &PrivateKeySigner,
) -> Value {
    let mut body = authorize_body(
        network,
        canonical_v1::ESCROW,
        op,
        canonical_v1::TOKEN_COLLECTOR,
    );
    let usdc = crate::network::USDCDeployment::by_network(network)
        .unwrap()
        .address();
    body["payload"]["paymentInfo"]["token"] = json!(usdc.to_string());
    body["payload"]["authorization"]["from"] = json!(payer.address());
    let payload: crate::payment_operator::types::EscrowPayload =
        serde_json::from_value(body["payload"].clone()).unwrap();
    let info = ContractPaymentInfo::from_escrow_payload(&payload);
    let digest = receive_with_authorization_digest(signed_for, &info);
    let signature = signer.sign_hash_sync(&digest).unwrap();
    body["payload"]["signature"] = json!(Bytes::from(signature.as_bytes().to_vec()));
    body
}

/// What the canonical collector verifies, computed from the chain side's
/// definitions: `find_known_eip712_metadata` for the domain and
/// `payer_agnostic_hash` for the nonce (checked against the escrow's own
/// `getHash` in `arc_chain_tests`).
fn receive_with_authorization_digest(network: Network, info: &ContractPaymentInfo) -> B256 {
    use alloy::sol_types::{eip712_domain, SolStruct};
    let chain_id = EvmChain::try_from(network).unwrap().chain_id;
    let (name, version) =
        crate::chain::evm::find_known_eip712_metadata(network, &info.token).expect("known token");
    let domain = eip712_domain! {
        name: name,
        version: version,
        chain_id: chain_id,
        verifying_contract: info.token,
    };
    super::ReceiveWithAuthorization {
        from: info.payer,
        to: canonical_v1::TOKEN_COLLECTOR,
        value: U256::from(info.max_amount),
        validAfter: U256::ZERO,
        validBefore: U256::from(info.pre_approval_expiry),
        nonce: super::payer_agnostic_hash(chain_id, canonical_v1::ESCROW, info),
    }
    .eip712_signing_hash(&domain)
}

#[tokio::test]
async fn arc_authorize_requires_the_payers_eoa_signature() {
    let network = Network::Arc;
    let (node, providers) = arc(network).await;
    deploy_v3_operator(&node, operator(network), canonical_v1::ESCROW);
    autoverify::set_for_test(network, operator(network), None);
    let payer = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x33)).unwrap();
    let stranger = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x44)).unwrap();

    let good = signed_authorize(network, network, &payer, &payer);
    settle(&good, &providers)
        .await
        .expect("an EOA authorization by the payer");
    let sent = node.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].input[..4],
        OperatorContract::authorizeCall::SELECTOR
    );
    assert_eq!(sent[0].to, Some(operator(network)));

    // Signed for the other Arc network, signed by somebody else under the
    // payer's name, and an envelope that is not a 65-byte EOA signature.
    let other_network = signed_authorize(network, Network::ArcTestnet, &payer, &payer);
    let someone_else = signed_authorize(network, network, &payer, &stranger);
    let mut wrapped = good.clone();
    wrapped["payload"]["signature"] = json!("0xabcdef1234567890");

    for bad in [other_network, someone_else, wrapped] {
        node.clear_log();
        let err = settle(&bad, &providers)
            .await
            .expect_err("not the payer's EOA signature");
        assert!(
            matches!(err, OperatorError::AuthorizationSignatureInvalid(_)),
            "{err:?}"
        );
        assert!(node.sent().is_empty());
        assert_eq!(err.http_answer().unwrap().status, 400);
    }
    autoverify::set_for_test(network, operator(network), None);
}

#[tokio::test]
async fn arc_authorize_waits_for_a_verified_operator() {
    let network = Network::ArcTestnet;
    let (node, providers) = arc(network).await;
    let op = operator(network);
    autoverify::set_for_test(network, op, None);
    let payer = PrivateKeySigner::from_bytes(&B256::repeat_byte(0x55)).unwrap();
    let body = signed_authorize(network, network, &payer, &payer);

    // Not deployed: no code. Refused, retryable, nothing sent.
    let err = settle(&body, &providers)
        .await
        .expect_err("operator not deployed");
    assert!(
        matches!(err, OperatorError::OperatorNotVerified { .. }),
        "{err:?}"
    );
    let (status, answer, retry_after) = http(&err).await;
    assert_eq!(status, 503);
    assert_eq!(answer["errorReason"], "operator_not_verified");
    assert_eq!(answer["retryable"], true);
    assert!(retry_after.is_some());
    assert!(node.sent().is_empty());
    // /verify says the same, as a verdict.
    let verdict = verify_escrow_enabled(&body.to_string(), &providers)
        .await
        .unwrap();
    assert_eq!(verdict["isValid"], false, "{verdict}");
    assert!(
        verdict["invalidReason"]
            .as_str()
            .unwrap()
            .contains("not verified"),
        "{verdict}"
    );

    // Deployed a moment later: inside RECHECK_FLOOR the standing verdict is
    // not re-read, so a burst of requests costs no reads at all.
    deploy_v3_operator(&node, op, canonical_v1::ESCROW);
    node.clear_log();
    let err = settle(&body, &providers)
        .await
        .expect_err("within the re-check floor");
    assert!(
        matches!(err, OperatorError::OperatorNotVerified { .. }),
        "{err:?}"
    );
    assert!(node.reads().is_empty() && node.code_reads().is_empty());
    assert!(node.sent().is_empty());
    node.set_code(op, Vec::new());

    // The node cannot be read: retryable 5xx, nothing sent.
    autoverify::set_for_test(network, op, None);
    node.fail_reads(true);
    let err = settle(&body, &providers)
        .await
        .expect_err("cannot check the operator");
    assert!(
        matches!(err, OperatorError::ChainReadUnavailable(_)),
        "{err:?}"
    );
    assert!((500..600).contains(&http(&err).await.0));
    assert!(node.sent().is_empty());
    node.fail_reads(false);

    // Bound to another escrow: still refused.
    autoverify::set_for_test(network, op, None);
    deploy_v3_operator(&node, op, create3::ESCROW);
    let err = settle(&body, &providers)
        .await
        .expect_err("bound elsewhere");
    assert!(
        matches!(err, OperatorError::OperatorNotVerified { .. }),
        "{err:?}"
    );

    // Bound to the canonical escrow but without one of the v3 entry points:
    // refused on the bytecode alone, without asking it for ESCROW().
    autoverify::set_for_test(network, op, None);
    deploy_v3_operator(&node, op, canonical_v1::ESCROW);
    let mut no_capture = Vec::new();
    for (name, selector) in autoverify::v3_selectors() {
        if name != "capture" {
            no_capture.push(0x63);
            no_capture.extend_from_slice(&selector);
        }
    }
    node.set_code(op, no_capture);
    node.clear_log();
    let err = settle(&body, &providers)
        .await
        .expect_err("no capture() in the bytecode");
    assert!(
        matches!(&err, OperatorError::OperatorNotVerified { reason, .. } if reason.contains("capture")),
        "{err:?}"
    );
    assert!(
        !node
            .reads()
            .contains(&(op, OperatorV3Contract::ESCROWCall::SELECTOR)),
        "{:?}",
        node.reads()
    );
    assert!(node.sent().is_empty());

    // Deployed as declared: verified on the next read, and authorize goes out.
    autoverify::set_for_test(network, op, None);
    deploy_v3_operator(&node, op, canonical_v1::ESCROW);
    node.clear_log();
    settle(&body, &providers).await.expect("verified operator");
    assert_eq!(autoverify::status(network, op), Some(Verdict::Verified));
    assert_eq!(node.sent().len(), 1);
    assert_eq!(node.sent()[0].to, Some(op));
    autoverify::set_for_test(network, op, None);
}
