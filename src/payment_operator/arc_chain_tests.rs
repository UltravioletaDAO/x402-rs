//! What Arc and Arc testnet answered about the canonical v1 escrow set.
//!
//! `tests/fixtures/escrow/arc-generation-d-chain.json` is recorded by
//! `scripts/record_arc_escrow_fixture.py` from the addresses
//! `addresses.rs` declares: JSON-RPC answers verbatim, nothing typed by hand.
//! These tests hold the code to what those answers say; the `#[ignore]` one
//! asks the chain again.

use std::collections::HashSet;

use alloy::primitives::{keccak256, Address, B256};
use alloy::sol_types::SolCall;
use serde_json::Value;

use super::abi::EscrowContract;
use super::addresses::{canonical_v1, create3, OperatorAddresses, ARC_EM_OPERATOR_CONFIG};
use super::autoverify::{self, Verdict};
use super::test_rpc::{self, CallAnswer, MockNode};
use super::types::ContractPaymentInfo;
use crate::chain::evm::EvmChain;
use crate::network::Network;
use crate::types::Scheme;

const FIXTURE: &str = include_str!("../../tests/fixtures/escrow/arc-generation-d-chain.json");

const ARC_NETWORKS: [Network; 2] = [Network::Arc, Network::ArcTestnet];

fn recorded(network: Network) -> Value {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    fixture["networks"][network.to_string()].clone()
}

fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).expect("hex without 0x")
}

/// The recorded `eth_getCode` of `address`, or `None` if it was not recorded.
fn recorded_code(rec: &Value, section: &str, address: Address) -> Option<Vec<u8>> {
    rec[section].as_object()?.iter().find_map(|(k, v)| {
        (k.parse::<Address>().ok() == Some(address)).then(|| unhex(v.as_str().unwrap()))
    })
}

/// Every address an Arc `escrow` / `commerce` entry announces, given which
/// operators passed their self-check.
fn announced(network: Network, verified: &HashSet<Address>) -> Vec<Address> {
    crate::facilitator_local::escrow_supported_kinds(|n, op| n == network && verified.contains(&op))
        .iter()
        .filter(|k| k.network == network.to_caip2())
        .flat_map(|k| {
            assert!(matches!(k.scheme, Scheme::Escrow | Scheme::Commerce));
            let e = k.extra.as_ref().unwrap().escrow.as_ref().unwrap();
            [
                e.escrow_address.0,
                e.operator_address.0,
                e.token_collector.0,
            ]
        })
        .collect()
}

/// Every address the `canonical_v1` module declares.
fn canonical_set() -> [Address; 5] {
    [
        canonical_v1::ESCROW,
        canonical_v1::TOKEN_COLLECTOR,
        canonical_v1::PROTOCOL_FEE_CONFIG,
        canonical_v1::FACTORY_PAYMENT_OPERATOR,
        canonical_v1::FACTORY_REFUND_REQUEST,
    ]
}

#[tokio::test]
async fn every_announced_arc_address_has_code() {
    for network in ARC_NETWORKS {
        let rec = recorded(network);
        let chain_id = EvmChain::try_from(network).unwrap().chain_id;
        assert_eq!(
            u64::from_str_radix(
                rec["chainId"].as_str().unwrap().trim_start_matches("0x"),
                16
            )
            .unwrap(),
            chain_id,
            "{network}: recorded from the right chain"
        );

        // The real self-check, against a node serving what the chain served.
        let node = MockNode::start(network).await;
        for (address, code) in rec["code"].as_object().unwrap() {
            node.set_code(address.parse().unwrap(), unhex(code.as_str().unwrap()));
        }
        for (address, answer) in rec["operatorEscrow"].as_object().unwrap() {
            node.on_call(
                address.parse().unwrap(),
                super::abi::OperatorV3Contract::ESCROWCall::SELECTOR,
                CallAnswer::Return(unhex(answer.as_str().unwrap())),
            );
        }
        let provider = test_rpc::provider(network, &node, true).await;
        let addrs = OperatorAddresses::for_network(network).unwrap();
        let mut verified = HashSet::new();
        for &operator in &addrs.payment_operators {
            let code = recorded_code(&rec, "code", operator)
                .unwrap_or_else(|| panic!("{network}: operator {operator} was not recorded"));
            let verdict = autoverify::check(&provider, operator, addrs.escrow).await;
            if code.is_empty() {
                // Not deployed when recorded: it must not be announced.
                assert!(matches!(verdict, Verdict::Mismatch(_)), "{verdict:?}");
            }
            if verdict == Verdict::Verified {
                verified.insert(operator);
            }
        }

        // What /supported would announce has code, every address of it.
        let announced = announced(network, &verified);
        assert!(!announced.is_empty());
        for address in announced {
            let code = recorded_code(&rec, "code", address)
                .unwrap_or_else(|| panic!("{network}: announced {address} was not recorded"));
            assert!(
                !code.is_empty(),
                "{network}: announced {address} has no code"
            );
        }

        // So does everything else declared for the network.
        for address in canonical_set() {
            let code = recorded_code(&rec, "code", address).unwrap();
            assert!(!code.is_empty(), "{network}: {address} has no code");
        }

        // The factory embeds the operator it deploys: its bytecode carries the
        // v3 selectors and none of the older generations' release /
        // refundInEscrow / FEE_RECIPIENT().
        let factory = recorded_code(&rec, "code", canonical_v1::FACTORY_PAYMENT_OPERATOR).unwrap();
        for (name, selector) in autoverify::v3_selectors() {
            assert!(
                autoverify::pushes_selector(&factory, selector),
                "{network}: {name}"
            );
        }
        for signature in [
            "release((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256),uint256)",
            "release((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256),uint256,bytes)",
            "refundInEscrow((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256),uint120)",
            "refundInEscrow((address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256),uint120,bytes)",
            "FEE_RECIPIENT()",
        ] {
            let selector: [u8; 4] = keccak256(signature)[..4].try_into().unwrap();
            assert!(!autoverify::pushes_selector(&factory, selector), "{network}: {signature}");
        }

        // And the CREATE3 set, which the code must never send to on Arc, has none.
        for address in [
            create3::ESCROW,
            create3::TOKEN_COLLECTOR,
            create3::FACTORY_PAYMENT_OPERATOR,
        ] {
            let code = recorded_code(&rec, "create3Code", address).unwrap();
            assert!(code.is_empty(), "{network}: CREATE3 {address} has code");
        }
    }
}

/// [`every_announced_arc_address_has_code`] against Arc mainnet itself,
/// through two independent RPCs. Reads are sequential, 1.4 s apart, and the
/// test stops at the first failure (a 429 included): the provider here has no
/// retry layer.
#[tokio::test]
#[ignore = "reads Arc mainnet through two public RPCs"]
async fn every_announced_arc_address_has_code_live() {
    use alloy::network::TransactionBuilder as _;
    use alloy::providers::{Provider, ProviderBuilder};
    use alloy::rpc::types::TransactionRequest;
    use std::time::Duration;

    let network = Network::Arc;
    let addrs = OperatorAddresses::for_network(network).unwrap();
    for rpc in ["https://rpc.mainnet.arc.io", "https://arc-mainnet.drpc.org"] {
        let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
        let pause = || tokio::time::sleep(Duration::from_millis(1400));

        assert_eq!(provider.get_chain_id().await.expect(rpc), 5042, "{rpc}");
        pause().await;

        let mut verified = HashSet::new();
        for &operator in &addrs.payment_operators {
            let code = provider.get_code_at(operator).await.expect(rpc);
            pause().await;
            let answer = if autoverify::code_passes(&code) {
                let tx = TransactionRequest::default()
                    .with_to(operator)
                    .with_input(super::abi::OperatorV3Contract::ESCROWCall {}.abi_encode());
                let raw = provider.call(tx).await.expect(rpc);
                pause().await;
                Some(raw)
            } else {
                None
            };
            let verdict =
                autoverify::judge(&code, answer.as_ref().map(|b| b.as_ref()), addrs.escrow);
            println!("{rpc}: operator {operator}: {verdict:?}");
            if verdict == Verdict::Verified {
                verified.insert(operator);
            }
        }

        let mut seen = HashSet::new();
        for address in announced(network, &verified)
            .into_iter()
            .chain(canonical_set())
        {
            if !seen.insert(address) {
                continue;
            }
            let code = provider.get_code_at(address).await.expect(rpc);
            pause().await;
            println!("{rpc}: {address}: {} bytes", code.len());
            assert!(!code.is_empty(), "{rpc}: {address} has no code");
        }
    }
}

#[test]
fn arc_operator_address_matches_recorded_compute_address() {
    // computeAddress(OperatorConfig) re-encoded from the config the code
    // declares: a static tuple of twelve addresses, one word each.
    let signature = format!("computeAddress(({}))", ["address"; 12].join(","));
    let mut calldata = keccak256(signature.as_bytes())[..4].to_vec();
    for address in ARC_EM_OPERATOR_CONFIG {
        calldata.extend_from_slice(address.into_word().as_slice());
    }

    for network in ARC_NETWORKS {
        let rec = recorded(network);
        let call = &rec["computeAddress"];
        let factory: Address = call["factory"].as_str().unwrap().parse().unwrap();
        assert_eq!(factory, canonical_v1::FACTORY_PAYMENT_OPERATOR, "{network}");
        assert_eq!(
            unhex(call["calldata"].as_str().unwrap()),
            calldata,
            "{network}: the recorded call is the one the declared config encodes to"
        );
        let result = unhex(call["result"].as_str().unwrap());
        assert_eq!(result.len(), 32);
        let computed = Address::from_word(B256::from_slice(&result));
        assert_eq!(
            OperatorAddresses::for_network(network)
                .unwrap()
                .payment_operators,
            vec![computed],
            "{network}: the declared operator is the factory's computeAddress"
        );
    }
}

/// The nonce `assert_eoa_authorization` recovers against is the escrow's own
/// `getHash` of the payment with the payer zeroed -- checked against what the
/// canonical escrow answered on each Arc network for the recorded probe.
#[test]
fn arc_payer_agnostic_nonce_matches_the_escrow() {
    for network in ARC_NETWORKS {
        let rec = recorded(network);
        let probe = &rec["getHashProbe"];
        let escrow: Address = probe["escrow"].as_str().unwrap().parse().unwrap();
        assert_eq!(escrow, canonical_v1::ESCROW);
        let call =
            EscrowContract::getHashCall::abi_decode(&unhex(probe["calldata"].as_str().unwrap()))
                .unwrap();
        let p = call.paymentInfo;
        assert_eq!(p.payer, Address::ZERO, "the probe is payer-agnostic");
        let info = ContractPaymentInfo {
            operator: p.operator,
            // Any payer: the nonce does not depend on it.
            payer: Address::repeat_byte(0x77),
            receiver: p.receiver,
            token: p.token,
            max_amount: p.maxAmount.to::<u128>(),
            pre_approval_expiry: p.preApprovalExpiry.to::<u64>(),
            authorization_expiry: p.authorizationExpiry.to::<u64>(),
            refund_expiry: p.refundExpiry.to::<u64>(),
            min_fee_bps: p.minFeeBps,
            max_fee_bps: p.maxFeeBps,
            fee_receiver: p.feeReceiver,
            salt: p.salt,
        };
        let chain_id = EvmChain::try_from(network).unwrap().chain_id;
        assert_eq!(
            super::operator::payer_agnostic_hash(chain_id, escrow, &info),
            B256::from_slice(&unhex(probe["result"].as_str().unwrap())),
            "{network}"
        );
    }
    // The type string is AuthCaptureEscrow's PAYMENT_INFO_TYPEHASH preimage
    // (0xae68ac7c...6591), the same as the lifecycle order's.
    let typehash = keccak256(super::operator::PAYMENT_INFO_TYPE);
    assert_eq!(typehash[..4], [0xae, 0x68, 0xac, 0x7c]);
    assert_eq!(typehash[30..], [0x65, 0x91]);
}
