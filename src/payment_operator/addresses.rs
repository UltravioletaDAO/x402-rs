//! Contract addresses for x402r Escrow Scheme by network
//!
//! These addresses are from the x402r-sdk multichain deployment:
//! https://github.com/BackTrackCo/x402r-sdk/blob/A1igator/multichain-config/packages/core/src/config/index.ts
//!
//! Supported networks (10 total):
//! - Base Sepolia (testnet)
//! - Base Mainnet
//! - Ethereum Sepolia (testnet)
//! - Ethereum Mainnet
//! - Polygon PoS
//! - Arbitrum One
//! - Celo
//! - Monad
//! - Avalanche C-Chain
//! - Optimism

use alloy::primitives::{address, Address};

use crate::network::Network;

/// Contract addresses for Base Sepolia testnet (eip155:84532)
pub mod base_sepolia {
    use super::*;

    pub const ESCROW: Address = address!("29025c0E9D4239d438e169570818dB9FE0A80873");
    pub const FACTORY: Address = address!("97d53e63A9CB97556c00BeFd325AF810c9b267B2");
    pub const TOKEN_COLLECTOR: Address = address!("5cA789000070DF15b4663DB64a50AeF5D49c5Ee0");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("8F96C493bAC365E41f0315cf45830069EBbDCaCe");
    pub const REFUND_REQUEST: Address = address!("1C2Ab244aC8bDdDB74d43389FF34B118aF2E90F4");
    pub const USDC: Address = address!("036CbD53842c5426634e7929541eC2318f3dCF7e");
}

/// Contract addresses for Base mainnet (eip155:8453)
pub mod base_mainnet {
    use super::*;

    pub const ESCROW: Address = address!("b9488351E48b23D798f24e8174514F28B741Eb4f");
    pub const FACTORY: Address = address!("3D0837fF8Ea36F417261577b9BA568400A840260");
    pub const TOKEN_COLLECTOR: Address = address!("48ADf6E37F9b31dC2AAD0462C5862B5422C736B8");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("59314674BAbb1a24Eb2704468a9cCdD50668a1C6");
    pub const REFUND_REQUEST: Address = address!("35fb2EFEfAc3Ee9f6E52A9AAE5C9655bC08dEc00");
    pub const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
}

/// Contract addresses for Ethereum Sepolia testnet (eip155:11155111)
pub mod ethereum_sepolia {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("1c7D4B196Cb0C7B01d743Fbc6116a902379C7238");
}

/// Contract addresses for Ethereum mainnet (eip155:1)
pub mod ethereum_mainnet {
    use super::*;

    pub const ESCROW: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const FACTORY: Address = address!("ed02d3E5167BCc9582D851885A89b050AB816a56");
    pub const TOKEN_COLLECTOR: Address = address!("E78648e7af7B1BaDE717FF6E410B922F92adE80f");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("b33D6502EdBbC47201cd1E53C49d703EC0a660b8");
    pub const REFUND_REQUEST: Address = address!("c9BbA6A2CF9838e7Dd8c19BC8B3BAC620B9D8178");
    pub const USDC: Address = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
}

/// Contract addresses for Polygon PoS (eip155:137)
pub mod polygon {
    use super::*;

    pub const ESCROW: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const FACTORY: Address = address!("b33D6502EdBbC47201cd1E53C49d703EC0a660b8");
    pub const TOKEN_COLLECTOR: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("E78648e7af7B1BaDE717FF6E410B922F92adE80f");
    pub const REFUND_REQUEST: Address = address!("ed02d3E5167BCc9582D851885A89b050AB816a56");
    pub const USDC: Address = address!("3c499c542cEF5E3811e1192ce70d8cC03d5c3359");
}

/// Contract addresses for Arbitrum One (eip155:42161)
pub mod arbitrum {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
}

/// Contract addresses for Celo (eip155:42220)
pub mod celo {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("cebA9300f2b948710d2653dD7B07f33A8B32118C");
}

/// Contract addresses for Monad (eip155:143)
pub mod monad {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("754704Bc059F8C67012fEd69BC8A327a5aafb603");
}

/// Contract addresses for Avalanche C-Chain (eip155:43114)
pub mod avalanche {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("B97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E");
}

/// Contract addresses for Optimism (eip155:10)
pub mod optimism {
    use super::*;

    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");
    pub const FACTORY: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");
    pub const TOKEN_COLLECTOR: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");
    pub const PROTOCOL_FEE_CONFIG: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");
    pub const USDC: Address = address!("0b2C639c533813f4Aa9D7837CAf62653d097Ff85");
}

/// All networks that have x402r escrow contracts deployed.
/// This is the single source of truth for escrow network support.
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
];

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
        _ => None,
    }
}

/// Check if a network supports the escrow scheme
pub fn is_supported(network: Network) -> bool {
    escrow_for_network(network).is_some()
}

/// All escrow contract addresses for a network
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
    /// NOTE: payment_operators is empty until a PaymentOperator is deployed
    /// on each network using the factory. Use deploy_operator.py to deploy.
    /// Multiple operators per network are supported (e.g. Fase 3 multi-operator).
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            Network::BaseSepolia => Some(Self {
                escrow: base_sepolia::ESCROW,
                factory: base_sepolia::FACTORY,
                payment_operators: vec![
                    address!("7D092ec506B3D43EB87846F9c9739303785D7B2f"), // Permissionless testnet operator (feeRecipient=facilitator, all conditions=zero)
                ],
                token_collector: base_sepolia::TOKEN_COLLECTOR,
                protocol_fee_config: base_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: base_sepolia::REFUND_REQUEST,
            }),
            Network::Base => Some(Self {
                escrow: base_mainnet::ESCROW,
                factory: base_mainnet::FACTORY,
                payment_operators: vec![
                    address!("271f9fa7f8907aCf178CCFB470076D9129D8F0Eb"), // EM Fase 5 trustless fee split (1300bps=13%, Facilitator-only refund)
                    address!("030353642B936c9D4213caD7BcB0fB8a1489cBe5"), // EM Fase 4 secure operator (OR release, Facilitator-only refund, feeCalculator=0)
                ],
                token_collector: base_mainnet::TOKEN_COLLECTOR,
                protocol_fee_config: base_mainnet::PROTOCOL_FEE_CONFIG,
                refund_request: base_mainnet::REFUND_REQUEST,
            }),
            Network::EthereumSepolia => Some(Self {
                escrow: ethereum_sepolia::ESCROW,
                factory: ethereum_sepolia::FACTORY,
                payment_operators: vec![
                    address!("a8d2432C7ab8bA551feC15e09b64F44505e72b36"), // Permissionless testnet operator (feeRecipient=facilitator, all conditions=zero)
                ],
                token_collector: ethereum_sepolia::TOKEN_COLLECTOR,
                protocol_fee_config: ethereum_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: ethereum_sepolia::REFUND_REQUEST,
            }),
            Network::Ethereum => Some(Self {
                escrow: ethereum_mainnet::ESCROW,
                factory: ethereum_mainnet::FACTORY,
                payment_operators: vec![],
                token_collector: ethereum_mainnet::TOKEN_COLLECTOR,
                protocol_fee_config: ethereum_mainnet::PROTOCOL_FEE_CONFIG,
                refund_request: ethereum_mainnet::REFUND_REQUEST,
            }),
            Network::Polygon => Some(Self {
                escrow: polygon::ESCROW,
                factory: polygon::FACTORY,
                payment_operators: vec![
                    address!("B87F1ECC85f074e50df3DD16A1F40e4e1EC4102e"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: polygon::TOKEN_COLLECTOR,
                protocol_fee_config: polygon::PROTOCOL_FEE_CONFIG,
                refund_request: polygon::REFUND_REQUEST,
            }),
            Network::Arbitrum => Some(Self {
                escrow: arbitrum::ESCROW,
                factory: arbitrum::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: arbitrum::TOKEN_COLLECTOR,
                protocol_fee_config: arbitrum::PROTOCOL_FEE_CONFIG,
                refund_request: arbitrum::REFUND_REQUEST,
            }),
            Network::Celo => Some(Self {
                escrow: celo::ESCROW,
                factory: celo::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: celo::TOKEN_COLLECTOR,
                protocol_fee_config: celo::PROTOCOL_FEE_CONFIG,
                refund_request: celo::REFUND_REQUEST,
            }),
            Network::Monad => Some(Self {
                escrow: monad::ESCROW,
                factory: monad::FACTORY,
                payment_operators: vec![
                    address!("9620Dbe2BB549E1d080Dc8e7982623A9e1Df8cC3"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: monad::TOKEN_COLLECTOR,
                protocol_fee_config: monad::PROTOCOL_FEE_CONFIG,
                refund_request: monad::REFUND_REQUEST,
            }),
            Network::Avalanche => Some(Self {
                escrow: avalanche::ESCROW,
                factory: avalanche::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: avalanche::TOKEN_COLLECTOR,
                protocol_fee_config: avalanche::PROTOCOL_FEE_CONFIG,
                refund_request: avalanche::REFUND_REQUEST,
            }),
            Network::Optimism => Some(Self {
                escrow: optimism::ESCROW,
                factory: optimism::FACTORY,
                payment_operators: vec![
                    address!("C2377a9Db1de2520BD6b2756eD012f4E82F7938e"), // EM Fase 5 (1300bps, OR release, Facilitator-only refund)
                ],
                token_collector: optimism::TOKEN_COLLECTOR,
                protocol_fee_config: optimism::PROTOCOL_FEE_CONFIG,
                refund_request: optimism::REFUND_REQUEST,
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
        // All escrow networks should be supported
        for &network in ESCROW_NETWORKS {
            assert!(is_supported(network), "{:?} should be supported", network);
        }
        // Non-escrow networks should not be supported
        assert!(!is_supported(Network::Solana));
        assert!(!is_supported(Network::HyperEvm));
    }

    #[test]
    fn test_escrow_networks_count() {
        assert_eq!(ESCROW_NETWORKS.len(), 10);
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
    fn test_base_mainnet_has_payment_operators() {
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert!(
            !addrs.payment_operators.is_empty(),
            "Base mainnet should have EM PaymentOperator(s) registered"
        );
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

    #[test]
    fn test_base_mainnet_addresses() {
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert_eq!(addrs.escrow, base_mainnet::ESCROW);
        assert_eq!(addrs.factory, base_mainnet::FACTORY);
        assert_eq!(addrs.token_collector, base_mainnet::TOKEN_COLLECTOR);
    }

    #[test]
    fn test_ethereum_mainnet_addresses() {
        let addrs = OperatorAddresses::for_network(Network::Ethereum).unwrap();
        assert_eq!(addrs.escrow, ethereum_mainnet::ESCROW);
        assert_eq!(addrs.factory, ethereum_mainnet::FACTORY);
        assert_eq!(addrs.token_collector, ethereum_mainnet::TOKEN_COLLECTOR);
    }
}
