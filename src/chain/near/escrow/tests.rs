//! Tests of the NEAR escrow scheme.
//!
//! Nothing here reaches a network: the outcomes are the recorded mainnet
//! settlement's envelope with receipts built to the shape NEP-141 and NEP-366
//! produce, and the node is a local JSON-RPC stub.
//!
//! W-13 (an authorization the escrow refuses, E06, with Circle's USDC) is the
//! real outcome the escrow repository's near-workspaces test recorded
//! (`tests/fixtures/near/escrow/w13_rechazo_e06_usdc_real.json`).

use super::*;
use crate::facilitator::Facilitator;
use crate::nonce_store::MemoryNonceStore;
use axum::{routing::post, Json, Router};
use near_crypto::{InMemorySigner, KeyType, SecretKey, Signer};
use near_primitives::action::delegate::NonDelegateAction;
use near_primitives::transaction::TransferAction;
use near_primitives::views::{BlockHeaderView, BlockView};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

const USDC_TESTNET: &str = "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af";
const USDC_MAINNET: &str = "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1";
const PAYER: &str = "payer-demo.testnet";
const WORKER: &str = "worker-demo.testnet";
const CAPTURER: &str = "em-capturer-v1b.arras-dev.testnet";
const AGENT: &str = "agent-1.testnet";
const CONTRACT: &str = "arras-v1b-1101.testnet";
const CONTRACT_V2: &str = "v2.arras-dev.testnet";
const RELAYER: &str = "facilitator.testnet";
const AMOUNT: u128 = 1_000_000;
const DELEGATE_NONCE: u64 = 42;
/// The tests' clock, Unix seconds: 2026-11-01T00:00:00Z, a day before V-1's
/// `release_deadline` and two before its `reclaim_after`.
const NOW: u64 = 1_793_491_200;

/// Section 7 of the escrow prescription, recomputed there byte for byte.
const V1_PREIMAGE: &str = "0102000000656d40000000336532323130653131383462343562363463386134333463306137653762323363633034656137656237613663336333323532306430336434616663623861661200000070617965722d64656d6f2e746573746e657413000000776f726b65722d64656d6f2e746573746e657440420f000000000000000000000000000000190d5311e4180000689ee75fe4181405000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
const V1_SHA256: &str = "b7b2b0e5d5108ea9a57677a8c6a8db75f5a3a831dd79c8aa64fbaf5b1c8573b2";
const V1_ID: &str = "DN5eMA3sNES7xsoy6qvy24cVF6Dmwjxpkvd2b4e8P38M";
const V2_SHA256: &str = "068f284abb07df3b822f07b5ea6463ccfbcbc79cb505350482aa684b4df90be8";
const V2_ID: &str = "Sc4D4Jn3ErWE68FckdDTLzThk1FTR1ZfLzuBUE4EeRm";
const V3_SHA256: &str = "1bda3698bc2574284b8a54b4078007aff35bf04514e8c66eae4e5c96f49ca359";
const V3_ID: &str = "2sizp6qpQdKz3DPmd1kX2ovobag9LfXerQKmEPQV3mb6";

/// The recorded mainnet settlement: its transaction envelope carries the
/// built outcomes below.
const RECORDED: &str = include_str!(
    "../../../../tests/fixtures/near/BY1QXFFLoojaNGr4bzqSW6Cmu1wc2G7sPLcHB6wXsSqM.json"
);
const TX_HASH: &str = "BY1QXFFLoojaNGr4bzqSW6Cmu1wc2G7sPLcHB6wXsSqM";
/// The Delegate receipt the recorded transaction converted to.
const DELEGATE_RECEIPT: &str = "5H2kQzxU6Vhq5sAfF2ZgHhZG2jANh21nDdvqTxa9eVyG";
/// W-13: the escrow's sandbox, an authorization of 10000001 over the
/// per-payment cap (E06) against a clone of Circle's testnet USDC, as
/// `EXPERIMENTAL_tx_status` returned it. The sandbox signed `ft_transfer_call`
/// as the payer; see [`w13_relayed`] for the Delegate the relayer adds.
const W13: &str =
    include_str!("../../../../tests/fixtures/near/escrow/w13_rechazo_e06_usdc_real.json");

fn v1_msg() -> String {
    format!(
        r#"{{"v":1,"action":"authorize","operator_id":"em","receiver":"worker-demo.testnet","release_deadline":"1793577600000000000","reclaim_after":"1793664000000000000","fee_bps_max":1300,"terms_hash":"{}","salt":"{}"}}"#,
        "0".repeat(64),
        "0".repeat(32)
    )
}

fn terms_v1() -> Terms {
    Terms {
        operator_id: "em".to_string(),
        receiver: WORKER.parse().unwrap(),
        release_deadline: 1_793_577_600_000_000_000,
        reclaim_after: 1_793_664_000_000_000_000,
        fee_bps_max: 1300,
        terms_hash: [0; 32],
        salt: [0; 16],
    }
}

fn account(s: &str) -> AccountId {
    s.parse().unwrap()
}

fn b64(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

fn config_from(pairs: &[(&str, &str)]) -> NearEscrowConfig {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    NearEscrowConfig::from_lookup(|name| map.get(name).cloned())
}

#[test]
fn the_defaults_leave_the_scheme_off() {
    let config = config_from(&[]);
    assert_eq!(config, NearEscrowConfig::default());
    assert!(!config.enabled);
    assert!(config.contracts_mainnet.is_empty() && config.contracts_testnet.is_empty());
    assert_eq!(config.min_gas, Gas::from_teragas(38));
    assert_eq!(config.max_gas, Gas::from_teragas(100));
    assert_eq!(config.sender_per_hour, 30);
    assert_eq!(config.operator_ids, vec!["em", "kk"]);
    assert!(!config.serves(Network::Near) && !config.serves(Network::NearTestnet));
}

#[test]
fn every_variable_is_read() {
    let config = config_from(&[
        (ENV_ENABLE_NEAR_ESCROW, "true"),
        (
            ENV_CONTRACTS_MAINNET,
            " v1.arras.ultravioletadao.near , v2.arras.ultravioletadao.near ",
        ),
        (ENV_CONTRACTS_TESTNET, "arras-v1b-1101.testnet"),
        (ENV_MIN_GAS_TGAS, "40"),
        (ENV_MAX_GAS_TGAS, "120"),
        (ENV_SENDER_PER_HOUR, "12"),
        (ENV_OPERATOR_IDS, "em, kk ,ops_2"),
    ]);
    assert!(config.enabled);
    assert_eq!(
        config.contracts(Network::Near),
        &[
            account("v1.arras.ultravioletadao.near"),
            account("v2.arras.ultravioletadao.near")
        ]
    );
    assert_eq!(config.contracts(Network::NearTestnet), &[account(CONTRACT)]);
    assert!(config.contracts(Network::Base).is_empty());
    assert_eq!(config.min_gas, Gas::from_teragas(40));
    assert_eq!(config.max_gas, Gas::from_teragas(120));
    assert_eq!(config.sender_per_hour, 12);
    assert_eq!(config.operator_ids, vec!["em", "kk", "ops_2"]);
}

#[test]
fn the_switch_is_on_only_for_true_or_1() {
    for on in ["true", "TRUE", " True ", "1"] {
        assert!(
            config_from(&[(ENV_ENABLE_NEAR_ESCROW, on)]).enabled,
            "{on:?}"
        );
    }
    for off in ["false", "0", "", "yes", "on", "2"] {
        assert!(
            !config_from(&[(ENV_ENABLE_NEAR_ESCROW, off)]).enabled,
            "{off:?}"
        );
    }
}

#[test]
fn a_value_out_of_range_falls_back_to_its_default() {
    let defaults = NearEscrowConfig::default();
    for (variable, value) in [
        (ENV_MIN_GAS_TGAS, "0"),
        (ENV_MIN_GAS_TGAS, "301"),
        (ENV_MIN_GAS_TGAS, "thirty"),
        (ENV_MAX_GAS_TGAS, "-5"),
        (ENV_MAX_GAS_TGAS, "1e2"),
        (ENV_SENDER_PER_HOUR, "0"),
        (ENV_SENDER_PER_HOUR, "3601"),
        (ENV_OPERATOR_IDS, "EM"),
        (ENV_OPERATOR_IDS, "em,this-operator-id-is-way-past-32-chars"),
        (ENV_OPERATOR_IDS, " , "),
    ] {
        let config = config_from(&[(variable, value)]);
        assert_eq!(config, defaults, "{variable}={value:?}");
    }
    // The ends of each range are in it.
    let config = config_from(&[
        (ENV_MIN_GAS_TGAS, "1"),
        (ENV_MAX_GAS_TGAS, "300"),
        (ENV_SENDER_PER_HOUR, "3600"),
    ]);
    assert_eq!(config.min_gas, Gas::from_teragas(1));
    assert_eq!(config.max_gas, Gas::from_teragas(300));
    assert_eq!(config.sender_per_hour, 3600);
}

#[test]
fn one_bad_contract_discards_the_list() {
    let config = config_from(&[(
        ENV_CONTRACTS_TESTNET,
        "arras-v1b-1101.testnet,Not An Account",
    )]);
    assert!(config.contracts_testnet.is_empty());
    let config = config_from(&[(
        ENV_CONTRACTS_TESTNET,
        "arras-v1b-1101.testnet,,arras-v1b-1101.testnet",
    )]);
    assert_eq!(config.contracts_testnet, vec![account(CONTRACT)]);
}

#[test]
fn an_inverted_gas_range_takes_both_defaults() {
    let config = config_from(&[(ENV_MIN_GAS_TGAS, "150"), (ENV_MAX_GAS_TGAS, "120")]);
    assert_eq!(config.min_gas, Gas::from_teragas(38));
    assert_eq!(config.max_gas, Gas::from_teragas(100));
}

#[test]
fn the_scheme_serves_a_network_only_on_and_with_a_contract_there() {
    let on = config_from(&[
        (ENV_ENABLE_NEAR_ESCROW, "true"),
        (ENV_CONTRACTS_TESTNET, CONTRACT),
    ]);
    assert!(on.serves(Network::NearTestnet));
    assert!(!on.serves(Network::Near));
    let off = config_from(&[(ENV_CONTRACTS_TESTNET, CONTRACT)]);
    assert!(!off.serves(Network::NearTestnet));
}

// ---------------------------------------------------------------------------
// payment_id and msg
// ---------------------------------------------------------------------------

#[test]
fn payment_id_vectors_match_byte_for_byte() {
    // V-1
    let v1 = terms_v1();
    let preimage = payment_id_preimage(&v1, USDC_TESTNET, PAYER, AMOUNT);
    assert_eq!(preimage.len(), 202);
    assert_eq!(hex::encode(&preimage), V1_PREIMAGE);
    assert_eq!(hex::encode(Sha256::digest(&preimage)), V1_SHA256);
    assert_eq!(compute_payment_id(&v1, USDC_TESTNET, PAYER, AMOUNT), V1_ID);
    assert_eq!(V1_ID.len(), 44);

    // V-2: the last salt byte, and a 43-character id.
    let mut v2 = terms_v1();
    v2.salt[15] = 1;
    let preimage = payment_id_preimage(&v2, USDC_TESTNET, PAYER, AMOUNT);
    assert_eq!(hex::encode(Sha256::digest(&preimage)), V2_SHA256);
    let id = compute_payment_id(&v2, USDC_TESTNET, PAYER, AMOUNT);
    assert_eq!(id, V2_ID);
    assert_eq!(id.len(), 43);

    // V-3: mainnet, sha256("em-task-0001") as terms hash.
    let v3 = Terms {
        operator_id: "em".to_string(),
        receiver: account("robot-k1.near"),
        release_deadline: 1_793_577_600_000_000_000,
        reclaim_after: 1_793_664_000_000_000_000,
        fee_bps_max: 1800,
        terms_hash: Sha256::digest(b"em-task-0001").into(),
        salt: [0xab; 16],
    };
    assert_eq!(
        hex::encode(v3.terms_hash),
        "134931ea6d298d41174d2200087b1c4a38c81987f1af576b4601921af65808d8"
    );
    let preimage = payment_id_preimage(&v3, USDC_MAINNET, "payer-demo.near", 10_000_000);
    assert_eq!(preimage.len(), 193);
    assert_eq!(hex::encode(Sha256::digest(&preimage)), V3_SHA256);
    assert_eq!(
        compute_payment_id(&v3, USDC_MAINNET, "payer-demo.near", 10_000_000),
        V3_ID
    );
}

#[test]
fn the_v1_msg_is_the_v1_terms() {
    let terms = parse_authorize_msg(&v1_msg()).unwrap();
    assert_eq!(terms, terms_v1());
    assert_eq!(
        compute_payment_id(&terms, USDC_TESTNET, PAYER, AMOUNT),
        V1_ID
    );
}

#[test]
fn a_msg_that_is_not_the_canonical_text_is_refused() {
    let msg = v1_msg();
    for variant in [
        msg.replacen(",\"action\"", ", \"action\"", 1),
        format!(" {msg}"),
        format!("{msg}\n"),
        msg.replacen(
            "{\"v\":1,\"action\":\"authorize\",",
            "{\"action\":\"authorize\",\"v\":1,",
            1,
        ),
    ] {
        assert_eq!(
            parse_authorize_msg(&variant),
            Err(EscrowRejectReason::MsgNotCanonical),
            "{variant}"
        );
    }
}

#[test]
fn a_msg_that_is_not_an_authorization_is_refused() {
    let msg = v1_msg();
    for variant in [
        msg.replacen("\"v\":1", "\"v\":2", 1),
        msg.replacen("\"authorize\"", "\"budget\"", 1),
        msg.replacen("\"em\"", "\"EM\"", 1),
        msg.replacen("\"worker-demo.testnet\"", "\"Worker Demo\"", 1),
        msg.replacen("\"1793577600000000000\"", "\"01793577600000000000\"", 1),
        msg.replacen("\"1793664000000000000\"", "1793664000000000000", 1),
        msg.replacen(&"0".repeat(64), &"A".repeat(64), 1),
        msg.replacen(&"0".repeat(64), &"0".repeat(62), 1),
        msg.replacen(
            &format!("\"{}\"}}", "0".repeat(32)),
            &format!("\"{}\"}}", "0".repeat(30)),
            1,
        ),
        msg.replacen("\"fee_bps_max\":1300", "\"fee_bps_max\":70000", 1),
        msg.replacen("}", ",\"operator_ticket\":null}", 1),
        msg.replacen("\"v\":1,", "\"v\":1,\"v\":1,", 1),
        "not json".to_string(),
    ] {
        assert_eq!(
            parse_authorize_msg(&variant),
            Err(EscrowRejectReason::MsgInvalid),
            "{variant}"
        );
    }
}

// ---------------------------------------------------------------------------
// Delegate actions and requests
// ---------------------------------------------------------------------------

fn cfg() -> NearEscrowConfig {
    NearEscrowConfig {
        enabled: true,
        contracts_testnet: vec![account(CONTRACT), account(CONTRACT_V2)],
        ..NearEscrowConfig::default()
    }
}

fn extra() -> NearEscrowExtra {
    NearEscrowExtra {
        contract: Some(CONTRACT.to_string()),
        operator_id: Some("em".to_string()),
        msg: Some(v1_msg()),
        payment_id: Some(V1_ID.to_string()),
        action: None,
    }
}

fn function_call(
    method: &str,
    args: Vec<u8>,
    gas_tgas: u64,
    deposit_yocto: u128,
) -> FunctionCallAction {
    FunctionCallAction {
        method_name: method.to_string(),
        args,
        gas: Gas::from_teragas(gas_tgas),
        deposit: NearToken::from_yoctonear(deposit_yocto),
    }
}

/// The V-1 authorization: 1 USDC to the escrow, 100 TGas, 1 yocto.
fn authorize_call() -> FunctionCallAction {
    let args = json!({"receiver_id": CONTRACT, "amount": "1000000", "msg": v1_msg()});
    function_call("ft_transfer_call", args.to_string().into_bytes(), 100, 1)
}

fn delegate(sender: &str, receiver: &str, actions: Vec<Action>, key: &SecretKey) -> DelegateAction {
    DelegateAction {
        sender_id: account(sender),
        receiver_id: account(receiver),
        actions: actions
            .into_iter()
            .map(|a| NonDelegateAction::try_from(a).unwrap())
            .collect(),
        nonce: DELEGATE_NONCE,
        max_block_height: 1_000_000,
        public_key: key.public_key(),
    }
}

fn unsigned(sender: &str, receiver: &str, calls: Vec<FunctionCallAction>) -> DelegateAction {
    let key = SecretKey::from_random(KeyType::ED25519);
    let actions = calls
        .into_iter()
        .map(|c| Action::FunctionCall(Box::new(c)))
        .collect();
    delegate(sender, receiver, actions, &key)
}

fn sign(sender: &str, receiver: &str, calls: Vec<FunctionCallAction>) -> SignedDelegateAction {
    let key = SecretKey::from_random(KeyType::ED25519);
    let actions = calls
        .into_iter()
        .map(|c| Action::FunctionCall(Box::new(c)))
        .collect();
    let delegate_action = delegate(sender, receiver, actions, &key);
    let signer: Signer = InMemorySigner::from_secret_key(account(sender), key);
    let signature = signer.sign(delegate_action.get_nep461_hash().as_bytes());
    SignedDelegateAction {
        delegate_action,
        signature,
    }
}

fn request(signed: Option<&SignedDelegateAction>) -> EscrowRequest {
    EscrowRequest {
        network: Network::NearTestnet,
        signed_delegate_action: signed
            .map(|s| b64(&borsh::to_vec(s).unwrap()))
            .unwrap_or_default(),
        asset: Some(USDC_TESTNET.to_string()),
        amount: Some(AMOUNT),
        extra: extra(),
    }
}

fn authorization(edit: impl FnOnce(&mut FunctionCallAction)) -> DelegateAction {
    let mut call = authorize_call();
    edit(&mut call);
    unsigned(PAYER, USDC_TESTNET, vec![call])
}

fn validate(da: &DelegateAction) -> Result<EscrowAuthorize, EscrowRejectReason> {
    validate_escrow_delegate_actions(da, &request(None), &cfg(), NOW)
}

fn with_args(call: &mut FunctionCallAction, edit: impl FnOnce(&mut Value)) {
    let mut args: Value = serde_json::from_slice(&call.args).unwrap();
    edit(&mut args);
    call.args = args.to_string().into_bytes();
}

#[test]
fn the_v1_authorization_passes() {
    assert_eq!(
        validate(&authorization(|_| {})),
        Ok(EscrowAuthorize {
            contract: account(CONTRACT),
            token: account(USDC_TESTNET),
            payer: account(PAYER),
            amount: AMOUNT,
            payment_id: V1_ID.to_string(),
        })
    );
}

#[test]
fn two_actions_are_refused() {
    let da = unsigned(
        PAYER,
        USDC_TESTNET,
        vec![authorize_call(), authorize_call()],
    );
    assert_eq!(validate(&da), Err(EscrowRejectReason::ActionsNotOne));
    let none = unsigned(PAYER, USDC_TESTNET, vec![]);
    assert_eq!(validate(&none), Err(EscrowRejectReason::ActionsNotOne));
}

#[test]
fn an_action_that_is_not_a_function_call_is_refused() {
    let key = SecretKey::from_random(KeyType::ED25519);
    let transfer = Action::Transfer(TransferAction {
        deposit: NearToken::from_yoctonear(1),
    });
    let da = delegate(PAYER, USDC_TESTNET, vec![transfer], &key);
    assert_eq!(validate(&da), Err(EscrowRejectReason::NotFunctionCall));
}

#[test]
fn another_method_is_refused() {
    for method in ["ft_transfer", "storage_deposit", "ft_transfer_call "] {
        let da = authorization(|c| c.method_name = method.to_string());
        assert_eq!(
            validate(&da),
            Err(EscrowRejectReason::MethodNotAllowed),
            "{method}"
        );
    }
}

#[test]
fn a_call_on_another_token_is_refused() {
    let da = unsigned(PAYER, "usdt.fakes.testnet", vec![authorize_call()]);
    assert_eq!(validate(&da), Err(EscrowRejectReason::WrongToken));
    // The mainnet USDC is another token on testnet.
    let da = unsigned(PAYER, USDC_MAINNET, vec![authorize_call()]);
    assert_eq!(validate(&da), Err(EscrowRejectReason::WrongToken));
}

#[test]
fn malformed_arguments_are_refused() {
    let da = authorization(|c| with_args(c, |a| a["unexpected"] = json!(1)));
    assert_eq!(validate(&da), Err(EscrowRejectReason::MalformedArgs));
    let da = authorization(|c| c.args = b"not json".to_vec());
    assert_eq!(validate(&da), Err(EscrowRejectReason::MalformedArgs));
    // memo is NEP-141's own optional field.
    let da = authorization(|c| with_args(c, |a| a["memo"] = json!("task 1")));
    assert!(validate(&da).is_ok());
}

#[test]
fn a_receiver_that_is_not_a_configured_escrow_is_refused() {
    let da = authorization(|c| with_args(c, |a| a["receiver_id"] = json!("thief.testnet")));
    assert_eq!(validate(&da), Err(EscrowRejectReason::ContractNotServed));
    let da = authorization(|c| with_args(c, |a| a["receiver_id"] = json!("Not An Account")));
    assert_eq!(validate(&da), Err(EscrowRejectReason::ContractNotServed));
}

#[test]
fn a_configured_escrow_other_than_the_402_names_is_refused() {
    let da = authorization(|c| with_args(c, |a| a["receiver_id"] = json!(CONTRACT_V2)));
    assert_eq!(validate(&da), Err(EscrowRejectReason::ContractMismatch));
}

#[test]
fn a_deposit_of_0_or_2_is_refused() {
    for deposit in [0, 2] {
        let da = authorization(|c| c.deposit = NearToken::from_yoctonear(deposit));
        assert_eq!(
            validate(&da),
            Err(EscrowRejectReason::WrongDeposit),
            "{deposit}"
        );
    }
}

#[test]
fn gas_outside_the_range_is_refused() {
    let config = cfg();
    for gas in [
        config.min_gas.as_gas() - 1,
        config.max_gas.as_gas() + 1,
        0,
        Gas::from_teragas(300).as_gas(),
    ] {
        let da = authorization(|c| c.gas = Gas::from_gas(gas));
        assert_eq!(
            validate(&da),
            Err(EscrowRejectReason::GasOutOfRange),
            "{gas}"
        );
    }
    for gas in [config.min_gas, config.max_gas] {
        let da = authorization(|c| c.gas = gas);
        assert!(validate(&da).is_ok(), "{gas:?}");
    }
}

#[test]
fn the_default_gas_floor_is_w09_plus_20_percent() {
    // W-09: an authorization with Circle's USDC needs 31 TGas prepaid; +20 %.
    let da = authorization(|c| c.gas = Gas::from_teragas(37));
    assert_eq!(validate(&da), Err(EscrowRejectReason::GasOutOfRange));
    let da = authorization(|c| c.gas = Gas::from_teragas(38));
    assert!(validate(&da).is_ok());
}

#[test]
fn another_amount_is_refused() {
    for amount in ["999999", "1000001", "01000000", "+1000000", "1000000.0", ""] {
        let da = authorization(|c| with_args(c, |a| a["amount"] = json!(amount)));
        assert_eq!(
            validate(&da),
            Err(EscrowRejectReason::AmountMismatch),
            "{amount:?}"
        );
    }
}

#[test]
fn a_msg_with_one_more_space_is_refused() {
    let spaced = v1_msg().replacen(",\"operator_id\"", ", \"operator_id\"", 1);
    let da = authorization(|c| with_args(c, |a| a["msg"] = json!(spaced)));
    assert_eq!(validate(&da), Err(EscrowRejectReason::MsgMismatch));
}

#[test]
fn a_non_canonical_msg_is_refused_even_when_the_402_carries_it() {
    let spaced = v1_msg().replacen(",\"operator_id\"", ", \"operator_id\"", 1);
    let da = authorization(|c| with_args(c, |a| a["msg"] = json!(spaced.clone())));
    let mut req = request(None);
    req.extra.msg = Some(spaced);
    assert_eq!(
        validate_escrow_delegate_actions(&da, &req, &cfg(), NOW),
        Err(EscrowRejectReason::MsgNotCanonical)
    );
}

#[test]
fn an_operator_other_than_the_402_names_is_refused() {
    let mut req = request(None);
    req.extra.operator_id = Some("kk".to_string());
    assert_eq!(
        validate_escrow_delegate_actions(&authorization(|_| {}), &req, &cfg(), NOW),
        Err(EscrowRejectReason::OperatorMismatch)
    );
}

#[test]
fn an_operator_the_facilitator_does_not_serve_is_refused() {
    let config = NearEscrowConfig {
        operator_ids: vec!["kk".to_string()],
        ..cfg()
    };
    assert_eq!(
        validate_escrow_delegate_actions(&authorization(|_| {}), &request(None), &config, NOW),
        Err(EscrowRejectReason::OperatorNotServed)
    );
}

#[test]
fn another_payment_id_is_refused() {
    let mut req = request(None);
    req.extra.payment_id = Some(V2_ID.to_string());
    assert_eq!(
        validate_escrow_delegate_actions(&authorization(|_| {}), &req, &cfg(), NOW),
        Err(EscrowRejectReason::PaymentIdMismatch)
    );
    // Same terms, another payer: another id.
    let da = unsigned("someone-else.testnet", USDC_TESTNET, vec![authorize_call()]);
    assert_eq!(validate(&da), Err(EscrowRejectReason::PaymentIdMismatch));
}

#[test]
fn an_extra_missing_its_fields_is_refused() {
    for edit in [
        (|e: &mut NearEscrowExtra| e.contract = None) as fn(&mut NearEscrowExtra),
        |e| e.msg = None,
        |e| e.payment_id = None,
    ] {
        let mut req = request(None);
        edit(&mut req.extra);
        assert!(
            validate_escrow_delegate_actions(&authorization(|_| {}), &req, &cfg(), NOW).is_err()
        );
    }
    let mut req = request(None);
    req.extra.msg = None;
    assert_eq!(
        validate_escrow_delegate_actions(&authorization(|_| {}), &req, &cfg(), NOW),
        Err(EscrowRejectReason::MissingExtra)
    );
}

fn v1_body(network: &str) -> Value {
    json!({
        "x402Version": 1,
        "paymentPayload": {
            "x402Version": 1,
            "scheme": "escrow",
            "network": network,
            "payload": {"signedDelegateAction": "AAAA"},
        },
        "paymentRequirements": {
            "scheme": "escrow",
            "network": network,
            "maxAmountRequired": "1000000",
            "resource": "https://execution.market/tasks/1",
            "description": "",
            "mimeType": "application/json",
            "payTo": WORKER,
            "maxTimeoutSeconds": 60,
            "asset": USDC_TESTNET,
            "extra": {"nearEscrow": {
                "contract": CONTRACT, "operatorId": "em", "msg": v1_msg(), "paymentId": V1_ID,
            }},
        },
    })
}

#[test]
fn a_v1_and_a_v2_body_parse_to_the_same_request() {
    let v1 = EscrowRequest::parse(&v1_body("near-testnet").to_string()).unwrap();
    assert_eq!(v1.network, Network::NearTestnet);
    assert_eq!(v1.signed_delegate_action, "AAAA");
    assert_eq!(v1.amount, Some(AMOUNT));
    assert_eq!(v1.asset.as_deref(), Some(USDC_TESTNET));
    assert_eq!(v1.extra.msg.as_deref(), Some(v1_msg().as_str()));
    assert_eq!(v1.extra.payment_id.as_deref(), Some(V1_ID));

    let v2 = json!({
        "x402Version": 2,
        "paymentPayload": {
            "x402Version": 2,
            "accepted": {
                "scheme": "escrow", "network": "near:testnet", "amount": "1000000",
                "asset": USDC_TESTNET, "payTo": WORKER, "maxTimeoutSeconds": 60,
                "extra": {"nearEscrow": {"contract": CONTRACT, "operatorId": "em", "msg": v1_msg(), "paymentId": V1_ID}},
            },
            "payload": {"signedDelegateAction": "AAAA"},
        },
    });
    let v2 = EscrowRequest::parse(&v2.to_string()).unwrap();
    assert_eq!(v2.network, v1.network);
    assert_eq!(v2.amount, v1.amount);
    assert_eq!(v2.extra.contract, v1.extra.contract);
}

#[test]
fn a_body_that_contradicts_itself_is_refused() {
    let mut body = v1_body("near-testnet");
    body["paymentRequirements"]["scheme"] = json!("exact");
    assert_eq!(
        EscrowRequest::parse(&body.to_string()).err(),
        Some(EscrowRejectReason::SchemeMismatch)
    );
    let mut body = v1_body("near-testnet");
    body["paymentPayload"]["network"] = json!("near");
    assert_eq!(
        EscrowRequest::parse(&body.to_string()).err(),
        Some(EscrowRejectReason::NetworkMismatch)
    );
    let mut body = v1_body("near-testnet");
    body["paymentRequirements"]["amount"] = json!("2000000");
    assert_eq!(
        EscrowRequest::parse(&body.to_string()).err(),
        Some(EscrowRejectReason::MalformedRequest)
    );
    let mut body = v1_body("near-testnet");
    body["paymentRequirements"]["extra"] = json!({});
    assert_eq!(
        EscrowRequest::parse(&body.to_string()).err(),
        Some(EscrowRejectReason::MissingExtra)
    );
    let mut body = v1_body("near-testnet");
    body["paymentRequirements"]["network"] = json!("base");
    assert_eq!(
        EscrowRequest::parse(&body.to_string()).err(),
        Some(EscrowRejectReason::MalformedRequest)
    );
}

#[test]
fn near_networks_are_named_either_way() {
    assert_eq!(near_network("near"), Some(Network::Near));
    assert_eq!(near_network("near:mainnet"), Some(Network::Near));
    assert_eq!(near_network("near-testnet"), Some(Network::NearTestnet));
    assert_eq!(near_network("near:testnet"), Some(Network::NearTestnet));
    assert_eq!(near_network("base"), None);
    assert_eq!(near_network("eip155:8453"), None);
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

fn rid(label: &str) -> String {
    CryptoHash::hash_bytes(label.as_bytes()).to_string()
}

fn receipt(
    id: &str,
    executor: &str,
    status: Value,
    receipt_ids: &[&str],
    logs: &[String],
) -> Value {
    json!({
        "id": id,
        "block_hash": "11111111111111111111111111111111",
        "proof": [],
        "outcome": {
            "executor_id": executor,
            "gas_burnt": 0,
            "tokens_burnt": "0",
            "logs": logs,
            "metadata": {"gas_profile": null, "version": 1},
            "receipt_ids": receipt_ids,
            "status": status,
        },
    })
}

fn returns(text: &str) -> Value {
    json!({ "SuccessValue": b64(text.as_bytes()) })
}

fn promise(id: &str) -> Value {
    json!({ "SuccessReceiptId": id })
}

fn panicked(message: &str) -> Value {
    json!({"Failure": {"ActionError": {"index": 0, "kind": {
        "FunctionCallError": {"ExecutionError": format!("Smart contract panicked: {message}")}
    }}}})
}

fn nep141(old: &str, new: &str, amount: &str) -> String {
    format!(
        "EVENT_JSON:{}",
        json!({"standard": "nep141", "version": "1.0.0", "event": "ft_transfer",
               "data": [{"old_owner_id": old, "new_owner_id": new, "amount": amount}]})
    )
}

fn arras(event: &str, data: Value) -> String {
    format!(
        "EVENT_JSON:{}",
        json!({"standard": "arras", "version": "1.0.0", "event": event, "data": [data]})
    )
}

fn authorized_event(payment_id: &str) -> String {
    arras(
        "authorized",
        json!({"payment_id": payment_id, "operator_id": "em", "token": USDC_TESTNET,
               "payer": PAYER, "receiver": WORKER, "amount": "1000000", "fee_bps": 1300,
               "fee_recipient": "em-fees.arras-dev.testnet", "terms_hash": "0".repeat(64)}),
    )
}

/// The recorded settlement's envelope around `receipts`.
fn outcome_with(receipts: Vec<Value>) -> Value {
    let mut outcome: Value = serde_json::from_str(RECORDED).unwrap();
    assert_eq!(
        outcome["transaction_outcome"]["outcome"]["status"]["SuccessReceiptId"],
        DELEGATE_RECEIPT
    );
    outcome["receipts_outcome"] = json!(receipts);
    outcome
}

/// What an accepted V-1 authorization's outcome looks like, NEP-141's
/// `ft_transfer_call` as near-contract-standards runs it.
fn authorized_outcome() -> Value {
    let (call, on, resolve, refund) = (rid("call"), rid("on"), rid("resolve"), rid("refund"));
    outcome_with(vec![
        receipt(DELEGATE_RECEIPT, PAYER, returns(""), &[&call], &[]),
        receipt(
            &call,
            USDC_TESTNET,
            promise(&resolve),
            &[&on, &resolve],
            &[nep141(PAYER, CONTRACT, "1000000")],
        ),
        receipt(
            &on,
            CONTRACT,
            returns("\"0\""),
            &[],
            &[authorized_event(V1_ID)],
        ),
        receipt(&resolve, USDC_TESTNET, returns("\"1000000\""), &[], &[]),
        receipt(&refund, RELAYER, returns(""), &[], &[]),
    ])
}

fn receipt_mut<'a>(outcome: &'a mut Value, id: &str) -> &'a mut Value {
    outcome["receipts_outcome"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["id"] == id)
        .map(|r| &mut r["outcome"])
        .unwrap_or_else(|| panic!("no receipt {id}"))
}

fn view(outcome: &Value) -> FinalExecutionOutcomeView {
    serde_json::from_value(outcome.clone()).expect("an outcome")
}

fn v1_authorize() -> EscrowAuthorize {
    EscrowAuthorize {
        contract: account(CONTRACT),
        token: account(USDC_TESTNET),
        payer: account(PAYER),
        amount: AMOUNT,
        payment_id: V1_ID.to_string(),
    }
}

fn confirm(outcome: &Value) -> Result<CryptoHash, FacilitatorLocalError> {
    confirm_escrow_authorize(&view(outcome), &v1_authorize(), Network::NearTestnet)
}

fn assert_paid(result: Result<CryptoHash, FacilitatorLocalError>) {
    assert_eq!(result.unwrap(), TX_HASH.parse::<CryptoHash>().unwrap());
}

fn assert_not_paid(result: Result<CryptoHash, FacilitatorLocalError>) -> String {
    match result {
        Err(FacilitatorLocalError::ContractCall(message)) => message,
        other => panic!("expected a not-paid verdict, got {other:?}"),
    }
}

fn assert_no_verdict(result: Result<CryptoHash, FacilitatorLocalError>) {
    let hash: CryptoHash = TX_HASH.parse().unwrap();
    match result {
        Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
            assert_eq!(tx, TransactionHash::Near(hash.0));
            assert_eq!(network, Network::NearTestnet);
        }
        other => panic!("expected no verdict, got {other:?}"),
    }
}

#[test]
fn an_authorization_the_escrow_accepted_is_paid() {
    assert_paid(confirm(&authorized_outcome()));
}

/// W-13 as the relayer would see it, in the names given: the sandbox signed
/// `ft_transfer_call` as the payer, and a relayed authorization runs inside a
/// Delegate, which adds one receipt on the payer above the token's. That
/// receipt is the only thing added; every recorded receipt stays as it was.
fn w13_relayed(token: &str, payer: &str, escrow: &str) -> Value {
    let text = W13
        .replace("usdc.test.near", token)
        .replace("payer.test.near", payer)
        .replace("arras.test.near", escrow);
    let mut outcome: Value = serde_json::from_str(&text).unwrap();
    let call = outcome["transaction_outcome"]["outcome"]["status"]["SuccessReceiptId"]
        .as_str()
        .unwrap()
        .to_string();
    let delegate = rid("w13-delegate");
    outcome["transaction_outcome"]["outcome"]["status"] = promise(&delegate);
    outcome["transaction_outcome"]["outcome"]["receipt_ids"] = json!([delegate]);
    outcome["receipts_outcome"]
        .as_array_mut()
        .unwrap()
        .insert(0, receipt(&delegate, payer, returns(""), &[&call], &[]));
    outcome
}

/// The msg the escrow repository's own test signed in W-13.
fn w13_msg() -> String {
    let w13: Value = serde_json::from_str(W13).unwrap();
    let args = w13["transaction"]["actions"][0]["FunctionCall"]["args"]
        .as_str()
        .unwrap();
    let args: Value = serde_json::from_slice(
        &base64::Engine::decode(&base64::engine::general_purpose::STANDARD, args).unwrap(),
    )
    .unwrap();
    args["msg"].as_str().unwrap().to_string()
}

#[test]
fn the_escrow_repository_signs_the_canonical_msg() {
    let terms = parse_authorize_msg(&w13_msg()).expect("W-13's msg is canonical");
    assert_eq!(terms.receiver, account("worker.test.near"));
    assert_eq!(terms.operator_id, "em");
    assert_eq!(terms.fee_bps_max, 1300);
    assert_eq!(terms.salt[15], 1);
}

#[test]
fn w13_a_refused_authorization_is_not_paid() {
    // What W-13 recorded: the escrow refused (E06), the token refunded the
    // payer and ft_transfer_call's final value is "0".
    let recorded: Value = serde_json::from_str(W13).unwrap();
    assert_eq!(recorded["status"], returns("\"0\""));
    let logs: Vec<String> = recorded["receipts_outcome"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["outcome"]["logs"].as_array().unwrap().clone())
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    assert!(logs.contains(&"arras:rechazo:E06_TOPE_POR_PAGO".to_string()));
    assert!(logs
        .iter()
        .any(|l| l.contains("\"old_owner_id\":\"arras.test.near\"")
            && l.contains("\"new_owner_id\":\"payer.test.near\"")
            && l.contains("\"memo\":\"refund\"")));

    let outcome = w13_relayed("usdc.test.near", "payer.test.near", "arras.test.near");
    let terms = parse_authorize_msg(&w13_msg()).unwrap();
    let expected = EscrowAuthorize {
        contract: account("arras.test.near"),
        token: account("usdc.test.near"),
        payer: account("payer.test.near"),
        amount: 10_000_001,
        payment_id: compute_payment_id(&terms, "usdc.test.near", "payer.test.near", 10_000_001),
    };
    match confirm_escrow_authorize(&view(&outcome), &expected, Network::NearTestnet) {
        Err(FacilitatorLocalError::ContractCall(message)) => {
            assert!(message.contains("used 0 of 10000001"), "got: {message}")
        }
        other => panic!("expected a not-paid verdict, got {other:?}"),
    }
}

#[test]
fn a_partial_amount_is_not_paid() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("resolve"))["status"] = returns("\"999999\"");
    assert_not_paid(confirm(&outcome));
}

#[test]
fn a_and_b_without_c_is_no_verdict() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([]);
    assert_no_verdict(confirm(&outcome));
}

#[test]
fn c_for_another_payment_is_no_verdict() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([authorized_event(V2_ID)]);
    assert_no_verdict(confirm(&outcome));
    // Another event of the same payment is not the authorization.
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([arras(
        "refunded",
        json!({"payment_id": V1_ID, "amount": "1000000"})
    )]);
    assert_no_verdict(confirm(&outcome));
    // An `authorized` under another standard is not the escrow's.
    let mut outcome = authorized_outcome();
    let foreign =
        authorized_event(V1_ID).replace("\"standard\":\"arras\"", "\"standard\":\"escrow\"");
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([foreign]);
    assert_no_verdict(confirm(&outcome));
}

#[test]
fn c_logged_by_another_account_does_not_count() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([]);
    receipt_mut(&mut outcome, &rid("call"))["logs"] =
        json!([nep141(PAYER, CONTRACT, "1000000"), authorized_event(V1_ID)]);
    assert_no_verdict(confirm(&outcome));
}

#[test]
fn c_in_a_failed_receipt_does_not_count() {
    // A contract that panics keeps the logs it wrote before the panic.
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["status"] = panicked("after the event");
    assert_no_verdict(confirm(&outcome));
}

#[test]
fn a_and_c_without_b_is_no_verdict() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("call"))["logs"] = json!([]);
    assert_no_verdict(confirm(&outcome));
    for (old, new, amount) in [
        ("someone-else.testnet", CONTRACT, "1000000"),
        (PAYER, "thief.testnet", "1000000"),
        (PAYER, CONTRACT, "999999"),
    ] {
        let mut outcome = authorized_outcome();
        receipt_mut(&mut outcome, &rid("call"))["logs"] = json!([nep141(old, new, amount)]);
        assert_no_verdict(confirm(&outcome));
    }
}

#[test]
fn a_missing_or_unread_a_is_no_verdict() {
    let mut outcome = authorized_outcome();
    outcome["receipts_outcome"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r["id"] != rid("resolve"));
    assert_no_verdict(confirm(&outcome));
    for status in [
        json!("Unknown"),
        panicked("resolve"),
        promise(&rid("refund")),
        returns("1000000"),
        returns(""),
    ] {
        let mut outcome = authorized_outcome();
        receipt_mut(&mut outcome, &rid("resolve"))["status"] = status.clone();
        assert_no_verdict(confirm(&outcome));
    }
    // The promise resolves on another contract.
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("resolve"))["executor_id"] = json!("usdt.fakes.testnet");
    assert_no_verdict(confirm(&outcome));
}

#[test]
fn a_failed_ft_transfer_call_is_not_paid() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("call"))["status"] =
        panicked("The account doesn't have enough balance");
    let message = assert_not_paid(confirm(&outcome));
    assert!(
        message.contains("ft_transfer_call failed"),
        "got: {message}"
    );
}

#[test]
fn a_failed_delegate_action_is_not_paid() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, DELEGATE_RECEIPT)["status"] =
        json!({"Failure": {"ActionError": {"index": 0, "kind": "DelegateActionExpired"}}});
    let message = assert_not_paid(confirm(&outcome));
    assert!(message.contains("Delegate action failed"), "got: {message}");
}

#[test]
fn an_ft_transfer_call_with_no_promise_or_unexecuted_is_no_verdict() {
    for status in [returns("\"1000000\""), json!("Unknown")] {
        let mut outcome = authorized_outcome();
        receipt_mut(&mut outcome, &rid("call"))["status"] = status.clone();
        assert_no_verdict(confirm(&outcome));
    }
}

/// Two receipts of `ft_transfer_call` on the escrow, one of them the accepted
/// authorization, whichever comes first: the outcome is not the one NEP-141
/// produces, and neither is read as (c).
#[test]
fn two_ft_on_transfer_receipts_on_the_escrow_are_no_verdict() {
    let (call, on, other, resolve) = (rid("call"), rid("on"), rid("other"), rid("resolve"));
    let accepted = receipt(
        &on,
        CONTRACT,
        returns("\"0\""),
        &[],
        &[authorized_event(V1_ID)],
    );
    let second = receipt(&other, CONTRACT, returns("\"0\""), &[], &[]);
    for children in [[&on, &other], [&other, &on]] {
        let outcome = outcome_with(vec![
            receipt(DELEGATE_RECEIPT, PAYER, returns(""), &[&call], &[]),
            receipt(
                &call,
                USDC_TESTNET,
                promise(&resolve),
                &[children[0], children[1], &resolve],
                &[nep141(PAYER, CONTRACT, "1000000")],
            ),
            accepted.clone(),
            second.clone(),
            receipt(&resolve, USDC_TESTNET, returns("\"1000000\""), &[], &[]),
        ]);
        assert_no_verdict(confirm(&outcome));
    }
}

#[test]
fn a_receipt_the_delegate_did_not_create_does_not_count() {
    // Two receipts on the token, or none, under the Delegate action.
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, DELEGATE_RECEIPT)["receipt_ids"] =
        json!([rid("call"), rid("resolve")]);
    assert_no_verdict(confirm(&outcome));
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, DELEGATE_RECEIPT)["receipt_ids"] = json!([]);
    assert_no_verdict(confirm(&outcome));
}

// ---------------------------------------------------------------------------
// The lifecycle
// ---------------------------------------------------------------------------

fn lifecycle(method: &str, args: Value, gas_tgas: u64) -> FunctionCallAction {
    function_call(method, args.to_string().into_bytes(), gas_tgas, 0)
}

fn release_call() -> FunctionCallAction {
    lifecycle("release", json!({"payment_id": V1_ID}), 30)
}

fn lifecycle_check(call: FunctionCallAction) -> Result<LifecycleCall, EscrowRejectReason> {
    let da = unsigned(CAPTURER, CONTRACT, vec![call]);
    validate_escrow_lifecycle_actions(&da, &cfg(), Network::NearTestnet)
}

#[test]
fn every_lifecycle_method_passes_with_its_own_arguments() {
    let cases = [
        (
            lifecycle("release", json!({"payment_id": V1_ID}), 30),
            LifecycleMethod::Release,
        ),
        (
            lifecycle(
                "release",
                json!({"payment_id": V1_ID, "amount": "400000"}),
                30,
            ),
            LifecycleMethod::Release,
        ),
        (
            lifecycle("refund", json!({"payment_id": V1_ID}), 30),
            LifecycleMethod::Refund,
        ),
        (
            lifecycle("reclaim", json!({"payment_id": V2_ID}), 30),
            LifecycleMethod::Reclaim,
        ),
        (
            lifecycle("withdraw", json!({}), 50),
            LifecycleMethod::Withdraw,
        ),
        (
            function_call("withdraw", vec![], 50, 0),
            LifecycleMethod::Withdraw,
        ),
        (
            lifecycle("withdraw_for", json!({"account": WORKER}), 50),
            LifecycleMethod::WithdrawFor,
        ),
    ];
    for (call, method) in cases {
        let checked = lifecycle_check(call).unwrap();
        assert_eq!(checked.method, method);
        assert_eq!(checked.contract, account(CONTRACT));
    }
    let checked =
        lifecycle_check(lifecycle("withdraw_for", json!({"account": WORKER}), 50)).unwrap();
    assert_eq!(checked.account, Some(account(WORKER)));
    assert_eq!(
        lifecycle_check(release_call())
            .unwrap()
            .payment_id
            .as_deref(),
        Some(V1_ID)
    );
}

#[test]
fn a_lifecycle_call_on_an_account_that_is_not_a_configured_escrow_is_refused() {
    let da = unsigned(CAPTURER, "v9.arras-dev.testnet", vec![release_call()]);
    assert_eq!(
        validate_escrow_lifecycle_actions(&da, &cfg(), Network::NearTestnet),
        Err(EscrowRejectReason::ContractNotServed)
    );
    // The testnet escrow is not a mainnet one.
    let da = unsigned(CAPTURER, CONTRACT, vec![release_call()]);
    assert_eq!(
        validate_escrow_lifecycle_actions(&da, &cfg(), Network::Near),
        Err(EscrowRejectReason::ContractNotServed)
    );
}

#[test]
fn a_method_outside_the_lifecycle_is_refused() {
    for method in [
        "register_operator",
        "update_operator",
        "pause",
        "compact",
        "open_dispute",
        "resolve_dispute",
        "withdraw_budget",
        "authorize_from_budget",
        "on_withdraw",
        "Release",
    ] {
        assert_eq!(
            lifecycle_check(lifecycle(method, json!({"payment_id": V1_ID}), 30)),
            Err(EscrowRejectReason::MethodNotAllowed),
            "{method}"
        );
    }
}

#[test]
fn a_lifecycle_call_with_a_deposit_is_refused() {
    let mut call = release_call();
    call.deposit = NearToken::from_yoctonear(1);
    assert_eq!(lifecycle_check(call), Err(EscrowRejectReason::WrongDeposit));
}

#[test]
fn a_lifecycle_call_over_its_gas_cap_is_refused() {
    for (method, args, cap) in [
        ("release", json!({"payment_id": V1_ID}), 30),
        ("refund", json!({"payment_id": V1_ID}), 30),
        ("reclaim", json!({"payment_id": V1_ID}), 30),
        ("withdraw", json!({}), 50),
        ("withdraw_for", json!({"account": WORKER}), 50),
    ] {
        let mut call = lifecycle(method, args.clone(), cap);
        assert!(lifecycle_check(call.clone()).is_ok(), "{method} at its cap");
        call.gas = Gas::from_gas(Gas::from_teragas(cap).as_gas() + 1);
        assert_eq!(
            lifecycle_check(call),
            Err(EscrowRejectReason::GasOutOfRange),
            "{method}"
        );
    }
}

#[test]
fn a_lifecycle_call_under_its_gas_floor_is_refused() {
    // W-09: release, refund and reclaim pass with 2 TGas prepaid, a withdrawal
    // with 24; each floor is that plus 20 %, rounded up.
    for (method, args, measured, floor) in [
        ("release", json!({"payment_id": V1_ID}), 2, 3),
        ("refund", json!({"payment_id": V1_ID}), 2, 3),
        ("reclaim", json!({"payment_id": V1_ID}), 2, 3),
        ("withdraw", json!({}), 24, 29),
        ("withdraw_for", json!({"account": WORKER}), 24, 29),
    ] {
        assert_eq!(floor, (measured * 12u64).div_ceil(10), "{method}");
        let call = lifecycle(method, args.clone(), floor);
        assert!(
            lifecycle_check(call.clone()).is_ok(),
            "{method} at its floor"
        );
        for gas in [
            Gas::from_gas(Gas::from_teragas(floor).as_gas() - 1),
            Gas::from_teragas(floor - 1),
            Gas::from_teragas(0),
        ] {
            let mut under = call.clone();
            under.gas = gas;
            assert_eq!(
                lifecycle_check(under),
                Err(EscrowRejectReason::GasOutOfRange),
                "{method} with {gas:?}"
            );
        }
    }
}

#[test]
fn a_lifecycle_call_with_other_arguments_is_refused() {
    for call in [
        lifecycle(
            "release",
            json!({"payment_id": V1_ID, "to": "thief.testnet"}),
            30,
        ),
        lifecycle("release", json!({"payment_id": V1_ID, "amount": "0"}), 30),
        lifecycle("release", json!({"payment_id": V1_ID, "amount": "01"}), 30),
        lifecycle("release", json!({"payment_id": "not-an-id"}), 30),
        lifecycle("refund", json!({}), 30),
        lifecycle(
            "reclaim",
            json!({"payment_id": "11111111111111111111111111111111111111111111111111"}),
            30,
        ),
        lifecycle("withdraw", json!({"account": WORKER}), 50),
        lifecycle("withdraw_for", json!({"account": "Not An Account"}), 50),
        lifecycle("withdraw_for", json!({}), 50),
    ] {
        let name = call.method_name.clone();
        assert_eq!(
            lifecycle_check(call),
            Err(EscrowRejectReason::MalformedArgs),
            "{name}"
        );
    }
}

#[test]
fn a_payment_id_is_the_base58_of_32_bytes_whatever_its_length() {
    // 32 bytes: 44, 43, 42 and 32 characters (leading zero bytes are '1's).
    for id in [
        V1_ID,
        V2_ID,
        "1y1ZbrieUbaioiUBHxX7EyZMNkCis2tLKCs5Xxezgy",
        "11111111111111111111111111111111",
    ] {
        assert!(is_payment_id(id), "{id} ({} chars)", id.len());
        let call = lifecycle("refund", json!({ "payment_id": id }), 30);
        assert!(lifecycle_check(call).is_ok(), "{id}");
    }
    // 31 bytes in 42 characters, 33 bytes in 45: the length decides nothing.
    let short = bs58::encode([7u8; 31]).into_string();
    let long = bs58::encode([7u8; 33]).into_string();
    assert_eq!((short.len(), long.len()), (42, 45));
    for id in [short, long] {
        assert!(!is_payment_id(&id), "{id}");
        let call = lifecycle("refund", json!({ "payment_id": id }), 30);
        assert_eq!(
            lifecycle_check(call),
            Err(EscrowRejectReason::MalformedArgs),
            "{id}"
        );
    }
}

#[test]
fn a_lifecycle_delegate_with_two_actions_is_refused() {
    let da = unsigned(CAPTURER, CONTRACT, vec![release_call(), release_call()]);
    assert_eq!(
        validate_escrow_lifecycle_actions(&da, &cfg(), Network::NearTestnet),
        Err(EscrowRejectReason::ActionsNotOne)
    );
}

/// The outcome of a relayed escrow call: the Delegate on `sender`, its one
/// receipt on the escrow ending in `status` with `logs`.
fn lifecycle_outcome(sender: &str, status: Value, logs: Vec<String>) -> Value {
    let call = rid("lifecycle");
    outcome_with(vec![
        receipt(DELEGATE_RECEIPT, sender, returns(""), &[&call], &[]),
        receipt(&call, CONTRACT, status, &[], &logs),
    ])
}

/// A `withdraw*` for `account`: the call returns the promise of its
/// `on_withdraw` callback, which logs `withdrawn`.
fn withdraw_outcome(sender: &str, account: &str, ok: bool, callback: Value) -> Value {
    let (call, transfer, back) = (rid("withdraw"), rid("transfer"), rid("on_withdraw"));
    let moved = if ok {
        vec![nep141(CONTRACT, account, "870000")]
    } else {
        vec![]
    };
    outcome_with(vec![
        receipt(DELEGATE_RECEIPT, sender, returns(""), &[&call], &[]),
        receipt(&call, CONTRACT, promise(&back), &[&transfer, &back], &[]),
        receipt(
            &transfer,
            USDC_TESTNET,
            if ok {
                returns("")
            } else {
                panicked("The account is not registered")
            },
            &[],
            &moved,
        ),
        receipt(
            &back,
            CONTRACT,
            callback,
            &[],
            &[arras(
                "withdrawn",
                json!({"account": account, "token": USDC_TESTNET, "amount": "870000", "ok": ok}),
            )],
        ),
    ])
}

fn confirm_lifecycle(
    outcome: &Value,
    expected: &ExpectedEvent,
) -> Result<CryptoHash, FacilitatorLocalError> {
    confirm_escrow_lifecycle(
        &view(outcome),
        &account(CONTRACT),
        expected,
        Network::NearTestnet,
    )
}

fn payment_event(event: &'static str) -> ExpectedEvent {
    ExpectedEvent::Payment {
        event,
        payment_id: V1_ID.to_string(),
    }
}

#[test]
fn each_lifecycle_effect_is_confirmed_by_its_event() {
    for event in ["released", "refunded", "reclaimed", "authorized"] {
        let outcome = lifecycle_outcome(
            CAPTURER,
            returns(""),
            vec![arras(event, json!({"payment_id": V1_ID}))],
        );
        assert_paid(confirm_lifecycle(&outcome, &payment_event(event)));
    }
    let outcome = withdraw_outcome(WORKER, WORKER, true, returns("\"870000\""));
    assert_paid(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
}

#[test]
fn a_lifecycle_call_whose_receipt_failed_is_not_paid() {
    for event in ["released", "refunded", "reclaimed", "authorized"] {
        // The event logged before the panic does not count.
        let outcome = lifecycle_outcome(
            CAPTURER,
            panicked("ARRAS_E22: estado invalido"),
            vec![arras(event, json!({"payment_id": V1_ID}))],
        );
        assert_not_paid(confirm_lifecycle(&outcome, &payment_event(event)));
    }
    let outcome = withdraw_outcome(WORKER, WORKER, true, panicked("on_withdraw"));
    assert_not_paid(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
}

#[test]
fn a_withdrawal_that_came_back_is_not_paid() {
    let outcome = withdraw_outcome(WORKER, WORKER, false, returns("\"0\""));
    let message = assert_not_paid(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
    assert!(message.contains("ok:false"), "got: {message}");
}

#[test]
fn a_lifecycle_call_without_its_event_is_no_verdict() {
    // No event; another payment's; another event; another account's withdrawal.
    for (logs, expected) in [
        (vec![], payment_event("released")),
        (
            vec![arras("released", json!({"payment_id": V2_ID}))],
            payment_event("released"),
        ),
        (
            vec![arras("refunded", json!({"payment_id": V1_ID}))],
            payment_event("released"),
        ),
        (
            vec![arras("released", json!({"payment_id": V1_ID}))],
            payment_event("reclaimed"),
        ),
    ] {
        let outcome = lifecycle_outcome(CAPTURER, returns(""), logs);
        assert_no_verdict(confirm_lifecycle(&outcome, &expected));
    }
    let outcome = withdraw_outcome(
        WORKER,
        "em-fees.arras-dev.testnet",
        true,
        returns("\"870000\""),
    );
    assert_no_verdict(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
}

#[test]
fn a_lifecycle_call_not_executed_or_off_the_escrow_is_no_verdict() {
    let outcome = lifecycle_outcome(CAPTURER, json!("Unknown"), vec![]);
    assert_no_verdict(confirm_lifecycle(&outcome, &payment_event("released")));
    // The callback is missing from the outcome.
    let mut outcome = withdraw_outcome(WORKER, WORKER, true, returns("\"870000\""));
    outcome["receipts_outcome"]
        .as_array_mut()
        .unwrap()
        .retain(|r| r["id"] != rid("on_withdraw"));
    assert_no_verdict(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
    // The promise resolves on the token, not the escrow.
    let mut outcome = withdraw_outcome(WORKER, WORKER, true, returns("\"870000\""));
    receipt_mut(&mut outcome, &rid("withdraw"))["status"] = promise(&rid("transfer"));
    assert_no_verdict(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
    // ...even when the account it resolves on logs the event the escrow would.
    let mut outcome = withdraw_outcome(WORKER, WORKER, true, returns("\"870000\""));
    receipt_mut(&mut outcome, &rid("on_withdraw"))["executor_id"] = json!("thief.testnet");
    assert_no_verdict(confirm_lifecycle(
        &outcome,
        &ExpectedEvent::Withdrawn {
            account: account(WORKER),
        },
    ));
    // The call landed on another account.
    let outcome = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    assert_no_verdict(confirm_escrow_lifecycle(
        &view(&outcome),
        &account(CONTRACT_V2),
        &payment_event("released"),
        Network::NearTestnet,
    ));
}

#[test]
fn a_promise_that_never_resolves_is_no_verdict() {
    // A receipt whose promise is itself: followed a bounded number of hops.
    let call = rid("lifecycle");
    let outcome = lifecycle_outcome(
        CAPTURER,
        promise(&call),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    assert_no_verdict(confirm_lifecycle(&outcome, &payment_event("released")));
}

// ---------------------------------------------------------------------------
// authorize_from_budget (v2)
// ---------------------------------------------------------------------------

fn budget_args() -> Value {
    json!({
        "owner": PAYER, "operator_id": "em", "receiver": WORKER, "amount": "1000000",
        "release_deadline": "1793577600000000000", "reclaim_after": "1793664000000000000",
        "dispute_deadline": "1793620800000000000", "fee_bps_max": 1300,
        "terms_hash": "0".repeat(64), "salt": "0".repeat(32),
    })
}

fn budget_call(edit: impl FnOnce(&mut Value)) -> FunctionCallAction {
    let mut args = budget_args();
    edit(&mut args);
    lifecycle("authorize_from_budget", args, 30)
}

fn budget_check(call: FunctionCallAction) -> Result<BudgetAuthorize, EscrowRejectReason> {
    let da = unsigned(AGENT, CONTRACT, vec![call]);
    validate_budget_authorize(&da, &request(None), &cfg())
}

#[test]
fn a_budget_authorization_of_the_402_passes() {
    // The owner is the payer, so the V-1 terms give the V-1 id.
    assert_eq!(
        budget_check(budget_call(|_| {})),
        Ok(BudgetAuthorize {
            contract: account(CONTRACT),
            owner: account(PAYER),
            amount: AMOUNT,
            payment_id: V1_ID.to_string(),
        })
    );
}

#[test]
fn a_budget_authorization_off_the_402_is_refused() {
    use EscrowRejectReason::*;
    let cases: Vec<(&str, Value, EscrowRejectReason)> = vec![
        ("amount", json!("2000000"), AmountMismatch),
        ("receiver", json!("thief.testnet"), ReceiverMismatch),
        ("operator_id", json!("kk"), OperatorMismatch),
        ("fee_bps_max", json!(1800), FeeBpsMaxMismatch),
        (
            "release_deadline",
            json!("1793577600000000001"),
            TermsMismatch,
        ),
        ("reclaim_after", json!("1793664000000000001"), TermsMismatch),
        ("terms_hash", json!("1".repeat(64)), TermsMismatch),
        ("salt", json!(format!("{}1", "0".repeat(31))), TermsMismatch),
        ("owner", json!("someone-else.testnet"), PaymentIdMismatch),
    ];
    for (field, value, reason) in cases {
        let call = budget_call(|a| a[field] = value.clone());
        assert_eq!(budget_check(call), Err(reason), "{field}");
    }
}

#[test]
fn a_budget_authorization_with_another_operator_in_the_402_is_refused() {
    let da = unsigned(AGENT, CONTRACT, vec![budget_call(|_| {})]);
    let mut req = request(None);
    req.extra.operator_id = Some("kk".to_string());
    assert_eq!(
        validate_budget_authorize(&da, &req, &cfg()),
        Err(EscrowRejectReason::OperatorMismatch)
    );
    let config = NearEscrowConfig {
        operator_ids: vec!["kk".to_string()],
        ..cfg()
    };
    assert_eq!(
        validate_budget_authorize(&da, &request(None), &config),
        Err(EscrowRejectReason::OperatorNotServed)
    );
    let mut req = request(None);
    req.extra.payment_id = Some(V2_ID.to_string());
    assert_eq!(
        validate_budget_authorize(&da, &req, &cfg()),
        Err(EscrowRejectReason::PaymentIdMismatch)
    );
}

#[test]
fn a_budget_authorization_out_of_shape_is_refused() {
    use EscrowRejectReason::*;
    let mut deposit = budget_call(|_| {});
    deposit.deposit = NearToken::from_yoctonear(1);
    assert_eq!(budget_check(deposit), Err(WrongDeposit));
    let mut gas = budget_call(|_| {});
    gas.gas = Gas::from_gas(Gas::from_teragas(30).as_gas() + 1);
    assert_eq!(budget_check(gas), Err(GasOutOfRange));
    assert_eq!(
        budget_check(budget_call(|a| a["agent"] = json!(AGENT))),
        Err(MalformedArgs)
    );
    assert_eq!(
        budget_check(budget_call(|a| a["amount"] = json!("01000000"))),
        Err(MalformedArgs)
    );
    let da = unsigned(AGENT, "v9.arras-dev.testnet", vec![budget_call(|_| {})]);
    assert_eq!(
        validate_budget_authorize(&da, &request(None), &cfg()),
        Err(ContractNotServed)
    );
    let da = unsigned(AGENT, CONTRACT_V2, vec![budget_call(|_| {})]);
    assert_eq!(
        validate_budget_authorize(&da, &request(None), &cfg()),
        Err(ContractMismatch)
    );
    let mut other = budget_call(|_| {});
    other.method_name = "authorize".to_string();
    assert_eq!(budget_check(other), Err(MethodNotAllowed));
}

// ---------------------------------------------------------------------------
// The provider: gate, relay count, verify and settle, on a stub node
// ---------------------------------------------------------------------------

/// What the stub node answers.
#[derive(Clone)]
struct Node {
    /// `broadcast_tx_commit`.
    outcome: Value,
    /// `get_payment` and `get_operator` on the escrow.
    payment: Value,
    operator: Value,
    /// `get_claimable`, per account (absent: "0").
    claimable: HashMap<String, String>,
    /// The accounts `storage_balance_of` finds on USDC (the rest: `null`).
    registered: Vec<String>,
    /// `ft_balance_of` on USDC, raw.
    balance: &'static str,
    /// Every access key's nonce (the delegate actions carry DELEGATE_NONCE).
    access_nonce: u64,
}

impl Node {
    /// V-1 authorized and alive (releasable at NOW, not yet reclaimable),
    /// EM's capturer, the worker owed 870000 and registered on USDC.
    fn new(outcome: Value) -> Self {
        Self {
            outcome,
            payment: json!({"operator_id": "em", "payer": PAYER, "receiver": WORKER,
                            "amount": "1000000", "status": "Authorized",
                            "release_deadline": "1793577600000000000",
                            "reclaim_after": "1793664000000000000"}),
            operator: json!({"owner": "owner.arras-dev.testnet", "capturers": [CAPTURER],
                             "fee_bps": 1300}),
            claimable: HashMap::from([(WORKER.to_string(), "870000".to_string())]),
            registered: vec![WORKER.to_string()],
            balance: r#""1000000""#,
            access_nonce: DELEGATE_NONCE - 1,
        }
    }

    /// The same payment past both deadlines: reclaimable at NOW.
    fn reclaimable(mut self) -> Self {
        self.payment["release_deadline"] = json!(ns(NOW - 7_200).to_string());
        self.payment["reclaim_after"] = json!(ns(NOW - 3_600).to_string());
        self
    }
}

/// Unix seconds as the escrow's nanoseconds.
fn ns(secs: u64) -> u64 {
    secs * 1_000_000_000
}

/// A stub node; its URL and the count of `broadcast_tx_commit` calls: every
/// transaction the relayer would pay for.
async fn stub(node: Node) -> (String, Arc<AtomicUsize>) {
    let sent = Arc::new(AtomicUsize::new(0));
    let counter = sent.clone();
    let block = serde_json::to_value(BlockView {
        author: "validator.testnet".parse().unwrap(),
        header: BlockHeaderView::default(),
        chunks: vec![],
    })
    .unwrap();
    let app = Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let (node, block, counter) = (node.clone(), block.clone(), counter.clone());
            async move {
                let params = &req["params"];
                let view = |result: Vec<u8>| {
                    json!({"result": result, "logs": [], "block_height": 1,
                           "block_hash": "11111111111111111111111111111111"})
                };
                let args: Value = params["args_base64"]
                    .as_str()
                    .and_then(|a| {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, a).ok()
                    })
                    .and_then(|a| serde_json::from_slice(&a).ok())
                    .unwrap_or(Value::Null);
                let call = |account: &str, method: &str| {
                    params["request_type"] == "call_function"
                        && params["account_id"] == account
                        && params["method_name"] == method
                };
                let result = match req["method"].as_str() {
                    Some("broadcast_tx_commit") => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        node.outcome
                    }
                    Some("block") => block,
                    Some("query") if params["request_type"] == "view_access_key" => json!({
                        "nonce": node.access_nonce, "permission": "FullAccess",
                        "block_height": 1, "block_hash": "11111111111111111111111111111111",
                    }),
                    Some("query") if call(USDC_TESTNET, "ft_balance_of") => {
                        view(node.balance.as_bytes().to_vec())
                    }
                    Some("query") if call(USDC_TESTNET, "storage_balance_of") => {
                        let known = args["account_id"]
                            .as_str()
                            .is_some_and(|a| node.registered.iter().any(|r| r == a));
                        view(if known {
                            br#"{"total":"1250000000000000000000","available":"0"}"#.to_vec()
                        } else {
                            b"null".to_vec()
                        })
                    }
                    Some("query")
                        if call(CONTRACT, "get_payment") && args["payment_id"] == V1_ID =>
                    {
                        view(node.payment.to_string().into_bytes())
                    }
                    Some("query") if call(CONTRACT, "get_payment") => view(b"null".to_vec()),
                    Some("query")
                        if call(CONTRACT, "get_operator") && args["operator_id"] == "em" =>
                    {
                        view(node.operator.to_string().into_bytes())
                    }
                    Some("query") if call(CONTRACT, "get_operator") => view(b"null".to_vec()),
                    Some("query") if call(CONTRACT, "get_claimable") => {
                        let owed = args["account"]
                            .as_str()
                            .and_then(|a| node.claimable.get(a).cloned())
                            .unwrap_or_else(|| "0".to_string());
                        view(json!(owed).to_string().into_bytes())
                    }
                    _ => {
                        return Json(json!({"jsonrpc": "2.0", "id": req["id"], "error": {
                            "code": -32601, "message": format!("unexpected call {req}"),
                        }}))
                    }
                };
                Json(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), sent)
}

async fn escrow_provider_with(
    node: Node,
    config: NearEscrowConfig,
) -> (NearProvider, Arc<AtomicUsize>) {
    escrow_provider_on(node, config, Arc::new(MemoryNonceStore::new())).await
}

/// A provider whose sender hours are counted in `store`: providers given the
/// same store share the count, as tasks share the claim store.
async fn escrow_provider_on(
    node: Node,
    config: NearEscrowConfig,
    store: Arc<dyn NonceStore>,
) -> (NearProvider, Arc<AtomicUsize>) {
    let (url, sent) = stub(node).await;
    let provider = NearProvider::try_new(
        SecretKey::from_random(KeyType::ED25519),
        RELAYER.to_string(),
        url,
        Network::NearTestnet,
    )
    .unwrap()
    .with_escrow(config, store, Arc::new(|| NOW));
    (provider, sent)
}

async fn escrow_provider(node: Node) -> (NearProvider, Arc<AtomicUsize>) {
    escrow_provider_with(node, cfg()).await
}

/// A lifecycle request: `extra.nearEscrow.action` names the method, no amount.
fn lifecycle_request(signed: &SignedDelegateAction, action: &str) -> EscrowRequest {
    EscrowRequest {
        network: Network::NearTestnet,
        signed_delegate_action: b64(&borsh::to_vec(signed).unwrap()),
        asset: None,
        amount: Some(0),
        extra: NearEscrowExtra {
            contract: Some(CONTRACT.to_string()),
            action: Some(action.to_string()),
            ..NearEscrowExtra::default()
        },
    }
}

fn refused(response: &SettleResponse) -> &str {
    assert!(!response.success, "expected a refusal, got {response:?}");
    match &response.error_reason {
        Some(FacilitatorErrorReason::FreeForm(token)) => token,
        other => panic!("expected a reason token, got {other:?}"),
    }
}

fn assert_valid(response: &VerifyResponse, payer: &str) {
    match response {
        VerifyResponse::Valid { payer: got } => {
            assert_eq!(got, &MixedAddress::Near(payer.to_string()))
        }
        other => panic!("expected a valid verdict, got {other:?}"),
    }
}

fn invalid_reason(response: &VerifyResponse) -> String {
    match response {
        VerifyResponse::Invalid {
            reason: FacilitatorErrorReason::FreeForm(token),
            ..
        } => token.clone(),
        other => panic!("expected an invalid verdict, got {other:?}"),
    }
}

#[tokio::test]
async fn settle_pays_an_authorization_the_escrow_accepted() {
    let (provider, sent) = escrow_provider(Node::new(authorized_outcome())).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(
        response.transaction,
        Some(TransactionHash::Near(
            TX_HASH.parse::<CryptoHash>().unwrap().0
        ))
    );
    assert_eq!(response.payer, MixedAddress::Near(PAYER.to_string()));
    let extensions = response.extensions.unwrap();
    assert_eq!(
        extensions["nearEscrow"],
        json!({"action": "authorize", "contract": CONTRACT, "paymentId": V1_ID})
    );
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    let verdict = provider
        .verify_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_valid(&verdict, PAYER);
}

#[tokio::test]
async fn settle_of_w13_is_success_false() {
    // W-13, retold between this module's accounts: the escrow refused, the
    // token refunded, "0" used.
    let (provider, sent) =
        escrow_provider(Node::new(w13_relayed(USDC_TESTNET, PAYER, CONTRACT))).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert!(!response.success);
    assert_eq!(response.transaction, None);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn settle_without_c_is_unconfirmed_with_the_hash() {
    let mut outcome = authorized_outcome();
    receipt_mut(&mut outcome, &rid("on"))["logs"] = json!([]);
    let (provider, _) = escrow_provider(Node::new(outcome)).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    match provider.settle_escrow(&request(Some(&signed))).await {
        Err(FacilitatorLocalError::SettlementUnconfirmed(tx, Network::NearTestnet)) => {
            assert_eq!(
                tx,
                TransactionHash::Near(TX_HASH.parse::<CryptoHash>().unwrap().0)
            );
        }
        other => panic!("expected SettlementUnconfirmed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_payer_short_of_the_amount_is_refused_before_spending() {
    let mut node = Node::new(authorized_outcome());
    node.balance = r#""999999""#;
    let (provider, sent) = escrow_provider(node).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "insufficient_funds");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

/// An authorization of 0 that is otherwise the 402's own (its payment id
/// derived for 0), from a payer holding no USDC: refused before any read
/// (with the delegate nonce used, a read would answer otherwise), nothing
/// relayed.
#[tokio::test]
async fn an_authorization_of_zero_is_refused_before_spending() {
    let mut call = authorize_call();
    with_args(&mut call, |args| args["amount"] = json!("0"));
    let signed = sign(PAYER, USDC_TESTNET, vec![call]);
    let mut req = request(Some(&signed));
    req.amount = Some(0);
    req.extra.payment_id = Some(compute_payment_id(&terms_v1(), USDC_TESTNET, PAYER, 0));
    for nonce in [DELEGATE_NONCE - 1, DELEGATE_NONCE] {
        let mut node = Node::new(authorized_outcome());
        node.balance = r#""0""#;
        node.access_nonce = nonce;
        let (provider, sent) = escrow_provider(node).await;
        let verdict = provider.verify_escrow(&req).await.unwrap();
        assert_eq!(invalid_reason(&verdict), "near_escrow_zero_amount");
        let response = provider.settle_escrow(&req).await.unwrap();
        assert_eq!(refused(&response), "near_escrow_zero_amount");
        assert_eq!(sent.load(Ordering::SeqCst), 0);
    }
}

/// The canonical msg of `terms`.
fn msg_of(terms: &Terms) -> String {
    serde_json::to_string(&AuthorizeMsg {
        v: 1,
        action: "authorize".to_string(),
        operator_id: terms.operator_id.clone(),
        receiver: terms.receiver.to_string(),
        release_deadline: terms.release_deadline.to_string(),
        reclaim_after: terms.reclaim_after.to_string(),
        fee_bps_max: terms.fee_bps_max,
        terms_hash: hex::encode(terms.terms_hash),
        salt: hex::encode(terms.salt),
    })
    .unwrap()
}

/// PAYER's authorization of AMOUNT on `terms` and the 402 that asks for it,
/// its payment id derived: the two agree, whatever the terms say.
fn authorization_on(terms: &Terms) -> (SignedDelegateAction, EscrowRequest) {
    let msg = msg_of(terms);
    let args = json!({"receiver_id": CONTRACT, "amount": AMOUNT.to_string(), "msg": msg});
    let call = function_call("ft_transfer_call", args.to_string().into_bytes(), 100, 1);
    let signed = sign(PAYER, USDC_TESTNET, vec![call]);
    let mut req = request(Some(&signed));
    req.extra.msg = Some(msg);
    req.extra.payment_id = Some(compute_payment_id(terms, USDC_TESTNET, PAYER, AMOUNT));
    (signed, req)
}

/// What the escrow refuses on the msg's terms alone is refused before any
/// read (with the delegate nonce used, a read would answer otherwise): E15, a
/// receiver that is the payer or the escrow; E11, `now < release_deadline <
/// reclaim_after` failing with `now` taken a minute ahead. A release deadline
/// one nanosecond past that minute passes.
#[tokio::test]
async fn terms_the_escrow_refuses_are_refused_before_spending() {
    assert_eq!(msg_of(&terms_v1()), v1_msg());
    let in_a_minute = ns(NOW + 60);
    let v1 = terms_v1();
    let cases: Vec<(Terms, &str)> = vec![
        (
            Terms {
                receiver: account(PAYER),
                ..terms_v1()
            },
            "near_escrow_receiver_invalid",
        ),
        (
            Terms {
                receiver: account(CONTRACT),
                ..terms_v1()
            },
            "near_escrow_receiver_invalid",
        ),
        (
            Terms {
                release_deadline: ns(NOW - 1),
                ..terms_v1()
            },
            "near_escrow_deadlines_invalid",
        ),
        (
            Terms {
                release_deadline: in_a_minute,
                ..terms_v1()
            },
            "near_escrow_deadlines_invalid",
        ),
        (
            Terms {
                reclaim_after: v1.release_deadline,
                ..terms_v1()
            },
            "near_escrow_deadlines_invalid",
        ),
        (
            Terms {
                reclaim_after: v1.release_deadline - 1,
                ..terms_v1()
            },
            "near_escrow_deadlines_invalid",
        ),
    ];
    let mut node = Node::new(authorized_outcome());
    node.access_nonce = DELEGATE_NONCE;
    let (provider, sent) = escrow_provider(node).await;
    for (terms, reason) in &cases {
        let (_, req) = authorization_on(terms);
        let verdict = provider.verify_escrow(&req).await.unwrap();
        assert_eq!(invalid_reason(&verdict), *reason, "{terms:?}");
        let response = provider.settle_escrow(&req).await.unwrap();
        assert_eq!(refused(&response), *reason, "{terms:?}");
    }
    assert_eq!(sent.load(Ordering::SeqCst), 0);

    let (provider, _) = escrow_provider(Node::new(authorized_outcome())).await;
    let (_, req) = authorization_on(&Terms {
        release_deadline: in_a_minute + 1,
        ..terms_v1()
    });
    assert_valid(&provider.verify_escrow(&req).await.unwrap(), PAYER);
}

/// Every relay counts against its sender's hour, an authorization the escrow
/// took as much as one it refused: with room for one, the next authorization
/// is refused before anything is sent.
#[tokio::test]
async fn every_authorization_relayed_counts_against_its_sender() {
    let one = NearEscrowConfig {
        sender_per_hour: 1,
        ..cfg()
    };
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let (provider, sent) = escrow_provider_with(Node::new(authorized_outcome()), one).await;
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
    let verdict = provider
        .verify_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(invalid_reason(&verdict), "near_escrow_sender_rate_limited");
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_rate_limited");
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

/// Authorizations in flight together are counted before any is relayed:
/// with room for one relay this hour, three at once (W-13, which the escrow
/// refuses) relay one, and the other two, and a fourth after them, are
/// refused before anything is sent.
#[tokio::test]
async fn an_authorization_burst_never_relays_more_than_the_hour_allows() {
    let one = NearEscrowConfig {
        sender_per_hour: 1,
        ..cfg()
    };
    let refusing = Node::new(w13_relayed(USDC_TESTNET, PAYER, CONTRACT));
    let (provider, sent) = escrow_provider_with(refusing, one).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let req = request(Some(&signed));
    let (a, b, c) = tokio::join!(
        provider.settle_escrow(&req),
        provider.settle_escrow(&req),
        provider.settle_escrow(&req),
    );
    let limited = [a.unwrap(), b.unwrap(), c.unwrap()]
        .iter()
        .filter(|response| {
            matches!(
                &response.error_reason,
                Some(FacilitatorErrorReason::FreeForm(token))
                    if token == "near_escrow_sender_rate_limited"
            )
        })
        .count();
    assert_eq!(limited, 2);
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    let response = provider.settle_escrow(&req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_rate_limited");
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

/// With room for two relays this hour, `req` relayed against `node`, whose
/// outcome ends `success: false`, takes one slot and not two: it is relayed a
/// second time, and the third is refused before anything is sent.
async fn a_relay_that_ends_success_false_takes_one_slot(node: Node, req: &EscrowRequest) {
    let two = NearEscrowConfig {
        sender_per_hour: 2,
        ..cfg()
    };
    let (provider, sent) = escrow_provider_with(node, two).await;
    for relayed in 1..=2 {
        let response = provider.settle_escrow(req).await.unwrap();
        assert!(!response.success, "{response:?}");
        assert_eq!(sent.load(Ordering::SeqCst), relayed);
    }
    let response = provider.settle_escrow(req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_rate_limited");
    assert_eq!(sent.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_authorization_that_ends_success_false_takes_one_slot() {
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    a_relay_that_ends_success_false_takes_one_slot(
        Node::new(w13_relayed(USDC_TESTNET, PAYER, CONTRACT)),
        &request(Some(&signed)),
    )
    .await;
}

#[tokio::test]
async fn a_lifecycle_call_that_ends_success_false_takes_one_slot() {
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    a_relay_that_ends_success_false_takes_one_slot(
        Node::new(lifecycle_outcome(CAPTURER, panicked("ARRAS_E23"), vec![])),
        &lifecycle_request(&release, "release"),
    )
    .await;
}

#[tokio::test]
async fn a_delegate_action_whose_nonce_was_used_is_refused_before_spending() {
    let mut node = Node::new(authorized_outcome());
    node.access_nonce = DELEGATE_NONCE;
    let (provider, sent) = escrow_provider(node).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_delegate_not_executable");
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    let response = provider
        .settle_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_delegate_not_executable");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_bad_signature_or_another_asset_is_refused_before_spending() {
    let (provider, sent) = escrow_provider(Node::new(authorized_outcome())).await;
    let mut forged = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    forged.delegate_action.nonce += 1;
    let response = provider
        .settle_escrow(&request(Some(&forged)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_invalid_signature");
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let mut req = request(Some(&signed));
    req.asset = Some(USDC_MAINNET.to_string());
    let response = provider.settle_escrow(&req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_invalid_asset");
    let mut req = request(Some(&signed));
    req.asset = None;
    let response = provider.settle_escrow(&req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_invalid_asset");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_lifecycle_delegate_never_answers_a_payment_requirement() {
    // A withdrawal the worker is entitled to, offered against the 402.
    let (provider, sent) = escrow_provider(Node::new(withdraw_outcome(
        WORKER,
        WORKER,
        true,
        returns("\"870000\""),
    )))
    .await;
    let signed = sign(WORKER, CONTRACT, vec![lifecycle("withdraw", json!({}), 50)]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert!(!response.success);
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    // And a lifecycle requirement never carries an amount.
    let mut req = lifecycle_request(&signed, "withdraw");
    req.amount = Some(1);
    let response = provider.settle_escrow(&req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_lifecycle_with_amount");
    // Nor another method than the one it names, nor another escrow.
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "withdraw_for"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_action_mismatch");
    let mut req = lifecycle_request(&signed, "withdraw");
    req.extra.contract = Some(CONTRACT_V2.to_string());
    let response = provider.settle_escrow(&req).await.unwrap();
    assert_eq!(refused(&response), "near_escrow_contract_mismatch");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

/// Each lifecycle method, by a sender the gate lets through, against an
/// outcome where the escrow's receipt failed: `success: false`. Then the same
/// call against the outcome with its event: `success: true`.
#[tokio::test]
async fn each_lifecycle_method_with_its_receipt_failed_is_success_false() {
    let cases: Vec<(&str, &str, FunctionCallAction, Value, Value)> = vec![
        (
            "release",
            CAPTURER,
            release_call(),
            lifecycle_outcome(CAPTURER, panicked("ARRAS_E23"), vec![]),
            lifecycle_outcome(
                CAPTURER,
                returns(""),
                vec![arras("released", json!({"payment_id": V1_ID}))],
            ),
        ),
        (
            "refund",
            CAPTURER,
            lifecycle("refund", json!({"payment_id": V1_ID}), 30),
            lifecycle_outcome(CAPTURER, panicked("ARRAS_E22"), vec![]),
            lifecycle_outcome(
                CAPTURER,
                returns(""),
                vec![arras("refunded", json!({"payment_id": V1_ID}))],
            ),
        ),
        (
            "reclaim",
            PAYER,
            lifecycle("reclaim", json!({"payment_id": V1_ID}), 30),
            lifecycle_outcome(PAYER, panicked("ARRAS_E27"), vec![]),
            lifecycle_outcome(
                PAYER,
                returns(""),
                vec![arras("reclaimed", json!({"payment_id": V1_ID}))],
            ),
        ),
        (
            "withdraw",
            WORKER,
            lifecycle("withdraw", json!({}), 50),
            lifecycle_outcome(WORKER, panicked("ARRAS_E30"), vec![]),
            withdraw_outcome(WORKER, WORKER, true, returns("\"870000\"")),
        ),
        (
            "withdraw_for",
            CAPTURER,
            lifecycle("withdraw_for", json!({"account": WORKER}), 50),
            lifecycle_outcome(CAPTURER, panicked("ARRAS_E30"), vec![]),
            withdraw_outcome(CAPTURER, WORKER, true, returns("\"870000\"")),
        ),
    ];
    for (method, sender, call, failed, done) in cases {
        let signed = sign(sender, CONTRACT, vec![call]);
        let node = |outcome| match method {
            "reclaim" => Node::new(outcome).reclaimable(),
            _ => Node::new(outcome),
        };
        let (provider, sent) = escrow_provider(node(failed)).await;
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert!(!response.success, "{method}: {response:?}");
        assert_eq!(response.transaction, None, "{method}");
        assert_eq!(sent.load(Ordering::SeqCst), 1, "{method}");

        let (provider, _) = escrow_provider(node(done)).await;
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert!(response.success, "{method}: {response:?}");
        assert_eq!(
            response.extensions.unwrap()["nearEscrow"]["action"],
            json!(method),
            "{method}"
        );
    }
}

/// v2's budget path is validated but not relayed until v2 exposes a budget
/// view: an `authorize_from_budget` that answers the 402 exactly, against an
/// escrow that would log `authorized`, is refused before any read (the node
/// says the delegate nonce is used, so a read would answer otherwise).
#[tokio::test]
async fn an_authorize_from_budget_is_not_relayed() {
    let done = lifecycle_outcome(AGENT, returns(""), vec![authorized_event(V1_ID)]);
    for nonce in [DELEGATE_NONCE - 1, DELEGATE_NONCE] {
        let mut node = Node::new(done.clone());
        node.access_nonce = nonce;
        let (provider, sent) = escrow_provider(node).await;
        let signed = sign(AGENT, CONTRACT, vec![budget_call(|_| {})]);
        assert!(validate_budget_authorize(
            &signed.delegate_action,
            &request(Some(&signed)),
            &cfg()
        )
        .is_ok());
        let verdict = provider
            .verify_escrow(&request(Some(&signed)))
            .await
            .unwrap();
        assert_eq!(invalid_reason(&verdict), "near_escrow_method_not_allowed");
        let response = provider
            .settle_escrow(&request(Some(&signed)))
            .await
            .unwrap();
        assert_eq!(refused(&response), "near_escrow_method_not_allowed");
        assert_eq!(sent.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn a_release_from_someone_not_a_capturer_is_not_relayed() {
    let done = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    let (provider, sent) = escrow_provider(Node::new(done)).await;
    // The receiver, the payer, the operator's owner and a third party.
    for sender in [WORKER, PAYER, "owner.arras-dev.testnet", "thief.testnet"] {
        let signed = sign(sender, CONTRACT, vec![release_call()]);
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, "release"))
            .await
            .unwrap();
        assert_eq!(refused(&response), "near_escrow_not_capturer", "{sender}");
        let verdict = provider
            .verify_escrow(&lifecycle_request(&signed, "release"))
            .await
            .unwrap();
        assert_eq!(
            invalid_reason(&verdict),
            "near_escrow_not_capturer",
            "{sender}"
        );
    }
    let signed = sign(
        WORKER,
        CONTRACT,
        vec![lifecycle("refund", json!({"payment_id": V1_ID}), 30)],
    );
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "refund"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_not_capturer");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    // The capturer is.
    let signed = sign(CAPTURER, CONTRACT, vec![release_call()]);
    let verdict = provider
        .verify_escrow(&lifecycle_request(&signed, "release"))
        .await
        .unwrap();
    assert_valid(&verdict, CAPTURER);
}

#[tokio::test]
async fn a_reclaim_from_someone_not_the_payer_is_not_relayed() {
    let (provider, sent) =
        escrow_provider(Node::new(lifecycle_outcome(PAYER, returns(""), vec![]))).await;
    for sender in [CAPTURER, WORKER] {
        let signed = sign(
            sender,
            CONTRACT,
            vec![lifecycle("reclaim", json!({"payment_id": V1_ID}), 30)],
        );
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, "reclaim"))
            .await
            .unwrap();
        assert_eq!(refused(&response), "near_escrow_not_payer", "{sender}");
    }
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_withdrawal_with_nothing_claimable_is_not_relayed() {
    let (provider, sent) = escrow_provider(Node::new(withdraw_outcome(
        WORKER,
        WORKER,
        true,
        returns("\"870000\""),
    )))
    .await;
    // PAYER is owed nothing.
    let signed = sign(PAYER, CONTRACT, vec![lifecycle("withdraw", json!({}), 50)]);
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "withdraw"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_nothing_claimable");
    let signed = sign(
        CAPTURER,
        CONTRACT,
        vec![lifecycle("withdraw_for", json!({"account": PAYER}), 50)],
    );
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "withdraw_for"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_nothing_claimable");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    // Anyone may have a withdrawal FOR the worker relayed: it pays the worker.
    let signed = sign(
        "anyone.testnet",
        CONTRACT,
        vec![lifecycle("withdraw_for", json!({"account": WORKER}), 50)],
    );
    let verdict = provider
        .verify_escrow(&lifecycle_request(&signed, "withdraw_for"))
        .await
        .unwrap();
    assert!(
        matches!(verdict, VerifyResponse::Valid { .. }),
        "{verdict:?}"
    );
}

/// The escrow pays a withdrawal with an `ft_transfer` on USDC, which fails
/// for an account the token does not know (`withdrawn` with `ok: false`,
/// every time): such a withdrawal is not relayed, for the account itself or
/// for anyone on its behalf.
#[tokio::test]
async fn a_withdrawal_to_an_account_the_token_does_not_know_is_not_relayed() {
    const NEWCOMER: &str = "newcomer.testnet";
    let mut node = Node::new(withdraw_outcome(
        NEWCOMER,
        NEWCOMER,
        true,
        returns("\"500000\""),
    ));
    node.claimable
        .insert(NEWCOMER.to_string(), "500000".to_string());
    let (provider, sent) = escrow_provider(node.clone()).await;
    for (sender, method, call) in [
        (NEWCOMER, "withdraw", lifecycle("withdraw", json!({}), 50)),
        // Signed by an account the token knows: the one paid is what counts.
        (
            WORKER,
            "withdraw_for",
            lifecycle("withdraw_for", json!({ "account": NEWCOMER }), 50),
        ),
    ] {
        let signed = sign(sender, CONTRACT, vec![call]);
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert_eq!(
            refused(&response),
            "near_escrow_account_not_registered",
            "{method}"
        );
        let verdict = provider
            .verify_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert_eq!(
            invalid_reason(&verdict),
            "near_escrow_account_not_registered",
            "{method}"
        );
    }
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    // Registered, the same withdrawal is relayed.
    node.registered.push(NEWCOMER.to_string());
    let (provider, sent) = escrow_provider(node).await;
    let signed = sign(
        NEWCOMER,
        CONTRACT,
        vec![lifecycle("withdraw", json!({}), 50)],
    );
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "withdraw"))
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_unknown_payment_or_operator_is_not_relayed() {
    let (provider, sent) =
        escrow_provider(Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]))).await;
    let signed = sign(
        CAPTURER,
        CONTRACT,
        vec![lifecycle("release", json!({"payment_id": V2_ID}), 30)],
    );
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_payment_not_found");

    // An operator the facilitator serves that the escrow does not know.
    let mut node = Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]));
    node.payment["operator_id"] = json!("kk");
    let (provider, sent_too) = escrow_provider(node).await;
    let signed = sign(CAPTURER, CONTRACT, vec![release_call()]);
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_operator_not_found");
    assert_eq!(
        sent.load(Ordering::SeqCst) + sent_too.load(Ordering::SeqCst),
        0
    );
}

/// `NEAR_ESCROW_OPERATOR_IDS` is the facilitator's scope for the lifecycle
/// too: the capturer of an operator it does not serve has no release or
/// refund relayed, before the operator is even read.
#[tokio::test]
async fn a_lifecycle_call_for_an_operator_not_served_is_refused() {
    let done = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    let kk_only = NearEscrowConfig {
        operator_ids: vec!["kk".to_string()],
        ..cfg()
    };
    let (provider, sent) = escrow_provider_with(Node::new(done.clone()), kk_only).await;
    let refund = lifecycle("refund", json!({"payment_id": V1_ID}), 30);
    for (method, call) in [("release", release_call()), ("refund", refund)] {
        let signed = sign(CAPTURER, CONTRACT, vec![call]);
        let response = provider
            .settle_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert_eq!(
            refused(&response),
            "near_escrow_operator_not_served",
            "{method}"
        );
        let verdict = provider
            .verify_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert_eq!(
            invalid_reason(&verdict),
            "near_escrow_operator_not_served",
            "{method}"
        );
    }
    assert_eq!(sent.load(Ordering::SeqCst), 0);
    // The operator unknown to the escrow is not served either: refused
    // without asking the escrow for it.
    let mut node = Node::new(done);
    node.payment["operator_id"] = json!("gone");
    let (provider, sent) = escrow_provider(node).await;
    let signed = sign(CAPTURER, CONTRACT, vec![release_call()]);
    let response = provider
        .settle_escrow(&lifecycle_request(&signed, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_operator_not_served");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

/// The escrow releases, refunds and reclaims only a live payment
/// (`Authorized` or `PartiallyReleased`); a closed one is not relayed, and a
/// payment answered without its status relays nothing.
#[tokio::test]
async fn a_reclaim_of_a_closed_payment_is_not_relayed() {
    let reclaim = sign(
        PAYER,
        CONTRACT,
        vec![lifecycle("reclaim", json!({"payment_id": V1_ID}), 30)],
    );
    for status in ["Released", "Refunded", "Reclaimed"] {
        let mut node = Node::new(lifecycle_outcome(PAYER, returns(""), vec![])).reclaimable();
        node.payment["status"] = json!(status);
        let (provider, sent) = escrow_provider(node).await;
        let response = provider
            .settle_escrow(&lifecycle_request(&reclaim, "reclaim"))
            .await
            .unwrap();
        assert_eq!(
            refused(&response),
            "near_escrow_payment_not_open",
            "{status}"
        );
        assert_eq!(sent.load(Ordering::SeqCst), 0, "{status}");
    }
    let mut node = Node::new(lifecycle_outcome(PAYER, returns(""), vec![])).reclaimable();
    node.payment["status"] = json!("PartiallyReleased");
    let (provider, _) = escrow_provider(node).await;
    let verdict = provider
        .verify_escrow(&lifecycle_request(&reclaim, "reclaim"))
        .await
        .unwrap();
    assert_valid(&verdict, PAYER);

    let mut node = Node::new(lifecycle_outcome(PAYER, returns(""), vec![])).reclaimable();
    node.payment.as_object_mut().unwrap().remove("status");
    let (provider, sent) = escrow_provider(node).await;
    assert!(provider
        .settle_escrow(&lifecycle_request(&reclaim, "reclaim"))
        .await
        .is_err());
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_release_or_refund_of_a_closed_payment_is_not_relayed() {
    let refund = lifecycle("refund", json!({"payment_id": V1_ID}), 30);
    for (method, call) in [("release", release_call()), ("refund", refund)] {
        let signed = sign(CAPTURER, CONTRACT, vec![call]);
        for status in ["Released", "Refunded", "Reclaimed"] {
            let mut node = Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]));
            node.payment["status"] = json!(status);
            let (provider, sent) = escrow_provider(node).await;
            let response = provider
                .settle_escrow(&lifecycle_request(&signed, method))
                .await
                .unwrap();
            assert_eq!(
                refused(&response),
                "near_escrow_payment_not_open",
                "{method} {status}"
            );
            assert_eq!(sent.load(Ordering::SeqCst), 0, "{method} {status}");
        }
        let mut node = Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]));
        node.payment["status"] = json!("PartiallyReleased");
        let (provider, _) = escrow_provider(node).await;
        let verdict = provider
            .verify_escrow(&lifecycle_request(&signed, method))
            .await
            .unwrap();
        assert_valid(&verdict, CAPTURER);
    }
}

/// The escrow reclaims from `reclaim_after` on, judged at its block's time:
/// the facilitator relays a reclaim only a minute past it.
#[tokio::test]
async fn a_reclaim_before_reclaim_after_is_not_relayed() {
    let reclaim = sign(
        PAYER,
        CONTRACT,
        vec![lifecycle("reclaim", json!({"payment_id": V1_ID}), 30)],
    );
    for reclaim_after in [
        ns(NOW - 60) + 1,
        ns(NOW - 59),
        ns(NOW),
        1_793_664_000_000_000_000,
    ] {
        let mut node = Node::new(lifecycle_outcome(PAYER, returns(""), vec![])).reclaimable();
        node.payment["reclaim_after"] = json!(reclaim_after.to_string());
        let (provider, sent) = escrow_provider(node).await;
        let response = provider
            .settle_escrow(&lifecycle_request(&reclaim, "reclaim"))
            .await
            .unwrap();
        assert_eq!(
            refused(&response),
            "near_escrow_reclaim_not_yet",
            "{reclaim_after}"
        );
        assert_eq!(sent.load(Ordering::SeqCst), 0, "{reclaim_after}");
    }
    let mut node = Node::new(lifecycle_outcome(PAYER, returns(""), vec![])).reclaimable();
    node.payment["reclaim_after"] = json!(ns(NOW - 60).to_string());
    let (provider, _) = escrow_provider(node).await;
    let verdict = provider
        .verify_escrow(&lifecycle_request(&reclaim, "reclaim"))
        .await
        .unwrap();
    assert_valid(&verdict, PAYER);
}

/// The escrow releases only before `release_deadline`, judged at its block's
/// time: the facilitator relays a release only with a minute to spare. A
/// refund has no deadline.
#[tokio::test]
async fn a_release_after_its_deadline_is_not_relayed() {
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    for release_deadline in [ns(NOW + 60), ns(NOW), ns(NOW - 1)] {
        let mut node = Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]));
        node.payment["release_deadline"] = json!(release_deadline.to_string());
        let (provider, sent) = escrow_provider(node).await;
        let response = provider
            .settle_escrow(&lifecycle_request(&release, "release"))
            .await
            .unwrap();
        assert_eq!(
            refused(&response),
            "near_escrow_release_deadline_passed",
            "{release_deadline}"
        );
        assert_eq!(sent.load(Ordering::SeqCst), 0, "{release_deadline}");
    }
    let mut node = Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![]));
    node.payment["release_deadline"] = json!((ns(NOW + 60) + 1).to_string());
    let (provider, _) = escrow_provider(node).await;
    let verdict = provider
        .verify_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_valid(&verdict, CAPTURER);

    let refund = sign(
        CAPTURER,
        CONTRACT,
        vec![lifecycle("refund", json!({"payment_id": V1_ID}), 30)],
    );
    let (provider, _) =
        escrow_provider(Node::new(lifecycle_outcome(CAPTURER, returns(""), vec![])).reclaimable())
            .await;
    let verdict = provider
        .verify_escrow(&lifecycle_request(&refund, "refund"))
        .await
        .unwrap();
    assert_valid(&verdict, CAPTURER);
}

#[tokio::test]
async fn a_sender_is_relayed_at_most_sender_per_hour_times() {
    let done = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    let config = NearEscrowConfig {
        sender_per_hour: 2,
        ..cfg()
    };
    let (provider, sent) = escrow_provider_with(Node::new(done), config).await;
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    for _ in 0..2 {
        let response = provider
            .settle_escrow(&lifecycle_request(&release, "release"))
            .await
            .unwrap();
        assert!(response.success, "{response:?}");
    }
    let response = provider
        .settle_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_rate_limited");
    let verdict = provider
        .verify_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_eq!(invalid_reason(&verdict), "near_escrow_sender_rate_limited");
    assert_eq!(sent.load(Ordering::SeqCst), 2);
    // Another sender has an hour of its own.
    let withdraw_for = sign(
        "anyone.testnet",
        CONTRACT,
        vec![lifecycle("withdraw_for", json!({"account": WORKER}), 50)],
    );
    let verdict = provider
        .verify_escrow(&lifecycle_request(&withdraw_for, "withdraw_for"))
        .await
        .unwrap();
    assert!(
        matches!(verdict, VerifyResponse::Valid { .. }),
        "{verdict:?}"
    );
}

#[tokio::test]
async fn verify_takes_no_relay_slot() {
    let done = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    let config = NearEscrowConfig {
        sender_per_hour: 1,
        ..cfg()
    };
    let (provider, _) = escrow_provider_with(Node::new(done), config).await;
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    for _ in 0..3 {
        let verdict = provider
            .verify_escrow(&lifecycle_request(&release, "release"))
            .await
            .unwrap();
        assert!(
            matches!(verdict, VerifyResponse::Valid { .. }),
            "{verdict:?}"
        );
    }
    let response = provider
        .settle_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
}

/// The same for an authorization: with room for one relay, three verifies
/// are valid and the settle after them is relayed.
#[tokio::test]
async fn verify_of_an_authorization_takes_no_relay_slot() {
    let one = NearEscrowConfig {
        sender_per_hour: 1,
        ..cfg()
    };
    let (provider, sent) = escrow_provider_with(Node::new(authorized_outcome()), one).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    for _ in 0..3 {
        let verdict = provider
            .verify_escrow(&request(Some(&signed)))
            .await
            .unwrap();
        assert_valid(&verdict, PAYER);
    }
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

/// A request refused on what the chain answers takes no slot: settle takes
/// it after every read. With room for one relay this hour, PAYER's reclaim
/// the gate refuses (before `reclaim_after`), its delegate whose nonce was
/// used and its authorization without the USDC to pay each leave the hour to
/// the authorization that follows. The last three run on three providers
/// sharing one claim store, as tasks do.
#[tokio::test]
async fn a_request_refused_after_reading_takes_no_slot() {
    let one = NearEscrowConfig {
        sender_per_hour: 1,
        ..cfg()
    };
    let authorization = request(Some(&sign(PAYER, USDC_TESTNET, vec![authorize_call()])));

    let (provider, sent) = escrow_provider_with(Node::new(authorized_outcome()), one.clone()).await;
    let reclaim = sign(
        PAYER,
        CONTRACT,
        vec![lifecycle("reclaim", json!({"payment_id": V1_ID}), 30)],
    );
    let response = provider
        .settle_escrow(&lifecycle_request(&reclaim, "reclaim"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_reclaim_not_yet");
    let response = provider.settle_escrow(&authorization).await.unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(sent.load(Ordering::SeqCst), 1);

    let store: Arc<dyn NonceStore> = Arc::new(MemoryNonceStore::new());
    let mut used = Node::new(authorized_outcome());
    used.access_nonce = DELEGATE_NONCE;
    let mut empty = Node::new(authorized_outcome());
    empty.balance = r#""0""#;
    for (node, reason) in [
        (used, "near_escrow_delegate_not_executable"),
        (empty, "insufficient_funds"),
    ] {
        let (provider, sent) = escrow_provider_on(node, one.clone(), store.clone()).await;
        let response = provider.settle_escrow(&authorization).await.unwrap();
        assert_eq!(refused(&response), reason);
        assert_eq!(sent.load(Ordering::SeqCst), 0);
    }
    let (provider, sent) = escrow_provider_on(Node::new(authorized_outcome()), one, store).await;
    let response = provider.settle_escrow(&authorization).await.unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(sent.load(Ordering::SeqCst), 1);
}

/// A claim store that cannot be read or written.
#[derive(Debug)]
struct Unreachable;

#[async_trait::async_trait]
impl NonceStore for Unreachable {
    async fn check_and_mark_used(&self, _: &str, _: u64) -> Result<(), NonceStoreError> {
        Err(NonceStoreError::ReadError("unreachable".to_string()))
    }
    async fn is_used(&self, _: &str) -> Result<bool, NonceStoreError> {
        Err(NonceStoreError::ReadError("unreachable".to_string()))
    }
    async fn health_check(&self) -> Result<(), NonceStoreError> {
        Err(NonceStoreError::ReadError("unreachable".to_string()))
    }
    fn store_type(&self) -> &'static str {
        "unreachable"
    }
}

#[tokio::test]
async fn an_unreadable_relay_count_relays_nothing() {
    let done = lifecycle_outcome(
        CAPTURER,
        returns(""),
        vec![arras("released", json!({"payment_id": V1_ID}))],
    );
    let (url, sent) = stub(Node::new(done)).await;
    let provider = NearProvider::try_new(
        SecretKey::from_random(KeyType::ED25519),
        RELAYER.to_string(),
        url,
        Network::NearTestnet,
    )
    .unwrap()
    .with_escrow(cfg(), Arc::new(Unreachable), Arc::new(|| NOW));
    let release = sign(CAPTURER, CONTRACT, vec![release_call()]);
    let response = provider
        .settle_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_count_unavailable");
    // An authorization too: whether its sender's hour is full is unknown.
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_sender_count_unavailable");
    let verdict = provider
        .verify_escrow(&lifecycle_request(&release, "release"))
        .await
        .unwrap();
    assert_eq!(
        invalid_reason(&verdict),
        "near_escrow_sender_count_unavailable"
    );
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_scheme_switched_off_relays_nothing() {
    let off = NearEscrowConfig {
        enabled: false,
        ..cfg()
    };
    let (provider, sent) = escrow_provider_with(Node::new(authorized_outcome()), off).await;
    let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
    let response = provider
        .settle_escrow(&request(Some(&signed)))
        .await
        .unwrap();
    assert_eq!(refused(&response), "near_escrow_disabled");
    assert_eq!(sent.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// /supported
// ---------------------------------------------------------------------------

#[test]
fn the_evm_escrow_extra_keeps_its_exact_json() {
    use crate::types::{EscrowSupportedInfo, EvmAddress};
    let address = |byte: u8| EvmAddress(alloy::primitives::Address::repeat_byte(byte));
    let extra = SupportedPaymentKindExtra {
        fee_payer: Some(MixedAddress::Evm(address(0x10))),
        tokens: None,
        escrow: Some(EscrowSupportedInfo {
            escrow_address: address(0x11),
            operator_address: address(0x22),
            token_collector: address(0x33),
        }),
        near_escrow: None,
    };
    let json = serde_json::to_value(&extra).unwrap();
    let text = |byte| serde_json::to_value(address(byte)).unwrap();
    assert_eq!(
        json,
        json!({
            "feePayer": text(0x10),
            "escrowAddress": text(0x11),
            "operatorAddress": text(0x22),
            "tokenCollector": text(0x33),
        })
    );
    // What clients sent before reads back the same.
    let back: SupportedPaymentKindExtra = serde_json::from_value(json.clone()).unwrap();
    assert!(back.near_escrow.is_none());
    assert_eq!(serde_json::to_value(&back).unwrap(), json);
}

#[tokio::test]
async fn the_near_escrow_kind_is_additive() {
    let (provider, _) = escrow_provider(Node::new(authorized_outcome())).await;
    let kinds = provider.supported().await.unwrap().kinds;
    assert_eq!(kinds.len(), 2);
    let exact = serde_json::to_value(&kinds[0]).unwrap();
    assert_eq!(exact["scheme"], "exact");
    assert!(exact["extra"].get("nearEscrow").is_none(), "{exact}");
    let escrow = serde_json::to_value(&kinds[1]).unwrap();
    assert_eq!(escrow["scheme"], "escrow");
    assert_eq!(escrow["network"], "near-testnet");
    assert_eq!(escrow["x402Version"], 1);
    assert_eq!(escrow["extra"]["feePayer"], RELAYER);
    assert_eq!(escrow["extra"]["tokens"], exact["extra"]["tokens"]);
    assert_eq!(
        escrow["extra"]["nearEscrow"],
        json!({
            "contracts": [CONTRACT, CONTRACT_V2],
            "operatorIds": ["em", "kk"],
            "minGasTgas": 38,
            "maxGasTgas": 100,
            "senderPerHour": 30,
        })
    );
    for evm_key in ["escrowAddress", "operatorAddress", "tokenCollector"] {
        assert!(escrow["extra"].get(evm_key).is_none(), "{evm_key}");
    }
    // Named both ways once /supported mirrors it.
    let mirrored = crate::facilitator_local::advertise_under_both_network_forms(kinds);
    assert!(mirrored
        .iter()
        .any(|k| k.scheme == Scheme::Escrow && k.network == "near:testnet"));

    // Off, or with no contract on the network: the exact kind alone.
    for config in [
        NearEscrowConfig {
            enabled: false,
            ..cfg()
        },
        NearEscrowConfig {
            contracts_testnet: vec![],
            ..cfg()
        },
    ] {
        let (provider, _) = escrow_provider_with(Node::new(authorized_outcome()), config).await;
        let kinds = provider.supported().await.unwrap().kinds;
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].scheme, Scheme::Exact);
    }
}

// ---------------------------------------------------------------------------
// The handler's cut: before the PaymentOperator gate
// ---------------------------------------------------------------------------

mod routing {
    use super::*;
    use crate::facilitator::Facilitator;
    use crate::payment_operator::test_rpc::{Providers, ESCROW_FLAGS};
    use crate::types::{SettleRequest, SupportedPaymentKindsResponse, VerifyRequest};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Extension;
    use tower::ServiceExt;

    /// A facilitator whose only chain is NEAR testnet. NEAR escrow never
    /// reaches `Facilitator::verify` / `settle`.
    #[derive(Clone)]
    struct OnNear(Arc<Providers>);

    impl HasProviderMap for OnNear {
        type Map = Providers;
        fn provider_map(&self) -> &Self::Map {
            &self.0
        }
    }

    impl Facilitator for OnNear {
        type Error = FacilitatorLocalError;
        async fn verify(&self, _: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
            Err(FacilitatorLocalError::ContractCall("not this path".into()))
        }
        async fn settle(&self, _: &SettleRequest) -> Result<SettleResponse, Self::Error> {
            Err(FacilitatorLocalError::ContractCall("not this path".into()))
        }
        async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
            Ok(SupportedPaymentKindsResponse { kinds: vec![] })
        }
    }

    async fn post(facilitator: &OnNear, path: &str, body: &Value) -> (StatusCode, Value) {
        let response = crate::handlers::verify_settle_routes::<OnNear>()
            .layer(Extension(Arc::new(
                crate::discovery::DiscoveryRegistry::new(),
            )))
            .layer(Extension(Arc::new(crate::events::EventBus::from_env())))
            .layer(Extension(
                crate::transaction_store::create_transaction_store().await,
            ))
            .with_state(facilitator.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn body(signed: &SignedDelegateAction) -> Value {
        let mut body = v1_body("near-testnet");
        body["paymentPayload"]["payload"]["signedDelegateAction"] =
            json!(b64(&borsh::to_vec(signed).unwrap()));
        body
    }

    async fn on_near(config: NearEscrowConfig) -> (OnNear, Arc<AtomicUsize>) {
        let (near, sent) = escrow_provider_with(Node::new(authorized_outcome()), config).await;
        let providers = Providers(HashMap::from([(
            Network::NearTestnet,
            NetworkProvider::Near(near),
        )]));
        (OnNear(Arc::new(providers)), sent)
    }

    /// Sets ENABLE_PAYMENT_OPERATOR for the test and puts it back.
    struct PaymentOperator {
        was: Option<String>,
        _flags: std::sync::MutexGuard<'static, ()>,
    }

    impl PaymentOperator {
        fn set(value: Option<&str>) -> Self {
            let flags = ESCROW_FLAGS.lock().unwrap_or_else(|e| e.into_inner());
            let was = std::env::var("ENABLE_PAYMENT_OPERATOR").ok();
            match value {
                Some(v) => std::env::set_var("ENABLE_PAYMENT_OPERATOR", v),
                None => std::env::remove_var("ENABLE_PAYMENT_OPERATOR"),
            }
            Self { was, _flags: flags }
        }
    }

    impl Drop for PaymentOperator {
        fn drop(&mut self) {
            match &self.was {
                Some(v) => std::env::set_var("ENABLE_PAYMENT_OPERATOR", v),
                None => std::env::remove_var("ENABLE_PAYMENT_OPERATOR"),
            }
        }
    }

    #[tokio::test]
    async fn near_escrow_runs_whatever_the_payment_operator_switch_says() {
        let (facilitator, sent) = on_near(cfg()).await;
        let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
        for switch in [None, Some("false"), Some("true")] {
            let _switch = PaymentOperator::set(switch);
            let (status, verdict) = post(&facilitator, "/verify", &body(&signed)).await;
            assert_eq!(status, StatusCode::OK, "{switch:?}: {verdict}");
            assert_eq!(verdict["isValid"], true, "{switch:?}: {verdict}");
            assert_eq!(verdict["payer"], PAYER, "{switch:?}");
        }
        let _switch = PaymentOperator::set(None);
        let (status, settled) = post(&facilitator, "/settle", &body(&signed)).await;
        assert_eq!(status, StatusCode::OK, "{settled}");
        assert_eq!(settled["success"], true, "{settled}");
        assert_eq!(settled["transaction"], TX_HASH);
        assert_eq!(settled["extensions"]["nearEscrow"]["paymentId"], V1_ID);
        assert_eq!(sent.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn near_escrow_off_answers_for_itself_and_relays_nothing() {
        let (facilitator, sent) = on_near(NearEscrowConfig {
            enabled: false,
            ..cfg()
        })
        .await;
        let signed = sign(PAYER, USDC_TESTNET, vec![authorize_call()]);
        let _switch = PaymentOperator::set(Some("true"));
        let (status, verdict) = post(&facilitator, "/verify", &body(&signed)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(verdict["isValid"], false);
        assert_eq!(verdict["invalidReason"], "near_escrow_disabled");
        let (_, settled) = post(&facilitator, "/settle", &body(&signed)).await;
        assert_eq!(settled["success"], false);
        assert_eq!(settled["errorReason"], "near_escrow_disabled");
        // Off is the answer before anything about the body is.
        let mut malformed = body(&signed);
        malformed["paymentRequirements"]["extra"] = json!({});
        let (_, verdict) = post(&facilitator, "/verify", &malformed).await;
        assert_eq!(verdict["invalidReason"], "near_escrow_disabled");
        assert_eq!(sent.load(Ordering::SeqCst), 0);
    }
}
