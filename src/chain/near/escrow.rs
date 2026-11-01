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

use near_primitives::action::delegate::DelegateAction;
use near_primitives::transaction::{Action, FunctionCallAction};
use near_primitives::types::{AccountId, Gas};
use near_token::NearToken;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::network::{Network, USDCDeployment};
use crate::types::{FacilitatorErrorReason, MixedAddress};

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
