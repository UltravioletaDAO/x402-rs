//! The `escrow` scheme on NEAR: payments into the Arras escrow contract.
//!
//! A payer authorizes a payment by signing a NEP-366 delegate action that
//! calls `ft_transfer_call` on the network's USDC with the escrow contract as
//! the receiver and a canonical `msg` naming the terms. The facilitator relays
//! it like an `exact` payment, and then reads the outcome for the three things
//! that prove the escrow holds the payment (see [`confirm_escrow_authorize`]).
//! The escrow's lifecycle (`release`, `refund`, `reclaim`, `withdraw`,
//! `withdraw_for`) and the v2 budget path (`authorize_from_budget`) are relayed
//! the same way, each confirmed by the event the contract logs for it.
//!
//! Everything here is off unless `ENABLE_NEAR_ESCROW` is true, and it only
//! relays to the contract accounts configured for the network: never to an
//! account a request names on its own.
//!
//! Who may have a lifecycle call relayed is decided by the escrow's own state
//! (the operator's capturers, the payment's payer, a non-zero claimable
//! balance), read before anything is sent. The facilitator holds no list of
//! accounts.

use std::str::FromStr;
use std::sync::OnceLock;

use near_primitives::action::delegate::{DelegateAction, SignedDelegateAction};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Action, FunctionCallAction};
use near_primitives::types::{AccountId, Gas};
use near_primitives::views::{
    ExecutionOutcomeWithIdView, ExecutionStatusView, FinalExecutionOutcomeView,
};
use near_token::NearToken;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    logs_transfer, parse_ft_balance, settle_outcome, ExpectedTransfer, NearProvider, Nep297Event,
};
use crate::chain::{FacilitatorLocalError, NetworkProvider, NetworkProviderOps};
use crate::network::{Network, USDCDeployment};
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    FacilitatorErrorReason, MixedAddress, SettleResponse, TransactionHash, VerifyResponse,
};

// ===========================================================================
// Configuration
// ===========================================================================

/// The switch. Off unless set to `true` (or `1`).
pub const ENV_ENABLE_NEAR_ESCROW: &str = "ENABLE_NEAR_ESCROW";
/// Comma-separated escrow contract accounts on NEAR mainnet (v1 and v2).
pub const ENV_CONTRACTS_MAINNET: &str = "NEAR_ESCROW_CONTRACTS_MAINNET";
/// Comma-separated escrow contract accounts on NEAR testnet.
pub const ENV_CONTRACTS_TESTNET: &str = "NEAR_ESCROW_CONTRACTS_TESTNET";
/// Least gas, in TGas, an authorization's `ft_transfer_call` may attach.
pub const ENV_MIN_GAS_TGAS: &str = "NEAR_ESCROW_MIN_GAS_TGAS";
/// Most gas, in TGas, an authorization's `ft_transfer_call` may attach.
pub const ENV_MAX_GAS_TGAS: &str = "NEAR_ESCROW_MAX_GAS_TGAS";
/// Most lifecycle and budget relays one `sender_id` gets per UTC hour.
pub const ENV_SENDER_PER_HOUR: &str = "NEAR_ESCROW_SENDER_PER_HOUR";
/// Comma-separated operator ids (`[a-z0-9_-]{1,32}`) the facilitator relays for.
pub const ENV_OPERATOR_IDS: &str = "NEAR_ESCROW_OPERATOR_IDS";

/// The least prepaid gas an authorization passes with, plus 20 %: measured
/// 2026-11-01 in the escrow's near-workspaces sandbox against a clone of
/// Circle's testnet USDC (W-09), `ft_transfer_call` into the escrow burns
/// 8.62 TGas and needs at least 31 TGas prepaid. Less fails on the token or
/// leaves `ft_on_transfer` short, and the relayer pays the gas either way.
/// Both bounds can be recalibrated through the environment, no rebuild needed.
const DEFAULT_MIN_GAS_TGAS: u64 = 38;
/// What the escrow's testnet runbook attaches to `ft_transfer_call`.
const DEFAULT_MAX_GAS_TGAS: u64 = 100;
/// The protocol's per-receipt prepaid gas limit.
const PROTOCOL_MAX_GAS_TGAS: u64 = 300;
const DEFAULT_SENDER_PER_HOUR: u32 = 30;
const MAX_SENDER_PER_HOUR: u32 = 3_600;
const DEFAULT_OPERATOR_IDS: [&str; 2] = ["em", "kk"];

/// The effective configuration of the NEAR escrow scheme.
///
/// Read once from the environment ([`NearEscrowConfig::current`]); a value
/// that does not parse, or falls outside its range, is replaced by its default
/// with a warning, never by an error at startup. The values that shape what a
/// client may sign are published in `/supported` (`extra.nearEscrow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearEscrowConfig {
    pub enabled: bool,
    pub contracts_mainnet: Vec<AccountId>,
    pub contracts_testnet: Vec<AccountId>,
    pub min_gas: Gas,
    pub max_gas: Gas,
    pub sender_per_hour: u32,
    pub operator_ids: Vec<String>,
}

impl Default for NearEscrowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            contracts_mainnet: Vec::new(),
            contracts_testnet: Vec::new(),
            min_gas: Gas::from_teragas(DEFAULT_MIN_GAS_TGAS),
            max_gas: Gas::from_teragas(DEFAULT_MAX_GAS_TGAS),
            sender_per_hour: DEFAULT_SENDER_PER_HOUR,
            operator_ids: DEFAULT_OPERATOR_IDS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl NearEscrowConfig {
    /// The configuration the environment describes.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The configuration `get` describes (the environment, or a test's map).
    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let enabled = parse_flag(get(ENV_ENABLE_NEAR_ESCROW));
        let contracts_mainnet = parse_contracts(ENV_CONTRACTS_MAINNET, get(ENV_CONTRACTS_MAINNET));
        let contracts_testnet = parse_contracts(ENV_CONTRACTS_TESTNET, get(ENV_CONTRACTS_TESTNET));
        let min = parse_number(
            ENV_MIN_GAS_TGAS,
            get(ENV_MIN_GAS_TGAS),
            DEFAULT_MIN_GAS_TGAS,
            1,
            PROTOCOL_MAX_GAS_TGAS,
        );
        let max = parse_number(
            ENV_MAX_GAS_TGAS,
            get(ENV_MAX_GAS_TGAS),
            DEFAULT_MAX_GAS_TGAS,
            1,
            PROTOCOL_MAX_GAS_TGAS,
        );
        let (min, max) = if min > max {
            tracing::warn!(
                min_tgas = min,
                max_tgas = max,
                "NEAR escrow gas range is inverted; using the defaults for both bounds"
            );
            (DEFAULT_MIN_GAS_TGAS, DEFAULT_MAX_GAS_TGAS)
        } else {
            (min, max)
        };
        let sender_per_hour = parse_number(
            ENV_SENDER_PER_HOUR,
            get(ENV_SENDER_PER_HOUR),
            DEFAULT_SENDER_PER_HOUR,
            1,
            MAX_SENDER_PER_HOUR,
        );
        let operator_ids = parse_operator_ids(get(ENV_OPERATOR_IDS));
        Self {
            enabled,
            contracts_mainnet,
            contracts_testnet,
            min_gas: Gas::from_teragas(min),
            max_gas: Gas::from_teragas(max),
            sender_per_hour,
            operator_ids,
        }
    }

    /// The process-wide configuration, read from the environment once.
    pub fn current() -> &'static NearEscrowConfig {
        static CURRENT: OnceLock<NearEscrowConfig> = OnceLock::new();
        CURRENT.get_or_init(|| {
            let config = Self::from_env();
            tracing::info!(
                enabled = config.enabled,
                contracts_mainnet = ?config.contracts_mainnet,
                contracts_testnet = ?config.contracts_testnet,
                min_gas_tgas = config.min_gas.as_gas() / 1_000_000_000_000,
                max_gas_tgas = config.max_gas.as_gas() / 1_000_000_000_000,
                sender_per_hour = config.sender_per_hour,
                operator_ids = ?config.operator_ids,
                "NEAR escrow configuration"
            );
            config
        })
    }

    /// The escrow contracts configured for `network`.
    pub fn contracts(&self, network: Network) -> &[AccountId] {
        match network {
            Network::Near => &self.contracts_mainnet,
            Network::NearTestnet => &self.contracts_testnet,
            _ => &[],
        }
    }

    /// Whether the scheme is on and has a contract to relay to on `network`.
    pub fn serves(&self, network: Network) -> bool {
        self.enabled && !self.contracts(network).is_empty()
    }

    /// Whether `contract` is one of `network`'s escrow contracts.
    fn relays_to(&self, network: Network, contract: &AccountId) -> bool {
        self.contracts(network).contains(contract)
    }
}

fn parse_flag(raw: Option<String>) -> bool {
    match raw.as_deref().map(str::trim) {
        None | Some("") => false,
        Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
        Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
        Some(v) => {
            tracing::warn!(
                variable = ENV_ENABLE_NEAR_ESCROW,
                value = v,
                "not a boolean; NEAR escrow stays off"
            );
            false
        }
    }
}

/// A comma-separated account list. One entry that is not a NEAR account
/// discards the whole list (default: none), so a typo never leaves half a
/// list in force.
fn parse_contracts(variable: &str, raw: Option<String>) -> Vec<AccountId> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut contracts: Vec<AccountId> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match AccountId::from_str(entry) {
            Ok(account) => {
                if !contracts.contains(&account) {
                    contracts.push(account);
                }
            }
            Err(e) => {
                tracing::warn!(
                    variable,
                    entry,
                    error = %e,
                    "not a NEAR account; the whole list is ignored"
                );
                return Vec::new();
            }
        }
    }
    contracts
}

fn parse_number<T>(variable: &str, raw: Option<String>, default: T, min: T, max: T) -> T
where
    T: FromStr + PartialOrd + Copy + std::fmt::Display,
{
    let Some(raw) = raw else {
        return default;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return default;
    }
    match raw.parse::<T>() {
        Ok(value) if value >= min && value <= max => value,
        _ => {
            tracing::warn!(
                variable,
                value = raw,
                %default,
                %min,
                %max,
                "out of range or not a number; using the default"
            );
            default
        }
    }
}

fn parse_operator_ids(raw: Option<String>) -> Vec<String> {
    let default = || DEFAULT_OPERATOR_IDS.iter().map(|s| s.to_string()).collect();
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return default();
    };
    let mut ids: Vec<String> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if !is_operator_id(entry) {
            tracing::warn!(
                variable = ENV_OPERATOR_IDS,
                entry,
                "not an operator id ([a-z0-9_-]{{1,32}}); using the default list"
            );
            return default();
        }
        if !ids.iter().any(|id| id == entry) {
            ids.push(entry.to_string());
        }
    }
    if ids.is_empty() {
        return default();
    }
    ids
}

/// `[a-z0-9_-]{1,32}`, the escrow's operator id alphabet.
fn is_operator_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

// ===========================================================================
// The authorization: msg, payment_id and the delegate action
// ===========================================================================

/// The NEP-297 `standard` the escrow logs its events under.
pub(crate) const ARRAS_STANDARD: &str = "arras";
const FT_TRANSFER_CALL: &str = "ft_transfer_call";
/// NEP-141 asserts exactly one yoctoNEAR on `ft_transfer_call`.
const AUTHORIZE_DEPOSIT: NearToken = NearToken::from_yoctonear(1);

/// Why a request is not relayed. The token is all that reaches the caller
/// (`invalidReason` / `errorReason`); the detail stays in the server log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscrowRejectReason {
    Disabled,
    MalformedRequest,
    SchemeMismatch,
    NetworkMismatch,
    InvalidSignature,
    InvalidAsset,
    MissingExtra,
    ActionsNotOne,
    NotFunctionCall,
    MethodNotAllowed,
    WrongToken,
    MalformedArgs,
    ContractNotServed,
    ContractMismatch,
    WrongDeposit,
    GasOutOfRange,
    AmountMismatch,
    MsgMismatch,
    MsgInvalid,
    MsgNotCanonical,
    OperatorMismatch,
    OperatorNotServed,
    PaymentIdMismatch,
    ReceiverMismatch,
    FeeBpsMaxMismatch,
    TermsMismatch,
    ActionMismatch,
    LifecycleWithAmount,
    DelegateNotExecutable,
    InsufficientFunds,
    PaymentNotFound,
    OperatorNotFound,
    NotCapturer,
    NotPayer,
    NothingClaimable,
    SenderRateLimited,
    SenderCountUnavailable,
}

impl EscrowRejectReason {
    pub(crate) fn as_str(self) -> &'static str {
        use EscrowRejectReason::*;
        match self {
            Disabled => "near_escrow_disabled",
            MalformedRequest => "near_escrow_malformed_request",
            SchemeMismatch => "near_escrow_scheme_mismatch",
            NetworkMismatch => "near_escrow_network_mismatch",
            InvalidSignature => "near_escrow_invalid_signature",
            InvalidAsset => "near_escrow_invalid_asset",
            MissingExtra => "near_escrow_missing_extra",
            ActionsNotOne => "near_escrow_actions_not_one",
            NotFunctionCall => "near_escrow_not_function_call",
            MethodNotAllowed => "near_escrow_method_not_allowed",
            WrongToken => "near_escrow_wrong_token",
            MalformedArgs => "near_escrow_malformed_args",
            ContractNotServed => "near_escrow_contract_not_served",
            ContractMismatch => "near_escrow_contract_mismatch",
            WrongDeposit => "near_escrow_wrong_deposit",
            GasOutOfRange => "near_escrow_gas_out_of_range",
            AmountMismatch => "near_escrow_amount_mismatch",
            MsgMismatch => "near_escrow_msg_mismatch",
            MsgInvalid => "near_escrow_msg_invalid",
            MsgNotCanonical => "near_escrow_msg_not_canonical",
            OperatorMismatch => "near_escrow_operator_mismatch",
            OperatorNotServed => "near_escrow_operator_not_served",
            PaymentIdMismatch => "near_escrow_payment_id_mismatch",
            ReceiverMismatch => "near_escrow_receiver_mismatch",
            FeeBpsMaxMismatch => "near_escrow_fee_bps_max_mismatch",
            TermsMismatch => "near_escrow_terms_mismatch",
            ActionMismatch => "near_escrow_action_mismatch",
            LifecycleWithAmount => "near_escrow_lifecycle_with_amount",
            DelegateNotExecutable => "near_escrow_delegate_not_executable",
            InsufficientFunds => "insufficient_funds",
            PaymentNotFound => "near_escrow_payment_not_found",
            OperatorNotFound => "near_escrow_operator_not_found",
            NotCapturer => "near_escrow_not_capturer",
            NotPayer => "near_escrow_not_payer",
            NothingClaimable => "near_escrow_nothing_claimable",
            SenderRateLimited => "near_escrow_sender_rate_limited",
            SenderCountUnavailable => "near_escrow_sender_count_unavailable",
        }
    }
}

impl From<EscrowRejectReason> for FacilitatorErrorReason {
    fn from(reason: EscrowRejectReason) -> Self {
        FacilitatorErrorReason::FreeForm(reason.as_str().to_string())
    }
}

/// The `msg` of `ft_transfer_call`, field for field, in the canonical order.
/// Serialized back, it is the canonical text: those keys, that order, no
/// whitespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizeMsg {
    v: u8,
    action: String,
    operator_id: String,
    receiver: String,
    release_deadline: String,
    reclaim_after: String,
    fee_bps_max: u16,
    terms_hash: String,
    salt: String,
}

/// The terms of an escrow payment, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Terms {
    pub operator_id: String,
    pub receiver: AccountId,
    pub release_deadline: u64,
    pub reclaim_after: u64,
    pub fee_bps_max: u16,
    pub terms_hash: [u8; 32],
    pub salt: [u8; 16],
}

/// A decimal with no sign, no leading zero and no padding.
fn canonical_u128(s: &str) -> Option<u128> {
    let n = s.parse::<u128>().ok()?;
    (n.to_string() == s).then_some(n)
}

fn canonical_u64(s: &str) -> Option<u64> {
    let n = s.parse::<u64>().ok()?;
    (n.to_string() == s).then_some(n)
}

/// Lowercase hex of exactly `N` bytes.
fn lower_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != 2 * N || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; N];
    hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

/// The terms of a canonical `authorize` msg.
///
/// The text has to be canonical as well as valid: a parser that reads the
/// same fields out of a different text is a difference between our reading
/// and the contract's waiting to happen, so only one text per terms passes.
pub(crate) fn parse_authorize_msg(msg: &str) -> Result<Terms, EscrowRejectReason> {
    use EscrowRejectReason::{MsgInvalid, MsgNotCanonical};
    let parsed: AuthorizeMsg = serde_json::from_str(msg).map_err(|_| MsgInvalid)?;
    if parsed.v != 1 || parsed.action != "authorize" || !is_operator_id(&parsed.operator_id) {
        return Err(MsgInvalid);
    }
    let terms = Terms {
        operator_id: parsed.operator_id.clone(),
        receiver: AccountId::from_str(&parsed.receiver).map_err(|_| MsgInvalid)?,
        release_deadline: canonical_u64(&parsed.release_deadline).ok_or(MsgInvalid)?,
        reclaim_after: canonical_u64(&parsed.reclaim_after).ok_or(MsgInvalid)?,
        fee_bps_max: parsed.fee_bps_max,
        terms_hash: lower_hex::<32>(&parsed.terms_hash).ok_or(MsgInvalid)?,
        salt: lower_hex::<16>(&parsed.salt).ok_or(MsgInvalid)?,
    };
    let canonical = serde_json::to_string(&parsed).map_err(|_| MsgInvalid)?;
    if canonical != msg {
        return Err(MsgNotCanonical);
    }
    Ok(terms)
}

/// The borsh preimage of a payment id: `v` u8 (1), `operator_id`, `token`,
/// `payer` and `receiver` as borsh strings (u32 LE length, then the bytes),
/// `amount` u128 LE, `release_deadline` and `reclaim_after` u64 LE,
/// `fee_bps_max` u16 LE, `terms_hash` 32 bytes, `salt` 16 bytes.
pub(crate) fn payment_id_preimage(
    terms: &Terms,
    token: &str,
    payer: &str,
    amount: u128,
) -> Vec<u8> {
    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    let mut out = vec![1u8];
    string(&mut out, &terms.operator_id);
    string(&mut out, token);
    string(&mut out, payer);
    string(&mut out, terms.receiver.as_str());
    out.extend_from_slice(&amount.to_le_bytes());
    out.extend_from_slice(&terms.release_deadline.to_le_bytes());
    out.extend_from_slice(&terms.reclaim_after.to_le_bytes());
    out.extend_from_slice(&terms.fee_bps_max.to_le_bytes());
    out.extend_from_slice(&terms.terms_hash);
    out.extend_from_slice(&terms.salt);
    out
}

/// `base58(sha256(preimage))`: the base58 of 32 bytes, which runs from 32 to
/// 44 characters (each leading zero byte is one `1`). Never judge it by length.
pub(crate) fn compute_payment_id(terms: &Terms, token: &str, payer: &str, amount: u128) -> String {
    let digest = Sha256::digest(payment_id_preimage(terms, token, payer, amount));
    bs58::encode(digest).into_string()
}

/// A payment id as the escrow writes it: the base58 of 32 bytes, canonical.
fn is_payment_id(s: &str) -> bool {
    bs58::decode(s)
        .into_vec()
        .map(|bytes| bytes.len() == 32 && bs58::encode(&bytes).into_string() == s)
        .unwrap_or(false)
}

/// The network's USDC contract: the one token the escrow holds.
fn usdc(network: Network) -> Option<AccountId> {
    match USDCDeployment::by_network(network)?.address() {
        MixedAddress::Near(account) => AccountId::from_str(&account).ok(),
        _ => None,
    }
}

/// `extra.nearEscrow` of the payment requirements.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NearEscrowExtra {
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub operator_id: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub payment_id: Option<String>,
    /// Absent or `authorize`: a payment. A lifecycle method name: that call.
    #[serde(default)]
    pub action: Option<String>,
}

/// What a NEAR escrow `/verify` or `/settle` carries, read out of the x402
/// v1 or v2 body.
#[derive(Debug, Clone)]
pub(crate) struct EscrowRequest {
    pub network: Network,
    /// Base64 borsh `SignedDelegateAction`.
    pub signed_delegate_action: String,
    pub asset: Option<String>,
    /// `maxAmountRequired` (v1) or `amount` (v2), atomic units.
    pub amount: Option<u128>,
    pub extra: NearEscrowExtra,
}

/// `near` / `near-testnet` / `near:mainnet` / `near:testnet`, or `None`.
pub(crate) fn near_network(name: &str) -> Option<Network> {
    let network = Network::from_caip2(name).or_else(|| Network::from_str(name).ok())?;
    matches!(network, Network::Near | Network::NearTestnet).then_some(network)
}

impl EscrowRequest {
    /// The request in `body`. The requirements are the seller's
    /// `paymentRequirements` when present, else the `accepted` copy.
    pub(crate) fn parse(body: &str) -> Result<Self, EscrowRejectReason> {
        use EscrowRejectReason::{MalformedRequest, MissingExtra, NetworkMismatch, SchemeMismatch};
        let json: Value = serde_json::from_str(body).map_err(|_| MalformedRequest)?;
        let payment_payload = json.get("paymentPayload");
        let requirements = json
            .get("paymentRequirements")
            .or_else(|| json.get("accepted"))
            .or_else(|| payment_payload.and_then(|p| p.get("accepted")))
            .or_else(|| payment_payload.and_then(|p| p.get("paymentRequirements")))
            .filter(|r| r.is_object())
            .ok_or(MalformedRequest)?;
        if let Some(scheme) = requirements.get("scheme") {
            if scheme.as_str() != Some("escrow") {
                return Err(SchemeMismatch);
            }
        }
        let network = requirements
            .get("network")
            .and_then(Value::as_str)
            .and_then(near_network)
            .ok_or(MalformedRequest)?;
        for elsewhere in [
            payment_payload.and_then(|p| p.get("network")),
            payment_payload.and_then(|p| p.get("accepted").and_then(|a| a.get("network"))),
        ]
        .into_iter()
        .flatten()
        {
            if elsewhere.as_str().and_then(near_network) != Some(network) {
                return Err(NetworkMismatch);
            }
        }
        let payload = payment_payload
            .and_then(|p| p.get("payload"))
            .or_else(|| json.get("payload"))
            .ok_or(MalformedRequest)?;
        let signed_delegate_action = payload
            .get("signedDelegateAction")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(MalformedRequest)?
            .to_string();
        let amount_of = |key: &str| -> Result<Option<u128>, EscrowRejectReason> {
            match requirements.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => canonical_u128(s).map(Some).ok_or(MalformedRequest),
                Some(Value::Number(n)) => {
                    n.as_u64().map(|n| Some(n as u128)).ok_or(MalformedRequest)
                }
                Some(_) => Err(MalformedRequest),
            }
        };
        let amount = match (amount_of("maxAmountRequired")?, amount_of("amount")?) {
            (Some(a), Some(b)) if a != b => return Err(MalformedRequest),
            (a, b) => a.or(b),
        };
        let extra = requirements
            .get("extra")
            .and_then(|e| e.get("nearEscrow"))
            .ok_or(MissingExtra)?;
        let extra: NearEscrowExtra =
            serde_json::from_value(extra.clone()).map_err(|_| MissingExtra)?;
        Ok(Self {
            network,
            signed_delegate_action,
            asset: requirements
                .get("asset")
                .and_then(Value::as_str)
                .map(str::to_string),
            amount,
            extra,
        })
    }
}

/// NEP-141 `ft_transfer_call` arguments. `memo` is standard and harmless;
/// any other key is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FtTransferCallArgs {
    receiver_id: String,
    amount: String,
    msg: String,
    #[serde(default)]
    #[allow(dead_code)]
    memo: Option<String>,
}

/// The single function call of a delegate action.
fn single_function_call(da: &DelegateAction) -> Result<FunctionCallAction, EscrowRejectReason> {
    if da.actions.len() != 1 {
        return Err(EscrowRejectReason::ActionsNotOne);
    }
    match Action::from(da.actions[0].clone()) {
        Action::FunctionCall(call) => Ok(*call),
        _ => Err(EscrowRejectReason::NotFunctionCall),
    }
}

/// An authorization that passed [`validate_escrow_delegate_actions`]: what the
/// outcome of its settlement has to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EscrowAuthorize {
    /// The escrow contract, `args.receiver_id`.
    pub contract: AccountId,
    /// The network's USDC, the delegate action's receiver.
    pub token: AccountId,
    /// The delegate action's sender.
    pub payer: AccountId,
    pub amount: u128,
    /// The id the contract will give the payment.
    pub payment_id: String,
}

/// Whether a delegate action authorizes exactly the payment `req` describes.
///
/// One action, a `FunctionCall` of `ft_transfer_call` on the network's USDC,
/// to one of the network's escrow contracts (the one `extra.nearEscrow.contract`
/// names), attaching exactly one yoctoNEAR and gas within the configured
/// range, moving exactly the required amount, with `msg` byte for byte the
/// canonical `extra.nearEscrow.msg`, for an operator the facilitator serves,
/// and producing, by the contract's own derivation, `extra.nearEscrow.paymentId`.
/// The first condition that fails is the answer.
pub(crate) fn validate_escrow_delegate_actions(
    da: &DelegateAction,
    req: &EscrowRequest,
    cfg: &NearEscrowConfig,
) -> Result<EscrowAuthorize, EscrowRejectReason> {
    use EscrowRejectReason::*;
    let call = single_function_call(da)?;
    if call.method_name != FT_TRANSFER_CALL {
        return Err(MethodNotAllowed);
    }
    let token = usdc(req.network).ok_or(WrongToken)?;
    if da.receiver_id != token {
        return Err(WrongToken);
    }
    let args: FtTransferCallArgs = serde_json::from_slice(&call.args).map_err(|_| MalformedArgs)?;
    let contract = AccountId::from_str(&args.receiver_id).map_err(|_| ContractNotServed)?;
    if !cfg.relays_to(req.network, &contract) {
        return Err(ContractNotServed);
    }
    if req.extra.contract.as_deref() != Some(contract.as_str()) {
        return Err(ContractMismatch);
    }
    if call.deposit != AUTHORIZE_DEPOSIT {
        return Err(WrongDeposit);
    }
    if call.gas < cfg.min_gas || call.gas > cfg.max_gas {
        return Err(GasOutOfRange);
    }
    let amount = canonical_u128(&args.amount).ok_or(AmountMismatch)?;
    if req.amount != Some(amount) {
        return Err(AmountMismatch);
    }
    let expected_msg = req.extra.msg.as_deref().ok_or(MissingExtra)?;
    if args.msg.as_bytes() != expected_msg.as_bytes() {
        return Err(MsgMismatch);
    }
    let terms = parse_authorize_msg(&args.msg)?;
    if req.extra.operator_id.as_deref() != Some(terms.operator_id.as_str()) {
        return Err(OperatorMismatch);
    }
    if !cfg.operator_ids.contains(&terms.operator_id) {
        return Err(OperatorNotServed);
    }
    let expected_id = req.extra.payment_id.as_deref().ok_or(MissingExtra)?;
    let payment_id = compute_payment_id(&terms, token.as_str(), da.sender_id.as_str(), amount);
    if payment_id != expected_id {
        return Err(PaymentIdMismatch);
    }
    Ok(EscrowAuthorize {
        contract,
        token,
        payer: da.sender_id.clone(),
        amount,
        payment_id,
    })
}

// ===========================================================================
// Reading an authorization's outcome
// ===========================================================================

/// What an outcome says about the escrow effect a relay was for.
#[derive(Debug)]
enum Verdict {
    /// The effect did not happen, and that is final: `success: false`.
    NotPaid(String),
    /// The outcome does not show the effect, nor that it failed. It may still
    /// have happened: the hash travels as `SettlementUnconfirmed`.
    NoVerdict(String),
}

fn verdict_error(verdict: Verdict, tx_hash: CryptoHash, network: Network) -> FacilitatorLocalError {
    match verdict {
        Verdict::NotPaid(why) => FacilitatorLocalError::ContractCall(format!(
            "Meta-transaction {tx_hash} executed without the escrow effect: {why}"
        )),
        Verdict::NoVerdict(why) => {
            tracing::warn!(%tx_hash, why, "NEAR escrow meta-transaction executed, effect not confirmed yet");
            FacilitatorLocalError::SettlementUnconfirmed(TransactionHash::Near(tx_hash.0), network)
        }
    }
}

fn no_verdict(why: &str) -> Verdict {
    Verdict::NoVerdict(why.to_string())
}

fn receipt_by_id<'a>(
    outcome: &'a FinalExecutionOutcomeView,
    id: &CryptoHash,
) -> Option<&'a ExecutionOutcomeWithIdView> {
    outcome.receipts_outcome.iter().find(|r| r.id == *id)
}

/// The receipts `parent` created that `account` executed.
fn children_on<'a>(
    outcome: &'a FinalExecutionOutcomeView,
    parent: &ExecutionOutcomeWithIdView,
    account: &AccountId,
) -> Vec<&'a ExecutionOutcomeWithIdView> {
    parent
        .outcome
        .receipt_ids
        .iter()
        .filter_map(|id| receipt_by_id(outcome, id))
        .filter(|r| r.outcome.executor_id == *account)
        .collect()
}

/// The one receipt the Delegate action created on `account`, the same way
/// `confirm_transfer` finds the `ft_transfer`: the transaction's own status
/// is the Delegate receipt's, which succeeds as soon as it has created the
/// inner one.
fn delegate_child_on<'a>(
    outcome: &'a FinalExecutionOutcomeView,
    account: &AccountId,
) -> Result<&'a ExecutionOutcomeWithIdView, Verdict> {
    let delegate = match &outcome.transaction_outcome.outcome.status {
        ExecutionStatusView::SuccessReceiptId(id) => receipt_by_id(outcome, id)
            .ok_or_else(|| no_verdict("Delegate receipt missing from the outcome"))?,
        _ => return Err(no_verdict("transaction not converted to a receipt")),
    };
    match &delegate.outcome.status {
        ExecutionStatusView::Failure(e) => {
            return Err(Verdict::NotPaid(format!("Delegate action failed: {e:?}")))
        }
        ExecutionStatusView::Unknown => return Err(no_verdict("Delegate receipt not executed")),
        ExecutionStatusView::SuccessValue(_) | ExecutionStatusView::SuccessReceiptId(_) => {}
    }
    match children_on(outcome, delegate, account).as_slice() {
        [] => Err(no_verdict(
            "no receipt of the Delegate action on its receiver",
        )),
        [only] => Ok(*only),
        _ => Err(no_verdict(
            "more than one receipt of the Delegate action on its receiver",
        )),
    }
}

/// The `data` entries of the `arras` events named `name` in `logs`.
fn arras_events<'a>(logs: impl IntoIterator<Item = &'a String>, name: &str) -> Vec<Value> {
    logs.into_iter()
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .filter_map(|json| serde_json::from_str::<Nep297Event>(json).ok())
        .filter(|event| event.standard == ARRAS_STANDARD && event.event == name)
        .filter_map(|event| match event.data {
            Value::Array(entries) => Some(entries),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Whether `logs` carry the `arras` event `name` for `payment_id`.
fn logs_payment_event<'a>(
    logs: impl IntoIterator<Item = &'a String>,
    name: &str,
    payment_id: &str,
) -> bool {
    arras_events(logs, name)
        .iter()
        .any(|data| data.get("payment_id").and_then(Value::as_str) == Some(payment_id))
}

/// The `ok` of the `withdrawn` event for `account` in `logs`: `Some(true)` if
/// one says the transfer went out, `Some(false)` if one says it came back.
fn logs_withdrawn<'a>(
    logs: impl IntoIterator<Item = &'a String>,
    account: &AccountId,
) -> Option<bool> {
    let oks: Vec<Option<bool>> = arras_events(logs, "withdrawn")
        .iter()
        .filter(|data| data.get("account").and_then(Value::as_str) == Some(account.as_str()))
        .map(|data| data.get("ok").and_then(Value::as_bool))
        .collect();
    if oks.contains(&Some(true)) {
        Some(true)
    } else if oks.contains(&Some(false)) {
        Some(false)
    } else {
        None
    }
}

/// Whether a relayed authorization put the payment in escrow.
///
/// `ft_transfer_call` runs on the token, which logs the NEP-141 transfer and
/// calls `ft_on_transfer` on the escrow; the escrow keeps what it accepts and
/// logs `authorized`; `ft_resolve_transfer` back on the token refunds the rest
/// and returns what was used, which becomes the call's own value. None of it
/// shows in the transaction's status. So the payment is in escrow only when
/// the outcome shows all three:
///
/// - (a) the receipt the call's promise resolved to, on the token
///   (`ft_resolve_transfer`), returned exactly the amount. Any other amount,
///   `"0"` included, is a refusal the token already refunded: not paid.
/// - (b) the token logged the NEP-141 `ft_transfer` of the amount from the
///   payer to the escrow, in the `ft_transfer_call` receipt.
/// - (c) the escrow's `ft_on_transfer` receipt succeeded and logged the
///   `arras` `authorized` event for this payment id.
///
/// A failed Delegate or `ft_transfer_call` receipt is final: not paid. Any
/// other gap is no verdict, answered as `SettlementUnconfirmed` with the hash.
pub(crate) fn confirm_escrow_authorize(
    outcome: &FinalExecutionOutcomeView,
    expected: &EscrowAuthorize,
    network: Network,
) -> Result<CryptoHash, FacilitatorLocalError> {
    let tx_hash = outcome.transaction.hash;
    let read = || -> Result<(), Verdict> {
        let call = delegate_child_on(outcome, &expected.token)?;
        let resolve = match &call.outcome.status {
            ExecutionStatusView::Failure(e) => {
                return Err(Verdict::NotPaid(format!("ft_transfer_call failed: {e:?}")))
            }
            ExecutionStatusView::Unknown => {
                return Err(no_verdict("ft_transfer_call receipt not executed"))
            }
            ExecutionStatusView::SuccessValue(_) => {
                return Err(no_verdict("ft_transfer_call returned no promise"))
            }
            ExecutionStatusView::SuccessReceiptId(id) => {
                receipt_by_id(outcome, id).ok_or_else(|| {
                    no_verdict("ft_resolve_transfer receipt missing from the outcome")
                })?
            }
        };
        if resolve.outcome.executor_id != expected.token {
            return Err(no_verdict(
                "the promise of ft_transfer_call is not on the token",
            ));
        }
        // (a)
        let used = match &resolve.outcome.status {
            ExecutionStatusView::SuccessValue(value) => parse_ft_balance(value)
                .map_err(|_| no_verdict("ft_resolve_transfer did not return an amount"))?,
            _ => return Err(no_verdict("ft_resolve_transfer did not return an amount")),
        };
        if used != expected.amount {
            return Err(Verdict::NotPaid(format!(
                "ft_resolve_transfer used {used} of {}: the escrow refused the payment and the token refunded it",
                expected.amount
            )));
        }
        // (b)
        let moved = ExpectedTransfer {
            token: expected.token.clone(),
            payer: expected.payer.clone(),
            receiver: expected.contract.to_string(),
            amount: expected.amount.to_string(),
        };
        if !logs_transfer(&call.outcome.logs, &moved) {
            return Err(no_verdict(
                "ft_transfer_call without the NEP-141 event of this payment",
            ));
        }
        // (c)
        let on_transfer = match children_on(outcome, call, &expected.contract).as_slice() {
            [only] => *only,
            [] => return Err(no_verdict("no ft_on_transfer receipt on the escrow")),
            _ => {
                return Err(no_verdict(
                    "more than one ft_on_transfer receipt on the escrow",
                ))
            }
        };
        if !matches!(
            on_transfer.outcome.status,
            ExecutionStatusView::SuccessValue(_)
        ) {
            return Err(no_verdict("ft_on_transfer did not succeed"));
        }
        if !logs_payment_event(
            &on_transfer.outcome.logs,
            "authorized",
            &expected.payment_id,
        ) {
            return Err(no_verdict("no arras authorized event for this payment id"));
        }
        Ok(())
    };
    read()
        .map(|()| tx_hash)
        .map_err(|verdict| verdict_error(verdict, tx_hash, network))
}

// ===========================================================================
// Verify and settle
// ===========================================================================

/// The escrow scheme's state on a provider.
pub(crate) struct EscrowRuntime {
    config: NearEscrowConfig,
}

impl EscrowRuntime {
    pub(crate) fn new(_network: Network, config: NearEscrowConfig) -> Self {
        Self { config }
    }
}

/// What a checked request relays.
#[derive(Debug, Clone)]
pub(crate) enum EscrowOp {
    Authorize(EscrowAuthorize),
}

/// A request that may be relayed.
#[derive(Debug)]
pub(crate) struct Checked {
    pub sender: AccountId,
    pub signed_delegate_action: SignedDelegateAction,
    pub op: EscrowOp,
}

/// A request that is not relayed, and who signed it when that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejected {
    pub sender: Option<AccountId>,
    pub reason: EscrowRejectReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Verify,
    Settle,
}

type Checking = Result<Result<Checked, Rejected>, FacilitatorLocalError>;

impl NearProvider {
    /// Whether this provider relays `req`, and what it relays.
    ///
    /// The outer error is the chain not answering (no verdict on the request);
    /// the inner one is a verdict.
    async fn check_escrow(&self, req: &EscrowRequest, _phase: Phase) -> Checking {
        let cfg = &self.escrow.config;
        let refuse = |sender: Option<&AccountId>, reason: EscrowRejectReason| -> Checking {
            tracing::warn!(
                network = %self.network(),
                sender = ?sender.map(|s| s.as_str()),
                reason = reason.as_str(),
                "NEAR escrow request refused"
            );
            Ok(Err(Rejected {
                sender: sender.cloned(),
                reason,
            }))
        };
        if !cfg.enabled {
            return refuse(None, EscrowRejectReason::Disabled);
        }
        if req.network != self.network() {
            return refuse(None, EscrowRejectReason::NetworkMismatch);
        }
        let signed = match self.decode_signed_delegate_action(&req.signed_delegate_action) {
            Ok(signed) => signed,
            Err(_) => return refuse(None, EscrowRejectReason::MalformedRequest),
        };
        let sender = signed.delegate_action.sender_id.clone();
        if !signed.verify() {
            return refuse(Some(&sender), EscrowRejectReason::InvalidSignature);
        }
        let da = &signed.delegate_action;
        let op = match req.extra.action.as_deref() {
            None | Some("authorize") => {
                let token = usdc(self.network());
                if req.asset.is_none() || req.asset.as_deref() != token.as_ref().map(|t| t.as_str())
                {
                    return refuse(Some(&sender), EscrowRejectReason::InvalidAsset);
                }
                let verdict =
                    validate_escrow_delegate_actions(da, req, cfg).map(EscrowOp::Authorize);
                match verdict {
                    Ok(op) => op,
                    Err(reason) => return refuse(Some(&sender), reason),
                }
            }
            Some(_) => return refuse(Some(&sender), EscrowRejectReason::MethodNotAllowed),
        };

        let timeout = crate::chain::rpc_http_timeout();
        match self.check_delegate_nonce_unused(&signed, timeout).await {
            Ok(()) => {}
            Err(FacilitatorLocalError::Other(why)) => {
                tracing::warn!(%sender, why, "NEAR escrow delegate action cannot execute");
                return refuse(Some(&sender), EscrowRejectReason::DelegateNotExecutable);
            }
            Err(e) => return Err(e),
        }
        match &op {
            EscrowOp::Authorize(authorize) => {
                match self
                    .check_payer_balance(
                        da,
                        alloy::primitives::U256::from(authorize.amount),
                        timeout,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(FacilitatorLocalError::InsufficientFunds(_)) => {
                        return refuse(Some(&sender), EscrowRejectReason::InsufficientFunds)
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(Ok(Checked {
            sender,
            signed_delegate_action: signed,
            op,
        }))
    }

    /// `/verify` of a NEAR escrow request.
    pub(crate) async fn verify_escrow(
        &self,
        req: &EscrowRequest,
    ) -> Result<VerifyResponse, FacilitatorLocalError> {
        Ok(match self.check_escrow(req, Phase::Verify).await? {
            Ok(checked) => VerifyResponse::valid(MixedAddress::Near(checked.sender.to_string())),
            Err(rejected) => VerifyResponse::invalid(
                rejected.sender.map(|s| MixedAddress::Near(s.to_string())),
                rejected.reason.into(),
            ),
        })
    }

    /// `/settle` of a NEAR escrow request: checked again, relayed, and
    /// answered from the outcome.
    pub(crate) async fn settle_escrow(
        &self,
        req: &EscrowRequest,
    ) -> Result<SettleResponse, FacilitatorLocalError> {
        let network = self.network();
        let checked = match self.check_escrow(req, Phase::Settle).await? {
            Ok(checked) => checked,
            Err(rejected) => {
                return Ok(SettleResponse {
                    success: false,
                    error_reason: Some(rejected.reason.into()),
                    payer: MixedAddress::Near(
                        rejected.sender.map(|s| s.to_string()).unwrap_or_default(),
                    ),
                    transaction: None,
                    network,
                    proof_of_payment: None,
                    extensions: None,
                })
            }
        };
        let payer = MixedAddress::Near(checked.sender.to_string());
        let relayed = self.relay_delegate(checked.signed_delegate_action).await;
        let confirmed = relayed.and_then(|outcome| match &checked.op {
            EscrowOp::Authorize(authorize) => {
                confirm_escrow_authorize(&outcome, authorize, network)
            }
        });
        let mut response = settle_outcome(confirmed, payer, network)?;
        if response.success {
            response.extensions = Some(settle_extensions(&checked.op));
        }
        Ok(response)
    }
}

/// `extensions.nearEscrow` of a successful settlement.
fn settle_extensions(op: &EscrowOp) -> std::collections::HashMap<String, Value> {
    let details = match op {
        EscrowOp::Authorize(a) => serde_json::json!({
            "action": "authorize",
            "contract": a.contract,
            "paymentId": a.payment_id,
        }),
    };
    std::collections::HashMap::from([("nearEscrow".to_string(), details)])
}

/// The NEAR provider `facilitator` holds for `network`.
fn near_provider<F>(
    facilitator: &F,
    network: Network,
) -> Result<&NearProvider, FacilitatorLocalError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    match facilitator.provider_map().by_network(network) {
        Some(NetworkProvider::Near(provider)) => Ok(provider),
        _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
    }
}

/// The NEAR provider for `network` and the request in `body`, or the answer
/// to give instead: the scheme switched off on that provider, or a body that
/// does not parse.
fn routed<'a, F>(
    body: &str,
    network: Network,
    facilitator: &'a F,
) -> Result<Result<(&'a NearProvider, EscrowRequest), EscrowRejectReason>, FacilitatorLocalError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    let provider = near_provider(facilitator, network)?;
    if !provider.escrow.config.enabled {
        tracing::warn!(%network, "NEAR escrow requested but ENABLE_NEAR_ESCROW is not set to true");
        return Ok(Err(EscrowRejectReason::Disabled));
    }
    let request = EscrowRequest::parse(body)
        .map_err(|reason| FacilitatorLocalError::DecodingError(reason.as_str().to_string()))?;
    Ok(Ok((provider, request)))
}

/// `/verify` of a body the handler routed here: scheme `escrow` on `network`,
/// a NEAR network.
pub async fn verify<F>(
    body: &str,
    network: Network,
    facilitator: &F,
) -> Result<VerifyResponse, FacilitatorLocalError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    match routed(body, network, facilitator)? {
        Ok((provider, request)) => provider.verify_escrow(&request).await,
        Err(reason) => Ok(VerifyResponse::invalid(None, reason.into())),
    }
}

/// `/settle` of a body the handler routed here: scheme `escrow` on `network`,
/// a NEAR network.
pub async fn settle<F>(
    body: &str,
    network: Network,
    facilitator: &F,
) -> Result<SettleResponse, FacilitatorLocalError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    match routed(body, network, facilitator)? {
        Ok((provider, request)) => provider.settle_escrow(&request).await,
        Err(reason) => Ok(SettleResponse {
            success: false,
            error_reason: Some(reason.into()),
            payer: MixedAddress::Near(String::new()),
            transaction: None,
            network,
            proof_of_payment: None,
            extensions: None,
        }),
    }
}
