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

use near_primitives::types::{AccountId, Gas};

use crate::network::Network;

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
