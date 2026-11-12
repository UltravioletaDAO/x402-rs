//! A gas ceiling on every EVM transaction the facilitator signs and pays for,
//! per network and per kind of call.
//!
//! Most of what the facilitator pays gas for runs code somebody else chose. A
//! payer that is a contract (EIP-1271, or an EOA delegated by EIP-7702) answers
//! `isValidSignature` inside an `exact` or `upto` settle; an ERC-721
//! recipient's `onERC721Received` runs inside an identity delivery; a rater's
//! delegated code runs inside a relayed rating; the `refund` extension calls
//! the proxy its request names. The node's estimate is whatever that code
//! burns, and it was signed as estimated. Every send now names its
//! [`GasKind`], and `chain::evm` holds it to that kind's ceiling on its
//! network:
//!
//! - an estimate above the ceiling is refused before a nonce is reserved and
//!   before an ERC-8004 write keeps its daily slot: nothing is sent;
//! - the gas limit sent is never above the ceiling (`send_transaction_from`
//!   adds its quarter of head-room up to it);
//! - a call whose gas could not be estimated (a transport failure, not a
//!   revert) is sent with the ceiling as its gas limit, never with whatever
//!   the filler finds.
//!
//! An ERC-8004 write the facilitator signs as itself is also held to
//! `erc8004::max_gas` where its network has a ceiling there: the lower one
//! applies.
//!
//! # Built-in values
//!
//! Each kind's value is a few times the most gas that kind of transaction was
//! measured to use, read from the facilitator's own mainnet transactions on
//! 2026-11-12 (public explorers, about 3,300 transactions on Ethereum, Celo,
//! Optimism, Avalanche and Unichain; receipts on Base and Arbitrum):
//! `exact` 104,927, `upto` 99,566 (one Base settle), PaymentOperator 238,192
//! (authorize; Polygon's were sent with limits of about 357,783), relayed
//! rating 312,814, mint 260,645 (961,860 estimated with 1 KiB of metadata on
//! Robinhood Chain), delivery 75,951, rating 215,580.
//! The head-room above them is for contract wallets, whose own signature check
//! (a passkey verified without a P-256 precompile costs a few hundred thousand)
//! runs inside the payment. The deposit of the `refund` extension and the
//! EIP-6492 deploy have no measured transaction; their values are provisional.
//!
//! The same contract costs the same gas on every network measured except
//! where the gas schedule differs: a FiatToken `transfer` estimated 39,945 to
//! 40,305 on Ethereum, Base, Optimism, Arbitrum (whose L1 share was about 1%),
//! Celo, Avalanche, Unichain, HyperEVM and HashKey, up to 1.42 times that on
//! Scroll, SKALE and Polygon, and 1.94 times on Monad, which therefore gets
//! twice every value ([`SCHEDULE_FACTOR`]).
//!
//! # Configuration
//!
//! `EVM_MAX_GAS_<CALL>_<NETWORK>` sets one ceiling, where `<CALL>` is the
//! kind's name ([`GasKind::as_str`]) and `<NETWORK>` the network's v1 name,
//! both upper-cased with `-` as `_` (`EVM_MAX_GAS_EXACT_BASE`,
//! `EVM_MAX_GAS_ERC8004_RELAY_BASE_SEPOLIA`). A value that is not a whole
//! number from [`MIN_CEILING`] to [`MAX_CEILING`] is ignored with a warning,
//! and the built-in value stays: a variable can raise or lower a ceiling,
//! never remove one, and never stop the process. `GET /config` publishes the
//! ceilings in force (`evmGasCeilings`).

use std::collections::HashMap;
use std::sync::LazyLock;

use tracing::{info, warn};

pub use crate::erc8004::max_gas::{MAX_CEILING, MIN_CEILING};
use crate::network::{Network, NetworkFamily};

/// Prefix of the per-call variable, followed by the call and the network.
pub const ENV_PREFIX: &str = "EVM_MAX_GAS_";

/// The error token of a transaction refused for an estimate above its
/// ceiling, outside ERC-8004 (whose routes answer
/// `erc8004::max_gas::GAS_OVER_LIMIT_CODE`).
pub const GAS_OVER_LIMIT_CODE: &str = "gas_over_limit";

/// What a transaction the facilitator pays for does, for its gas ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GasKind {
    /// An `exact` settle: the token's `transferWithAuthorization`, signed by
    /// an EOA or by a deployed contract wallet (EIP-1271).
    ExactSettle,
    /// An `exact` settle that first deploys the payer's wallet through its
    /// EIP-6492 factory, in one Multicall3 transaction. Unreachable while an
    /// EIP-6492 signature cannot pass the 65-byte rule.
    ExactSettleDeploy,
    /// An `upto` settle through the Permit2 proxy.
    UptoSettle,
    /// The `refund` extension's `executeDeposit` on the merchant's relay proxy.
    EscrowDeposit,
    /// A PaymentOperator call: authorize, charge, capture or release, refund,
    /// void.
    PaymentOperator,
    /// A relayed ERC-8004 rating or response (EIP-7702), sent to the author.
    Erc8004Relay,
    /// An ERC-8004 identity mint (`register`).
    Erc8004Register,
    /// An ERC-8004 identity delivery (`safeTransferFrom` to the recipient).
    Erc8004Transfer,
    /// Any other ERC-8004 write the facilitator signs as itself: a rating, a
    /// response, a revocation, an identity's URI.
    Erc8004Write,
}

impl GasKind {
    /// Every kind, in the order `GET /config` lists them.
    pub const ALL: [GasKind; 9] = [
        GasKind::ExactSettle,
        GasKind::ExactSettleDeploy,
        GasKind::UptoSettle,
        GasKind::EscrowDeposit,
        GasKind::PaymentOperator,
        GasKind::Erc8004Relay,
        GasKind::Erc8004Register,
        GasKind::Erc8004Transfer,
        GasKind::Erc8004Write,
    ];

    /// The kind's name in `GET /config` and, upper-cased, in its variable.
    pub fn as_str(self) -> &'static str {
        match self {
            GasKind::ExactSettle => "exact",
            GasKind::ExactSettleDeploy => "exact-deploy",
            GasKind::UptoSettle => "upto",
            GasKind::EscrowDeposit => "escrow-deposit",
            GasKind::PaymentOperator => "operator",
            GasKind::Erc8004Relay => "erc8004-relay",
            GasKind::Erc8004Register => "erc8004-register",
            GasKind::Erc8004Transfer => "erc8004-transfer",
            GasKind::Erc8004Write => "erc8004-write",
        }
    }

    /// The built-in ceiling where the gas schedule is Ethereum's (see the
    /// module documentation for what each value covers).
    fn built_in(self) -> u64 {
        match self {
            GasKind::ExactSettle => 500_000,
            GasKind::ExactSettleDeploy => 1_000_000,
            GasKind::UptoSettle => 500_000,
            GasKind::EscrowDeposit => 600_000,
            GasKind::PaymentOperator => 800_000,
            GasKind::Erc8004Relay => 1_000_000,
            GasKind::Erc8004Register => 1_000_000,
            GasKind::Erc8004Transfer => 300_000,
            GasKind::Erc8004Write => 1_000_000,
        }
    }
}

/// Networks whose gas schedule charges the same call more than Ethereum's,
/// and the factor every built-in value is multiplied by there. Monad prices
/// cold state access higher: the FiatToken `transfer` that estimated 39,945
/// on Ethereum's schedule estimated 77,573 there (2026-11-12).
const SCHEDULE_FACTOR: &[(Network, u64)] = &[(Network::Monad, 2)];

/// The built-in ceiling of `kind` on `network`.
fn built_in(network: Network, kind: GasKind) -> u64 {
    let factor = SCHEDULE_FACTOR
        .iter()
        .find(|(n, _)| *n == network)
        .map_or(1, |(_, factor)| *factor);
    kind.built_in().saturating_mul(factor)
}

/// The ceilings in force, read from the environment on first use.
pub fn global() -> &'static GasCeilings {
    static GLOBAL: LazyLock<GasCeilings> = LazyLock::new(|| {
        let ceilings = GasCeilings::from_env();
        let mut shown: Vec<String> = ceilings
            .overrides
            .iter()
            .map(|((network, kind), gas)| format!("{network}/{}={gas}", kind.as_str()))
            .collect();
        shown.sort();
        info!(
            overrides = %shown.join(","),
            "EVM gas ceiling per call configured"
        );
        ceilings
    });
    &GLOBAL
}

/// The ceiling in force for one `kind` transaction on `network`.
pub fn ceiling(network: Network, kind: GasKind) -> u64 {
    global().ceiling(network, kind)
}

/// Per-network, per-call gas ceilings.
pub struct GasCeilings {
    overrides: HashMap<(Network, GasKind), u64>,
}

impl GasCeilings {
    /// The built-in ceilings and the process environment's overrides.
    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    /// The built-in ceilings and the overrides `lookup` answers, by variable
    /// name.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut overrides = HashMap::new();
        for network in evm_networks() {
            for kind in GasKind::ALL {
                let var = env_var_for(network, kind);
                if let Some(raw) = lookup(&var) {
                    if let Some(gas) = parse_ceiling(&var, &raw) {
                        overrides.insert((network, kind), gas);
                    }
                }
            }
        }
        Self { overrides }
    }

    /// The ceiling of one `kind` transaction on `network`.
    pub fn ceiling(&self, network: Network, kind: GasKind) -> u64 {
        self.overrides
            .get(&(network, kind))
            .copied()
            .unwrap_or_else(|| built_in(network, kind))
    }

    /// Every EVM network's ceilings, sorted by network name, each kind in
    /// [`GasKind::ALL`] order -- what `GET /config` publishes.
    pub fn configured(&self) -> Vec<(Network, Vec<(GasKind, u64)>)> {
        let mut networks: Vec<Network> = evm_networks().collect();
        networks.sort_by_key(|n| n.to_string());
        networks
            .into_iter()
            .map(|network| {
                let kinds = GasKind::ALL
                    .iter()
                    .map(|kind| (*kind, self.ceiling(network, *kind)))
                    .collect();
                (network, kinds)
            })
            .collect()
    }
}

/// The EVM networks this build knows.
fn evm_networks() -> impl Iterator<Item = Network> {
    Network::variants()
        .iter()
        .copied()
        .filter(|network| matches!(NetworkFamily::from(*network), NetworkFamily::Evm))
}

/// `EVM_MAX_GAS_<CALL>_<NETWORK>`.
pub fn env_var_for(network: Network, kind: GasKind) -> String {
    format!(
        "{ENV_PREFIX}{}_{}",
        kind.as_str().to_uppercase().replace('-', "_"),
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
                "ignoring an EVM gas ceiling that is not a whole number within the bounds"
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

    /// The built-in values, each kind its own, the same on every network of
    /// Ethereum's gas schedule and twice that on Monad.
    #[test]
    fn every_call_has_its_own_built_in_ceiling() {
        let built_in = with(&[]);
        let expected = [
            (GasKind::ExactSettle, 500_000),
            (GasKind::ExactSettleDeploy, 1_000_000),
            (GasKind::UptoSettle, 500_000),
            (GasKind::EscrowDeposit, 600_000),
            (GasKind::PaymentOperator, 800_000),
            (GasKind::Erc8004Relay, 1_000_000),
            (GasKind::Erc8004Register, 1_000_000),
            (GasKind::Erc8004Transfer, 300_000),
            (GasKind::Erc8004Write, 1_000_000),
        ];
        for network in evm_networks() {
            let factor = if network == Network::Monad { 2 } else { 1 };
            for (kind, gas) in expected {
                assert_eq!(
                    built_in.ceiling(network, kind),
                    gas * factor,
                    "{network} {}",
                    kind.as_str()
                );
            }
        }
        assert_eq!(GasKind::ALL.len(), expected.len());
    }

    /// A variable sets one (call, network) ceiling and nothing else; a value
    /// that is not a number, or is outside the bounds, leaves the built-in
    /// value in place, so no variable removes a ceiling or stops the process.
    #[test]
    fn variables_override_one_call_on_one_network() {
        let ceilings = with(&[
            ("EVM_MAX_GAS_EXACT_BASE", " 250000 "),
            ("EVM_MAX_GAS_ERC8004_RELAY_BASE_SEPOLIA", "2000000"),
            ("EVM_MAX_GAS_UPTO_POLYGON", "lots"),
            ("EVM_MAX_GAS_OPERATOR_CELO", "0"),
            ("EVM_MAX_GAS_EXACT_ARC", "20999"),
            ("EVM_MAX_GAS_EXACT_ETHEREUM", "-5"),
            ("EVM_MAX_GAS_EXACT_OPTIMISM", "30000001"),
            ("EVM_MAX_GAS_EXACT_MONAD", "18446744073709551615"),
        ]);
        assert_eq!(
            ceilings.ceiling(Network::Base, GasKind::ExactSettle),
            250_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Base, GasKind::UptoSettle),
            500_000
        );
        assert_eq!(
            ceilings.ceiling(Network::BaseSepolia, GasKind::Erc8004Relay),
            2_000_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Polygon, GasKind::UptoSettle),
            500_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Celo, GasKind::PaymentOperator),
            800_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Arc, GasKind::ExactSettle),
            500_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Ethereum, GasKind::ExactSettle),
            500_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Optimism, GasKind::ExactSettle),
            500_000
        );
        assert_eq!(
            ceilings.ceiling(Network::Monad, GasKind::ExactSettle),
            1_000_000
        );

        let at_minimum = with(&[("EVM_MAX_GAS_EXACT_BASE", "21000")]);
        assert_eq!(
            at_minimum.ceiling(Network::Base, GasKind::ExactSettle),
            MIN_CEILING
        );
        let at_maximum = with(&[("EVM_MAX_GAS_EXACT_BASE", "30000000")]);
        assert_eq!(
            at_maximum.ceiling(Network::Base, GasKind::ExactSettle),
            MAX_CEILING
        );
    }

    /// The variable is read from the process environment under the name the
    /// module documents, and no two (call, network) pairs share a name.
    #[test]
    fn the_process_environment_is_read_under_the_documented_names() {
        assert_eq!(
            env_var_for(Network::Base, GasKind::ExactSettle),
            "EVM_MAX_GAS_EXACT_BASE"
        );
        assert_eq!(
            env_var_for(Network::BaseSepolia, GasKind::Erc8004Relay),
            "EVM_MAX_GAS_ERC8004_RELAY_BASE_SEPOLIA"
        );
        let mut names = std::collections::HashSet::new();
        for network in evm_networks() {
            for kind in GasKind::ALL {
                assert!(
                    names.insert(env_var_for(network, kind)),
                    "{network} {} shares its variable",
                    kind.as_str()
                );
            }
        }

        let var = "EVM_MAX_GAS_ESCROW_DEPOSIT_CELO_SEPOLIA";
        assert_eq!(
            env_var_for(Network::CeloSepolia, GasKind::EscrowDeposit),
            var
        );
        std::env::set_var(var, "123456");
        let ceilings = GasCeilings::from_env();
        std::env::remove_var(var);
        assert_eq!(
            ceilings.ceiling(Network::CeloSepolia, GasKind::EscrowDeposit),
            123_456
        );
    }

    /// What `GET /config` lists: every EVM network this build knows, each
    /// with every call, and no network of another family.
    #[test]
    fn the_published_table_covers_every_evm_network_and_call() {
        let table = with(&[("EVM_MAX_GAS_EXACT_BASE", "250000")]).configured();
        let names: Vec<String> = table.iter().map(|(n, _)| n.to_string()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert_eq!(table.len(), evm_networks().count());
        for (network, kinds) in &table {
            assert!(matches!(NetworkFamily::from(*network), NetworkFamily::Evm));
            assert_eq!(
                kinds.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
                GasKind::ALL.to_vec()
            );
        }
        let base = &table.iter().find(|(n, _)| *n == Network::Base).unwrap().1;
        assert_eq!(base[0], (GasKind::ExactSettle, 250_000));
        assert!(!names.iter().any(|n| n == "solana" || n == "near"));
    }
}
