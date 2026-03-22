//! Contract addresses for x402r Escrow Scheme
//!
//! Hybrid address model:
//! - Existing networks (Base, Ethereum, etc.) use LEGACY per-chain addresses because
//!   their deployed PaymentOperators reference the old infrastructure contracts.
//! - New networks (SKALE, future) use CREATE3 unified addresses.
//!
//! Source: https://github.com/BackTrackCo/x402r-sdk/blob/main/packages/core/src/config/index.ts
//!
//! Supported networks (11 total):
//! - Base Sepolia (testnet)
//! - Ethereum Sepolia (testnet)
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
// CREATE3 Unified Addresses (for NEW networks only: SKALE, future chains)
// ============================================================================

/// Infrastructure contracts deployed via CREATE3 — identical address on every chain.
/// Used for SKALE and future network deployments.
/// Existing networks keep their legacy per-chain addresses for backward compatibility
/// with already-deployed PaymentOperators.
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
// Legacy Per-Chain Addresses (for existing networks with deployed operators)
// ============================================================================

/// Base Sepolia testnet (eip155:84532) — legacy deployment
pub mod base_sepolia {
    use super::*;
    pub const ESCROW: Address = address!("29025c0E9D4239d438e169570818dB9FE0A80873");
    pub const FACTORY: Address = address!("97d53e63A9CB97556c00BeFd325AF810c9b267B2");
    pub const TOKEN_COLLECTOR: Address = address!("5cA789000070DF15b4663DB64a50AeF5D49c5Ee0");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("8F96C493bAC365E41f0315cf45830069EBbDCaCe");
    pub const REFUND_REQUEST: Address = address!("1C2Ab244aC8bDdDB74d43389FF34B118aF2E90F4");
}

/// Base mainnet (eip155:8453) — legacy deployment
pub mod base_mainnet {
    use super::*;
    pub const ESCROW: Address = address!("b9488351E48b23D798f24e8174514F28B741Eb4f");
    pub const FACTORY: Address = address!("3D0837fF8Ea36F417261577b9BA568400A840260");
    pub const TOKEN_COLLECTOR: Address = address!("48ADf6E37F9b31dC2AAD0462C5862B5422C736B8");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("59314674BAbb1a24Eb2704468a9cCdD50668a1C6");
    pub const REFUND_REQUEST: Address = address!("35fb2EFEfAc3Ee9f6E52A9AAE5C9655bC08dEc00");
}

/// Ethereum Sepolia testnet (eip155:11155111) — legacy deployment
pub mod ethereum_sepolia {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
}

/// Ethereum mainnet (eip155:1) — legacy deployment
pub mod ethereum_mainnet {
    use super::*;
    pub const ESCROW: Address = address!("9D4146EF898c8E60B3e865AE254ef438E7cEd2A0");
    pub const FACTORY: Address = address!("1e52a74cE6b69F04a506eF815743E1052A1BD28F");
    pub const TOKEN_COLLECTOR: Address = address!("206D4DbB6E7b876e4B5EFAAD2a04e7d7813FB6ba");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("5b3e33791C1764cF7e2573Bf8116F1D361FD97Cd");
    pub const REFUND_REQUEST: Address = address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70");
}

/// Polygon PoS (eip155:137) — legacy deployment
pub mod polygon {
    use super::*;
    pub const ESCROW: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const FACTORY: Address = address!("b33D6502EdBbC47201cd1E53C49d703EC0a660b8");
    pub const TOKEN_COLLECTOR: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("E78648e7af7B1BaDE717FF6E410B922F92adE80f");
    pub const REFUND_REQUEST: Address = address!("ed02d3E5167BCc9582D851885A89b050AB816a56");
}

/// Arbitrum One (eip155:42161) — legacy deployment
pub mod arbitrum {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
}

/// Celo (eip155:42220) — legacy deployment
pub mod celo {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
}

/// Monad (eip155:143) — legacy deployment
pub mod monad {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
}

/// Avalanche C-Chain (eip155:43114) — legacy deployment
pub mod avalanche {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
}

/// Optimism (eip155:10) — legacy deployment
pub mod optimism {
    use super::*;
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
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

/// Get escrow address for a given network
pub fn escrow_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::ESCROW),
        Network::Base => Some(base_mainnet::ESCROW),
        Network::EthereumSepolia => Some(ethereum_sepolia::ESCROW),
        Network::Ethereum => Some(ethereum_mainnet::ESCROW),
        Network::Polygon => Some(polygon::ESCROW),
        Network::Arbitrum => Some(arbitrum::ESCROW),
        Network::Celo => Some(celo::ESCROW),
        Network::Monad => Some(monad::ESCROW),
        Network::Avalanche => Some(avalanche::ESCROW),
        Network::Optimism => Some(optimism::ESCROW),
        Network::SkaleBase => Some(create3::ESCROW),
        _ => None,
    }
}

/// Get factory address for a given network
pub fn factory_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::FACTORY),
        Network::Base => Some(base_mainnet::FACTORY),
        Network::EthereumSepolia => Some(ethereum_sepolia::FACTORY),
        Network::Ethereum => Some(ethereum_mainnet::FACTORY),
        Network::Polygon => Some(polygon::FACTORY),
        Network::Arbitrum => Some(arbitrum::FACTORY),
        Network::Celo => Some(celo::FACTORY),
        Network::Monad => Some(monad::FACTORY),
        Network::Avalanche => Some(avalanche::FACTORY),
        Network::Optimism => Some(optimism::FACTORY),
        Network::SkaleBase => Some(create3::FACTORY_PAYMENT_OPERATOR),
        _ => None,
    }
}

/// Get token collector address for a given network
pub fn token_collector_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::TOKEN_COLLECTOR),
        Network::Base => Some(base_mainnet::TOKEN_COLLECTOR),
        Network::EthereumSepolia => Some(ethereum_sepolia::TOKEN_COLLECTOR),
        Network::Ethereum => Some(ethereum_mainnet::TOKEN_COLLECTOR),
        Network::Polygon => Some(polygon::TOKEN_COLLECTOR),
        Network::Arbitrum => Some(arbitrum::TOKEN_COLLECTOR),
        Network::Celo => Some(celo::TOKEN_COLLECTOR),
        Network::Monad => Some(monad::TOKEN_COLLECTOR),
        Network::Avalanche => Some(avalanche::TOKEN_COLLECTOR),
        Network::Optimism => Some(optimism::TOKEN_COLLECTOR),
        Network::SkaleBase => Some(create3::TOKEN_COLLECTOR),
        _ => None,
    }
}

/// Get protocol fee config address for a given network
pub fn protocol_fee_config_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::PROTOCOL_FEE_CONFIG),
        Network::Base => Some(base_mainnet::PROTOCOL_FEE_CONFIG),
        Network::EthereumSepolia => Some(ethereum_sepolia::PROTOCOL_FEE_CONFIG),
        Network::Ethereum => Some(ethereum_mainnet::PROTOCOL_FEE_CONFIG),
        Network::Polygon => Some(polygon::PROTOCOL_FEE_CONFIG),
        Network::Arbitrum => Some(arbitrum::PROTOCOL_FEE_CONFIG),
        Network::Celo => Some(celo::PROTOCOL_FEE_CONFIG),
        Network::Monad => Some(monad::PROTOCOL_FEE_CONFIG),
        Network::Avalanche => Some(avalanche::PROTOCOL_FEE_CONFIG),
        Network::Optimism => Some(optimism::PROTOCOL_FEE_CONFIG),
        Network::SkaleBase => Some(create3::PROTOCOL_FEE_CONFIG),
        _ => None,
    }
}

/// Get refund request address for a given network
pub fn refund_request_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::REFUND_REQUEST),
        Network::Base => Some(base_mainnet::REFUND_REQUEST),
        Network::EthereumSepolia => Some(ethereum_sepolia::REFUND_REQUEST),
        Network::Ethereum => Some(ethereum_mainnet::REFUND_REQUEST),
        Network::Polygon => Some(polygon::REFUND_REQUEST),
        Network::Arbitrum => Some(arbitrum::REFUND_REQUEST),
        Network::Celo => Some(celo::REFUND_REQUEST),
        Network::Monad => Some(monad::REFUND_REQUEST),
        Network::Avalanche => Some(avalanche::REFUND_REQUEST),
        Network::Optimism => Some(optimism::REFUND_REQUEST),
        Network::SkaleBase => Some(create3::FACTORY_REFUND_REQUEST),
        _ => None,
    }
}

/// Check if a network supports the escrow scheme
pub fn is_supported(network: Network) -> bool {
    ESCROW_NETWORKS.contains(&network)
}

// ============================================================================
// Per-Network Operator Addresses
// ============================================================================

/// All escrow contract addresses for a network.
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
    /// Get addresses for a network.
    ///
    /// Legacy networks use per-chain infrastructure addresses.
    /// New networks (SKALE) use CREATE3 unified addresses.
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            // Testnets (legacy)
            Network::BaseSepolia => Some(Self {
                escrow: base_sepolia::ESCROW,
                factory: base_sepolia::FACTORY,
                payment_operators: vec![
                    address!("7D092ec506B3D43EB87846F9c9739303785D7B2f"),
                ],
                token_collector: base_sepolia::TOKEN_COLLECTOR,
                protocol_fee_config: base_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: base_sepolia::REFUND_REQUEST,
            }),
            Network::EthereumSepolia => Some(Self {
                escrow: ethereum_sepolia::ESCROW,
                factory: ethereum_sepolia::FACTORY,
                payment_operators: vec![
                    address!("a8d2432C7ab8bA551feC15e09b64F44505e72b36"),
                ],
                token_collector: ethereum_sepolia::TOKEN_COLLECTOR,
                protocol_fee_config: ethereum_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: ethereum_sepolia::REFUND_REQUEST,
            }),

            // Mainnets (legacy)
            Network::Base => Some(Self {
                escrow: base_mainnet::ESCROW,
                factory: base_mainnet::FACTORY,
                payment_operators: vec![
                    address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb"),
                    address!("030353642B936c9D4213caD7BcB0fB8a1489cBe5"),
                ],
                token_collector: base_mainnet::TOKEN_COLLECTOR,
                protocol_fee_config: base_mainnet::PROTOCOL_FEE_CONFIG,
                refund_request: base_mainnet::REFUND_REQUEST,
            }),
            Network::Ethereum => Some(Self {
                escrow: ethereum_mainnet::ESCROW,
                factory: ethereum_mainnet::FACTORY,
                payment_operators: vec![
                    address!("69B67962ffb7c5C7078ff348a87DF604dfA8001b"),
                ],
                token_collector: ethereum_mainnet::TOKEN_COLLECTOR,
                protocol_fee_config: ethereum_mainnet::PROTOCOL_FEE_CONFIG,
                refund_request: ethereum_mainnet::REFUND_REQUEST,
            }),
            Network::Polygon => Some(Self {
                escrow: polygon::ESCROW,
                factory: polygon::FACTORY,
                payment_operators: vec![
                    address!("B87F1ECC85f074e50df3DD16A1F40e4e1EC4102e"),
                ],
                token_collector: polygon::TOKEN_COLLECTOR,
                protocol_fee_config: polygon::PROTOCOL_FEE_CONFIG,
                refund_request: polygon::REFUND_REQUEST,
            }),
            Network::Arbitrum => Some(Self {
                escrow: arbitrum::ESCROW,
                factory: arbitrum::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"),
                ],
                token_collector: arbitrum::TOKEN_COLLECTOR,
                protocol_fee_config: arbitrum::PROTOCOL_FEE_CONFIG,
                refund_request: arbitrum::REFUND_REQUEST,
            }),
            Network::Celo => Some(Self {
                escrow: celo::ESCROW,
                factory: celo::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"),
                ],
                token_collector: celo::TOKEN_COLLECTOR,
                protocol_fee_config: celo::PROTOCOL_FEE_CONFIG,
                refund_request: celo::REFUND_REQUEST,
            }),
            Network::Monad => Some(Self {
                escrow: monad::ESCROW,
                factory: monad::FACTORY,
                payment_operators: vec![
                    address!("9620Dbe2BB549E1d080Dc8e7982623A9e1Df8cC3"),
                ],
                token_collector: monad::TOKEN_COLLECTOR,
                protocol_fee_config: monad::PROTOCOL_FEE_CONFIG,
                refund_request: monad::REFUND_REQUEST,
            }),
            Network::Avalanche => Some(Self {
                escrow: avalanche::ESCROW,
                factory: avalanche::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"),
                ],
                token_collector: avalanche::TOKEN_COLLECTOR,
                protocol_fee_config: avalanche::PROTOCOL_FEE_CONFIG,
                refund_request: avalanche::REFUND_REQUEST,
            }),
            Network::Optimism => Some(Self {
                escrow: optimism::ESCROW,
                factory: optimism::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"),
                ],
                token_collector: optimism::TOKEN_COLLECTOR,
                protocol_fee_config: optimism::PROTOCOL_FEE_CONFIG,
                refund_request: optimism::REFUND_REQUEST,
            }),

            // New networks (CREATE3)
            Network::SkaleBase => Some(Self {
                escrow: create3::ESCROW,
                factory: create3::FACTORY_PAYMENT_OPERATOR,
                payment_operators: vec![],
                token_collector: create3::TOKEN_COLLECTOR,
                protocol_fee_config: create3::PROTOCOL_FEE_CONFIG,
                refund_request: create3::FACTORY_REFUND_REQUEST,
            }),

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
    fn test_legacy_networks_use_per_chain_addresses() {
        // Base mainnet should use legacy addresses, NOT CREATE3
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert_eq!(addrs.escrow, base_mainnet::ESCROW);
        assert_eq!(addrs.token_collector, base_mainnet::TOKEN_COLLECTOR);
        assert_ne!(addrs.token_collector, create3::TOKEN_COLLECTOR);
    }

    #[test]
    fn test_skale_uses_create3_addresses() {
        let addrs = OperatorAddresses::for_network(Network::SkaleBase).unwrap();
        assert_eq!(addrs.escrow, create3::ESCROW);
        assert_eq!(addrs.token_collector, create3::TOKEN_COLLECTOR);
        assert_eq!(addrs.factory, create3::FACTORY_PAYMENT_OPERATOR);
        assert!(addrs.payment_operators.is_empty());
    }

    #[test]
    fn test_base_mainnet_has_payment_operators() {
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert!(!addrs.payment_operators.is_empty());
        assert_eq!(addrs.payment_operators.len(), 2);
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
