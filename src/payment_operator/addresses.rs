//! Contract addresses for x402r Escrow Scheme
//!
//! CREATE3 unified deployment (2026-03-22): all infrastructure contracts share
//! the same address on every chain. Only PaymentOperator instances (deployed
//! via the factory) differ per chain.
//!
//! Source: https://github.com/BackTrackCo/x402r-sdk/blob/main/packages/core/src/config/index.ts
//!
//! Supported networks (11 total):
//! - Base Sepolia (testnet)
//! - Ethereum Sepolia (testnet)
//! - Arbitrum Sepolia (testnet)
//! - Base Mainnet
//! - Ethereum Mainnet
//! - Polygon PoS
//! - Arbitrum One
//! - Celo
//! - Monad
//! - Avalanche C-Chain
//! - Optimism
//! - SKALE Base (gasless L3, CREDIT gas token)

use alloy::primitives::{address, Address};

use crate::network::Network;

// ============================================================================
// CREATE3 Unified Addresses (same on ALL chains)
// ============================================================================

/// Infrastructure contracts deployed via CREATE3 — identical address on every chain.
pub mod create3 {
    use super::*;

    // Core
    pub const ESCROW: Address = address!("e050bB89eD43BB02d71343063824614A7fb80B77");
    pub const TOKEN_COLLECTOR: Address = address!("cE66Ab399EDA513BD12760b6427C87D6602344a7");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("7e868A42a458fa2443b6259419aA6A8a161E08c8");

    // Factories
    pub const FACTORY_PAYMENT_OPERATOR: Address = address!("dc41F932dF2d22346F218E4f5650694c650ab863");
    pub const FACTORY_REFUND_REQUEST: Address = address!("9cD87Bb58553Ef5ad90Ed6260EBdB906a50D6b83");
    pub const FACTORY_REFUND_REQUEST_EVIDENCE: Address = address!("3769Be76BBEa31345A2B2d84EF90683E9A377e00");
    pub const FACTORY_ESCROW_PERIOD: Address = address!("15DB06aADEB3a39D47756Bf864a173cc48bafe24");
    pub const FACTORY_FREEZE: Address = address!("df129EFFE040c3403aca597c0F0bb704859a78Fd");
    pub const FACTORY_STATIC_FEE_CALCULATOR: Address = address!("6CDdBdB46e2d7Caae31A6b213B59a1412d7f16Ac");
    pub const FACTORY_STATIC_ADDRESS_CONDITION: Address = address!("fB09350b200fda7dDd06565F5296A0CA625311d5");
    pub const FACTORY_AND_CONDITION: Address = address!("5a1F3b6d030D25a2B86aAE469Ae1216ef3be308D");
    pub const FACTORY_OR_CONDITION: Address = address!("101B2fac8cdC6348E541A0ef087275dA62AA13A0");
    pub const FACTORY_NOT_CONDITION: Address = address!("1D58f97843579356863d3393ebe24feEd76ceefF");
    pub const FACTORY_RECORDER_COMBINATOR: Address = address!("ACf2b5e21CFc14135C9cD43ebE96a481F184C1A1");
    pub const FACTORY_SIGNATURE_CONDITION: Address = address!("669B4930f9E72884725F5C7D837Ab9517eA3040f");

    // Singletons
    pub const USDC_TVL_LIMIT: Address = address!("0F1F26719219CfAdC8a1C80D2216098A0534a091");
    pub const ARBITER_REGISTRY: Address = address!("1c2d7d5978d46a943FA98aC9a649519C1424FB3e");
    pub const RECEIVER_REFUND_COLLECTOR: Address = address!("E5500a38BE45a6C598420fbd7867ac85EC451A07");

    // Condition singletons
    pub const CONDITION_PAYER: Address = address!("33F5F1154A02d0839266EFd23Fd3b85a3505bB4B");
    pub const CONDITION_RECEIVER: Address = address!("F41974A853940Ff4c18d46B6565f973c1180E171");
    pub const CONDITION_ALWAYS_TRUE: Address = address!("b295df7E7f786fd84D614AB26b1f2e86026C3483");
}

// ============================================================================
// Network Support
// ============================================================================

/// All networks that have x402r escrow contracts deployed.
pub const ESCROW_NETWORKS: &[Network] = &[
    Network::BaseSepolia,
    Network::Base,
    Network::EthereumSepolia,
    Network::Ethereum,
    Network::Polygon,
    Network::Arbitrum,
    Network::Celo,
    Network::Monad,
    Network::Avalanche,
    Network::Optimism,
    Network::SkaleBase,
];

// ============================================================================
// Helper Functions
// ============================================================================

/// Get escrow address for a given network (CREATE3 = same on all supported networks)
pub fn escrow_for_network(network: Network) -> Option<Address> {
    if is_supported(network) { Some(create3::ESCROW) } else { None }
}

/// Get factory address for a given network (CREATE3 = same on all supported networks)
pub fn factory_for_network(network: Network) -> Option<Address> {
    if is_supported(network) { Some(create3::FACTORY_PAYMENT_OPERATOR) } else { None }
}

/// Get token collector address for a given network (CREATE3 = same on all supported networks)
pub fn token_collector_for_network(network: Network) -> Option<Address> {
    if is_supported(network) { Some(create3::TOKEN_COLLECTOR) } else { None }
}

/// Get protocol fee config address for a given network (CREATE3 = same on all supported networks)
pub fn protocol_fee_config_for_network(network: Network) -> Option<Address> {
    if is_supported(network) { Some(create3::PROTOCOL_FEE_CONFIG) } else { None }
}

/// Get refund request factory address for a given network (CREATE3 = same on all supported networks)
pub fn refund_request_for_network(network: Network) -> Option<Address> {
    if is_supported(network) { Some(create3::FACTORY_REFUND_REQUEST) } else { None }
}

/// Check if a network supports the escrow scheme
pub fn is_supported(network: Network) -> bool {
    ESCROW_NETWORKS.contains(&network)
}

// ============================================================================
// Per-Network Operator Addresses
// ============================================================================

/// All escrow contract addresses for a network.
/// Infrastructure addresses are CREATE3-unified; only `payment_operators` differs per chain.
#[derive(Debug, Clone)]
pub struct OperatorAddresses {
    pub escrow: Address,
    pub factory: Address,
    pub payment_operators: Vec<Address>,
    pub token_collector: Address,
    pub protocol_fee_config: Address,
    pub refund_request: Address,
}

impl OperatorAddresses {
    /// Build an OperatorAddresses with CREATE3 infrastructure and per-chain operators.
    fn with_operators(operators: Vec<Address>) -> Self {
        Self {
            escrow: create3::ESCROW,
            factory: create3::FACTORY_PAYMENT_OPERATOR,
            token_collector: create3::TOKEN_COLLECTOR,
            protocol_fee_config: create3::PROTOCOL_FEE_CONFIG,
            refund_request: create3::FACTORY_REFUND_REQUEST,
            payment_operators: operators,
        }
    }

    /// Get addresses for a network.
    ///
    /// Infrastructure addresses are CREATE3-unified (same on all chains).
    /// PaymentOperator addresses are per-chain, deployed via the factory.
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            // Testnets
            Network::BaseSepolia => Some(Self::with_operators(vec![
                address!("7D092ec506B3D43EB87846F9c9739303785D7B2f"), // Permissionless testnet operator
            ])),
            Network::EthereumSepolia => Some(Self::with_operators(vec![
                address!("a8d2432C7ab8bA551feC15e09b64F44505e72b36"), // Permissionless testnet operator
            ])),

            // Mainnets
            Network::Base => Some(Self::with_operators(vec![
                address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb"), // Fase 5 (1300bps, OR release, Facilitator-only refund)
                address!("030353642B936c9D4213caD7BcB0fB8a1489cBe5"), // Fase 4 (OR release, Facilitator-only refund, feeCalculator=0)
            ])),
            Network::Ethereum => Some(Self::with_operators(vec![
                address!("69B67962ffb7c5C7078ff348a87DF604dfA8001b"), // Fase 5
            ])),
            Network::Polygon => Some(Self::with_operators(vec![
                address!("B87F1ECC85f074e50df3DD16A1F40e4e1EC4102e"), // Fase 5
            ])),
            Network::Arbitrum => Some(Self::with_operators(vec![
                address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // Fase 5
            ])),
            Network::Celo => Some(Self::with_operators(vec![
                address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // Fase 5
            ])),
            Network::Monad => Some(Self::with_operators(vec![
                address!("9620Dbe2BB549E1d080Dc8e7982623A9e1Df8cC3"), // Fase 5
            ])),
            Network::Avalanche => Some(Self::with_operators(vec![
                address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // Fase 5
            ])),
            Network::Optimism => Some(Self::with_operators(vec![
                address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // Fase 5
            ])),

            // SKALE Base (gasless L3, CREDIT gas token, legacy tx only)
            // PaymentOperator not yet deployed -- will be deployed via factory
            Network::SkaleBase => Some(Self::with_operators(vec![])),

            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escrow_for_all_supported_networks() {
        for &network in ESCROW_NETWORKS {
            assert!(
                escrow_for_network(network).is_some(),
                "escrow_for_network should return Some for {:?}",
                network
            );
        }
    }

    #[test]
    fn test_is_supported() {
        for &network in ESCROW_NETWORKS {
            assert!(is_supported(network), "{:?} should be supported", network);
        }
        assert!(!is_supported(Network::Solana));
        assert!(!is_supported(Network::HyperEvm));
    }

    #[test]
    fn test_escrow_networks_count() {
        assert_eq!(ESCROW_NETWORKS.len(), 11);
    }

    #[test]
    fn test_operator_addresses_all_networks() {
        for &network in ESCROW_NETWORKS {
            let addrs = OperatorAddresses::for_network(network);
            assert!(
                addrs.is_some(),
                "OperatorAddresses::for_network should return Some for {:?}",
                network
            );
        }
    }

    #[test]
    fn test_create3_unified_addresses() {
        // All supported networks must return the same infrastructure addresses
        for &network in ESCROW_NETWORKS {
            let addrs = OperatorAddresses::for_network(network).unwrap();
            assert_eq!(addrs.escrow, create3::ESCROW, "escrow mismatch for {:?}", network);
            assert_eq!(addrs.factory, create3::FACTORY_PAYMENT_OPERATOR, "factory mismatch for {:?}", network);
            assert_eq!(addrs.token_collector, create3::TOKEN_COLLECTOR, "token_collector mismatch for {:?}", network);
            assert_eq!(addrs.protocol_fee_config, create3::PROTOCOL_FEE_CONFIG, "protocol_fee_config mismatch for {:?}", network);
            assert_eq!(addrs.refund_request, create3::FACTORY_REFUND_REQUEST, "refund_request mismatch for {:?}", network);
        }
    }

    #[test]
    fn test_base_mainnet_has_payment_operators() {
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert!(!addrs.payment_operators.is_empty());
        assert_eq!(addrs.payment_operators.len(), 2);
    }

    #[test]
    fn test_skale_base_supported() {
        assert!(is_supported(Network::SkaleBase));
        let addrs = OperatorAddresses::for_network(Network::SkaleBase).unwrap();
        assert_eq!(addrs.escrow, create3::ESCROW);
        assert_eq!(addrs.factory, create3::FACTORY_PAYMENT_OPERATOR);
        // No operators deployed yet
        assert!(addrs.payment_operators.is_empty());
    }

    #[test]
    fn test_factory_for_all_networks() {
        for &network in ESCROW_NETWORKS {
            assert!(
                factory_for_network(network).is_some(),
                "factory_for_network should return Some for {:?}",
                network
            );
        }
    }

    #[test]
    fn test_token_collector_for_all_networks() {
        for &network in ESCROW_NETWORKS {
            assert!(
                token_collector_for_network(network).is_some(),
                "token_collector_for_network should return Some for {:?}",
                network
            );
        }
    }

    #[test]
    fn test_unsupported_network_returns_none() {
        assert!(escrow_for_network(Network::Solana).is_none());
        assert!(factory_for_network(Network::Solana).is_none());
        assert!(OperatorAddresses::for_network(Network::Solana).is_none());
    }
}
