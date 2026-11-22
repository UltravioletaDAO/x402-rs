//! Which counterfactual-wallet deployments (EIP-6492) an EVM `exact` payment
//! may ask the facilitator to pay for: a policy per network, in configuration.
//!
//! # What the facilitator does with an EIP-6492 signature today
//!
//! It accepts none and deploys nothing; sponsoring deployments is not enabled.
//! Every EVM `exact` signature must be exactly 65 bytes
//! (`chain::evm`'s `assert_valid_payment`, the EIP-2 low-s rule), and an
//! EIP-6492 envelope -- `(factory, factoryCalldata, innerSig)` ABI-encoded,
//! then a 32-byte magic suffix -- never is: `/verify` answers
//! `invalid_signature` and `/settle` sends no transaction. The settle branch
//! that would deploy the payer's wallet through its factory in the same
//! Multicall3 transaction as the transfer is unreachable, as
//! [`crate::chain::gas_ceiling::GasKind::ExactSettleDeploy`] already says. A
//! deployed contract wallet's ERC-1271 signature meets the same rule: it passes
//! only when it is 65 bytes.
//!
//! # What this module is
//!
//! The policy that branch would answer to if sponsorship were enabled (lifting
//! the 65-byte rule for envelopes, which this module does not do), enforced where every EVM `exact`
//! payment is judged -- in `assert_valid_payment`, ahead of the 65-byte rule
//! and before anything is sent (for a token in the static EIP-712 table, before
//! any RPC too), for `/verify` and `/settle` alike:
//!
//! * **Factories.** Only a factory on the network's list is one the
//!   facilitator would sponsor. The default list, the same on every EVM
//!   network, is [`DEFAULT_FACTORIES`]; `EIP6492_FACTORY_ALLOWLIST` replaces
//!   it on every network and `EIP6492_FACTORY_ALLOWLIST_<NETWORK>` on one
//!   (comma-separated addresses; an empty value lists none). A value with an
//!   entry that is not an address is ignored whole, with a warning, and the
//!   list it would have replaced stays: a typo never stops the process and
//!   never passes for a list.
//! * **Gas.** The deploy-and-transfer transaction is held to the
//!   `exact-deploy` ceiling of [`crate::chain::gas_ceiling`]
//!   (`EVM_MAX_GAS_EXACT_DEPLOY_<NETWORK>`): an estimate above it is refused
//!   before a nonce is reserved, and nothing is sent. One knob for one limit,
//!   so this module reads that ceiling and never defines a second one.
//! * **Mode.** `EIP6492_SPONSOR_MODE`: `log` (the default) records an
//!   envelope whose factory is off the list and lets the payment go on exactly
//!   as before; `enforce` refuses it, `invalidReason`
//!   `eip6492_factory_not_allowed`, with nothing read from the chain and
//!   nothing sent. Anything else is `log` plus a warning.
//!
//! `GET /config` publishes the policy in force (`eip6492Sponsorship`).
//!
//! # Who pays when it fails
//!
//! Nobody, by construction: `/verify` simulates the deployment and the
//! transfer in one `eth_call` (the universal validator's
//! `isValidSigWithSideEffects`, then `transferWithAuthorization`), and
//! `/settle` estimates the transaction before it signs it, so a deployment or
//! a transfer that would revert -- a payer without the token balance, a
//! factory call that does not produce the wallet -- is refused before a
//! transaction exists. The payer must hold the token before the payment; the
//! facilitator pays the gas of a valid one.

use std::collections::HashMap;
use std::sync::LazyLock;

use alloy::primitives::{address, Address};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::chain::gas_ceiling::{self, GasKind};
use crate::chain::FacilitatorLocalError;
use crate::network::{Network, NetworkFamily};
use crate::types::EvmAddress;

/// The mode variable.
pub const ENV_MODE: &str = "EIP6492_SPONSOR_MODE";
/// The list that replaces [`DEFAULT_FACTORIES`] on every network.
pub const ENV_ALLOWLIST: &str = "EIP6492_FACTORY_ALLOWLIST";
/// The token `/verify` answers a refused envelope with (`invalidReason`).
pub const REFUSAL_CODE: &str = "eip6492_factory_not_allowed";

/// The smart-wallet factories sponsored by default, and where each address
/// was read. Each is deployed at the same address on every chain that has it
/// (CREATE2 through a deterministic deployer), so one list serves every EVM
/// network; on a chain where a factory has no code its call deploys nothing,
/// the simulation refuses the payment, and listing it costs nothing.
///
/// Read 2026-11-22:
/// - Coinbase Smart Wallet, the `Factory Address` table of the README of
///   github.com/coinbase/smart-wallet (versions 1 and 1.1); deployed through
///   the Safe Singleton Factory, "the same address across 248 chains".
/// - Safe ProxyFactory, github.com/safe-global/safe-deployments:
///   `src/assets/v1.4.1/safe_proxy_factory.json` (`canonical`; its
///   `networkAddresses` list HyperEVM, 999, among the networks served here)
///   and `src/assets/v1.3.0/proxy_factory.json` (`canonical` and `eip155`).
pub const DEFAULT_FACTORIES: &[(Address, &str)] = &[
    (
        address!("0x0BA5ED0c6AA8c49038F819E587E2633c4A9F428a"),
        "Coinbase Smart Wallet factory v1 (github.com/coinbase/smart-wallet, README)",
    ),
    (
        address!("0xBA5ED110eFDBa3D005bfC882d75358ACBbB85842"),
        "Coinbase Smart Wallet factory v1.1 (github.com/coinbase/smart-wallet, README)",
    ),
    (
        address!("0x4e1DCf7AD4e460CfD30791CCC4F9c8a4f820ec67"),
        "Safe ProxyFactory 1.4.1 canonical (safe-global/safe-deployments v1.4.1/safe_proxy_factory.json)",
    ),
    (
        address!("0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2"),
        "Safe ProxyFactory 1.3.0 canonical (safe-global/safe-deployments v1.3.0/proxy_factory.json)",
    ),
    (
        address!("0xC22834581EbC8527d974F8a1c97E1bEA4EF910BC"),
        "Safe ProxyFactory 1.3.0 eip155 (safe-global/safe-deployments v1.3.0/proxy_factory.json)",
    ),
];

/// What the policy does with an envelope whose factory is off the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Record it and let the payment go on as before.
    Log,
    /// Refuse the payment before anything is read or sent.
    Enforce,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Log => "log",
            Mode::Enforce => "enforce",
        }
    }
}

/// The policy's answer for one factory on one network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// On the network's list.
    Allowed,
    /// Off the list, in `log` mode: recorded, and the payment goes on.
    OutsideLogged,
    /// Off the list, in `enforce` mode: refused.
    OutsideRefused,
}

/// The sponsorship policy in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SponsorPolicy {
    mode: Mode,
    everywhere: Vec<Address>,
    per_network: HashMap<Network, Vec<Address>>,
}

/// The policy in force, read from the environment on first use.
pub fn global() -> &'static SponsorPolicy {
    static GLOBAL: LazyLock<SponsorPolicy> = LazyLock::new(|| {
        let policy = SponsorPolicy::from_env();
        let mut overridden: Vec<String> = policy
            .per_network
            .keys()
            .map(|network| network.to_string())
            .collect();
        overridden.sort();
        info!(
            mode = policy.mode.as_str(),
            factories = policy.everywhere.len(),
            per_network = %overridden.join(","),
            "EIP-6492 sponsorship policy configured"
        );
        policy
    });
    &GLOBAL
}

impl SponsorPolicy {
    /// The policy the process environment describes.
    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    /// The policy the variables `lookup` answers describe, by name.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mode = match lookup(ENV_MODE) {
            None => Mode::Log,
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "log" | "" => Mode::Log,
                "enforce" => Mode::Enforce,
                _ => {
                    warn!(var = ENV_MODE, value = %raw, "not log or enforce; using log");
                    Mode::Log
                }
            },
        };
        let defaults: Vec<Address> = DEFAULT_FACTORIES.iter().map(|(a, _)| *a).collect();
        let everywhere = lookup(ENV_ALLOWLIST)
            .and_then(|raw| parse_list(ENV_ALLOWLIST, &raw))
            .unwrap_or(defaults);
        let mut per_network = HashMap::new();
        for network in evm_networks() {
            let var = env_var_for(network);
            if let Some(list) = lookup(&var).and_then(|raw| parse_list(&var, &raw)) {
                per_network.insert(network, list);
            }
        }
        Self {
            mode,
            everywhere,
            per_network,
        }
    }

    /// The mode in force.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The factories this policy would sponsor on `network`.
    pub fn factories(&self, network: Network) -> &[Address] {
        self.per_network.get(&network).unwrap_or(&self.everywhere)
    }

    /// The verdict for an envelope naming `factory` on `network`.
    pub fn judge(&self, network: Network, factory: Address) -> Verdict {
        if self.factories(network).contains(&factory) {
            Verdict::Allowed
        } else if self.mode == Mode::Enforce {
            Verdict::OutsideRefused
        } else {
            Verdict::OutsideLogged
        }
    }

    /// Admit or refuse a payment whose EIP-6492 envelope names `factory`.
    pub fn admit(
        &self,
        network: Network,
        payer: EvmAddress,
        factory: Address,
    ) -> Result<Verdict, FacilitatorLocalError> {
        let verdict = self.judge(network, factory);
        match verdict {
            Verdict::Allowed => {}
            Verdict::OutsideLogged => warn!(
                network = %network,
                factory = %factory,
                mode = "log",
                "eip6492: the factory of this envelope is not on the sponsorship list; going on (log mode)"
            ),
            Verdict::OutsideRefused => {
                warn!(
                    network = %network,
                    factory = %factory,
                    mode = "enforce",
                    "eip6492: the factory of this envelope is not on the sponsorship list; refused"
                );
                return Err(FacilitatorLocalError::InvalidSignature(
                    payer.into(),
                    format!(
                        concat!(
                            "{}: this facilitator does not sponsor a wallet deployment ",
                            "through factory {} on {}. See GET /config, eip6492Sponsorship."
                        ),
                        REFUSAL_CODE, factory, network
                    ),
                ));
            }
        }
        Ok(verdict)
    }

    /// What `GET /config` publishes: the mode, the factories per network,
    /// and the gas ceiling a deployment is held to there.
    pub fn document(&self) -> Value {
        let per_network: serde_json::Map<String, Value> = evm_networks()
            .map(|network| {
                (
                    network.to_string(),
                    json!({
                        "factories": self.factories(network).iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                        "deployGasCeiling": gas_ceiling::ceiling(network, GasKind::ExactSettleDeploy),
                    }),
                )
            })
            .collect();
        json!({
            "mode": self.mode().as_str(),
            "env": {
                "mode": ENV_MODE,
                "allowlist": ENV_ALLOWLIST,
                "allowlistPerNetwork": format!("{ENV_ALLOWLIST}_<NETWORK>"),
                "deployGasCeiling": gas_ceiling::env_var_for(Network::Base, GasKind::ExactSettleDeploy)
                    .replace("_BASE", "_<NETWORK>"),
            },
            "refusal": { "invalidReason": REFUSAL_CODE, "when": "mode is enforce and the envelope's factory is not listed for its network" },
            "sponsoring": false,
            "sixtyFiveByteRule": "every EVM exact signature must be 65 bytes, so an EIP-6492 envelope is refused as invalid_signature whatever its factory and no wallet is deployed; this policy is what a deployment would answer to if sponsorship were enabled",
            "perNetwork": per_network,
        })
    }
}

/// Parse a comma-separated list of addresses; `None` (with a warning) when an
/// entry is not one, so the list it would have replaced stays.
fn parse_list(var: &str, raw: &str) -> Option<Vec<Address>> {
    let mut list = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match entry.parse::<Address>() {
            Ok(address) => {
                if !list.contains(&address) {
                    list.push(address);
                }
            }
            Err(_) => {
                warn!(var = var, entry = %entry, "not an address; the variable is ignored");
                return None;
            }
        }
    }
    Some(list)
}

/// `EIP6492_FACTORY_ALLOWLIST_<NETWORK>`: the network's v1 name, upper-cased,
/// `-` as `_`.
pub fn env_var_for(network: Network) -> String {
    format!(
        "{ENV_ALLOWLIST}_{}",
        network.to_string().to_uppercase().replace('-', "_")
    )
}

fn evm_networks() -> impl Iterator<Item = Network> {
    Network::variants()
        .iter()
        .copied()
        .filter(|network| matches!(NetworkFamily::from(*network), NetworkFamily::Evm))
}

#[cfg(test)]
thread_local! {
    /// A policy one test thread runs under ([`set_for_test`]). Thread-local,
    /// so a test in another thread never sees it.
    static UNDER_TEST: std::cell::RefCell<Option<SponsorPolicy>> = const { std::cell::RefCell::new(None) };
}

/// Run this thread under `policy` until the guard drops (tests only).
#[cfg(test)]
pub(crate) fn set_for_test(policy: SponsorPolicy) -> TestPolicyGuard {
    UNDER_TEST.with(|cell| *cell.borrow_mut() = Some(policy));
    TestPolicyGuard
}

#[cfg(test)]
pub(crate) struct TestPolicyGuard;

#[cfg(test)]
impl Drop for TestPolicyGuard {
    fn drop(&mut self) {
        UNDER_TEST.with(|cell| *cell.borrow_mut() = None);
    }
}

/// Admit or refuse an EIP-6492 envelope naming `factory` on `network`,
/// under the policy in force.
pub fn admit(
    network: Network,
    payer: EvmAddress,
    factory: Address,
) -> Result<Verdict, FacilitatorLocalError> {
    #[cfg(test)]
    {
        if let Some(policy) = UNDER_TEST.with(|cell| cell.borrow().clone()) {
            return policy.admit(network, payer, factory);
        }
    }
    global().admit(network, payer, factory)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COINBASE_V1: Address = address!("0x0BA5ED0c6AA8c49038F819E587E2633c4A9F428a");
    const UNKNOWN: Address = address!("0x00000000000000000000000000000000000f4c70");

    fn policy(vars: &[(&str, &str)]) -> SponsorPolicy {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        SponsorPolicy::from_lookup(|var| vars.get(var).cloned())
    }

    fn payer() -> EvmAddress {
        EvmAddress(address!("0x1111111111111111111111111111111111111111"))
    }

    #[test]
    fn it_starts_in_log_mode_with_the_known_factories_on_every_evm_network() {
        let p = policy(&[]);
        assert_eq!(p.mode(), Mode::Log);
        for network in evm_networks() {
            let listed = p.factories(network);
            for (factory, _) in DEFAULT_FACTORIES {
                assert!(listed.contains(factory), "{network}: {factory}");
            }
            assert_eq!(p.judge(network, COINBASE_V1), Verdict::Allowed);
            assert_eq!(p.judge(network, UNKNOWN), Verdict::OutsideLogged);
        }
    }

    #[test]
    fn every_default_factory_names_its_source() {
        for (factory, source) in DEFAULT_FACTORIES {
            assert!(
                source.contains("github.com/") || source.contains("safe-global/"),
                "{factory}: {source}"
            );
        }
    }

    #[test]
    fn log_mode_lets_an_unlisted_factory_go_on_and_enforce_refuses_it() {
        assert_eq!(
            policy(&[])
                .admit(Network::HyperEvm, payer(), UNKNOWN)
                .unwrap(),
            Verdict::OutsideLogged
        );
        let enforce = policy(&[(ENV_MODE, "enforce")]);
        let error = enforce
            .admit(Network::HyperEvm, payer(), UNKNOWN)
            .expect_err("enforce refuses an unlisted factory");
        let FacilitatorLocalError::InvalidSignature(_, message) = error else {
            panic!("expected an invalid-signature verdict, got {error:?}");
        };
        assert!(message.starts_with(REFUSAL_CODE), "{message}");
        assert!(message.contains("hyperevm"), "{message}");
        assert!(!message.contains("  "), "{message:?}");
        assert_eq!(
            enforce
                .admit(Network::HyperEvm, payer(), COINBASE_V1)
                .unwrap(),
            Verdict::Allowed
        );
    }

    #[test]
    fn a_list_replaces_the_default_everywhere_or_on_one_network() {
        let p = policy(&[
            (ENV_MODE, "enforce"),
            (ENV_ALLOWLIST, &format!("{UNKNOWN}")),
            ("EIP6492_FACTORY_ALLOWLIST_HYPEREVM", ""),
        ]);
        assert_eq!(p.judge(Network::Base, UNKNOWN), Verdict::Allowed);
        assert_eq!(p.judge(Network::Base, COINBASE_V1), Verdict::OutsideRefused);
        // An empty value on one network lists no factory there.
        assert_eq!(p.judge(Network::HyperEvm, UNKNOWN), Verdict::OutsideRefused);
        assert!(p.factories(Network::HyperEvm).is_empty());
    }

    #[test]
    fn a_garbage_value_is_ignored_whole_and_never_passes_for_a_list() {
        let p = policy(&[
            (ENV_MODE, "enfroce"),
            (
                ENV_ALLOWLIST,
                "0x0BA5ED0c6AA8c49038F819E587E2633c4A9F428a, not-an-address",
            ),
        ]);
        assert_eq!(p.mode(), Mode::Log, "a typo is log, never enforce");
        assert_eq!(p.factories(Network::Base).len(), DEFAULT_FACTORIES.len());
    }

    /// The gas a deployment is held to is the `exact-deploy` ceiling, the
    /// one the send path enforces, read and not copied.
    #[test]
    fn the_published_policy_carries_each_networks_deploy_ceiling() {
        let doc = policy(&[]).document();
        assert_eq!(doc["mode"], json!("log"));
        assert_eq!(
            doc["sponsoring"],
            json!(false),
            "no deployment is sponsored today"
        );
        assert_eq!(doc["refusal"]["invalidReason"], json!(REFUSAL_CODE));
        for network in evm_networks() {
            let entry = &doc["perNetwork"][network.to_string()];
            assert_eq!(
                entry["deployGasCeiling"],
                json!(gas_ceiling::ceiling(network, GasKind::ExactSettleDeploy)),
                "{network}"
            );
            assert_eq!(
                entry["factories"].as_array().unwrap().len(),
                DEFAULT_FACTORIES.len()
            );
        }
        assert_eq!(
            doc["env"]["deployGasCeiling"],
            json!("EVM_MAX_GAS_EXACT_DEPLOY_<NETWORK>")
        );
    }
}
