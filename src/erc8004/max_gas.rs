//! A per-network ceiling on the gas of each ERC-8004 transaction on EVM.
//!
//! The daily limit ([`super::daily_cap`]) bounds how many writes a network
//! serves in a UTC day; it does not bound what they cost. On a network with
//! a ceiling, `chain::evm::send_call_estimated` -- the path of every ERC-8004
//! transaction the facilitator signs as itself (mint, delivery, retirement,
//! rating, response, revocation) -- holds each one to it:
//!
//! - an estimate above the ceiling is refused before a nonce is reserved and
//!   before the write's daily slot is kept: nothing is sent, and the route
//!   answers `400` with `errorCode` [`GAS_OVER_LIMIT_CODE`];
//! - a transaction whose gas could not be estimated is sent with the ceiling
//!   as its gas limit, never with whatever the filler finds.
//!
//! The EIP-7702 relays (`/feedback/evm/submit`, `send_transaction_from`) do
//! not go through it; none is served on a network without a `FeedbackDelegate`,
//! which today includes both networks with a built-in ceiling.
//!
//! One write can send more than one transaction under its daily slot: a
//! registration sends its mint and its delivery, and when the delivery fails,
//! a retirement (a retry of it can restore and deliver again). So on such a
//! network the most one UTC day of writes can cost is the daily limit x the
//! transactions one write sends x the ceiling x the gas price.
//!
//! # Configuration
//!
//! Built in only for Robinhood Chain and its testnet (2.56.0). Every other
//! network has none and sends exactly what it sent before.
//! `ERC8004_MAX_GAS_<NETWORK>` sets one network's ceiling, where `<NETWORK>`
//! is its v1 name in upper case with `-` as `_` (`ERC8004_MAX_GAS_ROBINHOOD`,
//! `ERC8004_MAX_GAS_ROBINHOOD_TESTNET`), and overrides the built-in value. A
//! value that is not a whole number from [`MIN_CEILING`] -- the gas of the
//! plainest transaction -- to [`MAX_CEILING`] is ignored with a warning, and
//! the built-in value stays: a variable can raise or lower a ceiling, never
//! remove one. `GET /config` publishes the ceilings in force.

use std::collections::HashMap;
use std::sync::LazyLock;

use tracing::{info, warn};

use crate::network::Network;

/// Prefix of the per-network variable, followed by the network's v1 name.
pub const ENV_MAX_GAS_PREFIX: &str = "ERC8004_MAX_GAS_";

/// The `errorCode` of a write refused for an estimate above its network's
/// ceiling.
pub const GAS_OVER_LIMIT_CODE: &str = "erc8004_gas_over_limit";

/// The smallest ceiling a variable may set: the intrinsic gas of any
/// transaction. Below it every write would be refused, and one sent without
/// an estimate could not be mined.
pub const MIN_CEILING: u64 = 21_000;

/// The largest ceiling a variable may set. A ceiling sits near what one
/// ERC-8004 transaction needs (the built-in one is 1_000_000); one thirty
/// times that would hold nothing back, so a variable naming more is read as a
/// mistake, like `0`, and the built-in value stays.
pub const MAX_CEILING: u64 = 30_000_000;

/// Built-in ceilings. 1_000_000 covers the ERC-8004 writes measured on
/// Robinhood Chain on 2026-11-04 (`eth_estimateGas`, read only): a `register`
/// with 1 KiB of metadata at 915_873 (961_860 later that day with a longer
/// URI: the margin is thin there), a `register` with a URI alone at 134_949
/// to 180_467 and a `giveFeedback` with short tags at 216_961.
const BUILT_IN_MAX_GAS: &[(Network, u64)] = &[
    (Network::Robinhood, 1_000_000),
    (Network::RobinhoodTestnet, 1_000_000),
];

/// The ceilings in force, read from the environment on first use.
pub fn global() -> &'static GasCeilings {
    static GLOBAL: LazyLock<GasCeilings> = LazyLock::new(|| {
        let ceilings = GasCeilings::from_env();
        let shown: Vec<String> = ceilings
            .configured()
            .iter()
            .map(|(network, gas)| format!("{network}={gas}"))
            .collect();
        info!(
            per_network = %shown.join(","),
            "ERC-8004 gas ceiling per write configured"
        );
        ceilings
    });
    &GLOBAL
}

/// The ceiling in force for one ERC-8004 write on `network`, if it has one.
pub fn ceiling(network: Network) -> Option<u64> {
    global().ceiling(&network)
}

/// Per-network gas ceilings for one ERC-8004 write.
pub struct GasCeilings {
    per_network: HashMap<Network, u64>,
}

impl GasCeilings {
    /// The built-in ceilings and the process environment's overrides.
    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    /// The built-in ceilings and the overrides `lookup` answers, by variable
    /// name.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut per_network: HashMap<Network, u64> = BUILT_IN_MAX_GAS.iter().copied().collect();
        for network in Network::variants() {
            let var = env_var_for(network);
            if let Some(raw) = lookup(&var) {
                if let Some(gas) = parse_ceiling(&var, &raw) {
                    per_network.insert(*network, gas);
                }
            }
        }
        Self { per_network }
    }

    /// The ceiling for `network`, or `None` when it has none.
    pub fn ceiling(&self, network: &Network) -> Option<u64> {
        self.per_network.get(network).copied()
    }

    /// Every network's ceiling, sorted by network name -- what `GET /config`
    /// publishes.
    pub fn configured(&self) -> Vec<(Network, u64)> {
        let mut ceilings: Vec<(Network, u64)> =
            self.per_network.iter().map(|(n, g)| (*n, *g)).collect();
        ceilings.sort_by_key(|(n, _)| n.to_string());
        ceilings
    }
}

fn env_var_for(network: &Network) -> String {
    format!(
        "{ENV_MAX_GAS_PREFIX}{}",
        network.to_string().to_uppercase().replace('-', "_")
    )
}

fn parse_ceiling(var: &str, raw: &str) -> Option<u64> {
    match raw.trim().parse::<u64>() {
        Ok(gas) if (MIN_CEILING..=MAX_CEILING).contains(&gas) => Some(gas),
        _ => {
            warn!(
                var,
                value = raw,
                min = MIN_CEILING,
                max = MAX_CEILING,
                "ignoring an ERC-8004 gas ceiling that is not a whole number within the bounds"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> GasCeilings {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        GasCeilings::from_lookup(move |var| vars.get(var).cloned())
    }

    /// Robinhood Chain and its testnet have a ceiling each, and no other
    /// network has one: everywhere else the writes send what they sent before.
    #[test]
    fn only_robinhood_and_its_testnet_have_a_built_in_ceiling() {
        let built_in = with(&[]);
        assert_eq!(built_in.ceiling(&Network::Robinhood), Some(1_000_000));
        assert_eq!(
            built_in.ceiling(&Network::RobinhoodTestnet),
            Some(1_000_000)
        );
        for network in Network::variants() {
            if !matches!(network, Network::Robinhood | Network::RobinhoodTestnet) {
                assert_eq!(built_in.ceiling(network), None, "{network}");
            }
        }
        assert_eq!(
            built_in.configured(),
            vec![
                (Network::Robinhood, 1_000_000),
                (Network::RobinhoodTestnet, 1_000_000)
            ]
        );
    }

    /// A variable overrides its network's ceiling or gives one to a network
    /// without it; a value that is not a number, or is outside the bounds,
    /// leaves the built-in value in place, so no variable removes a ceiling.
    #[test]
    fn variables_override_the_built_in_ceilings() {
        let ceilings = with(&[
            ("ERC8004_MAX_GAS_ROBINHOOD", " 250000 "),
            ("ERC8004_MAX_GAS_BASE", "2000000"),
            ("ERC8004_MAX_GAS_ROBINHOOD_TESTNET", "lots"),
            ("ERC8004_MAX_GAS_POLYGON", "0"),
            ("ERC8004_MAX_GAS_ARC", "20999"),
            ("ERC8004_MAX_GAS_CELO", "-5"),
            ("ERC8004_MAX_GAS_ETHEREUM", "30000001"),
        ]);
        assert_eq!(ceilings.ceiling(&Network::Ethereum), None);
        assert_eq!(ceilings.ceiling(&Network::Robinhood), Some(250_000));
        assert_eq!(ceilings.ceiling(&Network::Base), Some(2_000_000));
        assert_eq!(
            ceilings.ceiling(&Network::RobinhoodTestnet),
            Some(1_000_000)
        );
        assert_eq!(ceilings.ceiling(&Network::Polygon), None);
        assert_eq!(ceilings.ceiling(&Network::Arc), None);
        assert_eq!(ceilings.ceiling(&Network::Celo), None);

        let at_minimum = with(&[("ERC8004_MAX_GAS_ROBINHOOD", "21000")]);
        assert_eq!(at_minimum.ceiling(&Network::Robinhood), Some(MIN_CEILING));
        let at_maximum = with(&[("ERC8004_MAX_GAS_ROBINHOOD", "30000000")]);
        assert_eq!(at_maximum.ceiling(&Network::Robinhood), Some(MAX_CEILING));
        let removed = with(&[("ERC8004_MAX_GAS_ROBINHOOD", "18446744073709551615")]);
        assert_eq!(removed.ceiling(&Network::Robinhood), Some(1_000_000));
    }

    /// The variable is read from the process environment under the name the
    /// module documents. Celo Sepolia, on which no test sends a write.
    #[test]
    fn the_process_environment_is_read_under_the_documented_name() {
        let var = "ERC8004_MAX_GAS_CELO_SEPOLIA";
        assert_eq!(env_var_for(&Network::CeloSepolia), var);
        assert_eq!(
            env_var_for(&Network::RobinhoodTestnet),
            "ERC8004_MAX_GAS_ROBINHOOD_TESTNET"
        );
        std::env::set_var(var, "123456");
        let ceilings = GasCeilings::from_env();
        std::env::remove_var(var);
        assert_eq!(ceilings.ceiling(&Network::CeloSepolia), Some(123_456));
    }
}
