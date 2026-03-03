//! ERC-8004 Trustless Agents Integration Module
//!
//! This module implements support for the ERC-8004 "Trustless Agents" standard,
//! enabling AI agent identity, reputation, and validation across multiple networks.
//!
//! # Overview
//!
//! ERC-8004 defines three on-chain registries:
//! - **Identity Registry**: ERC-721 based agent handles with metadata URIs
//! - **Reputation Registry**: Standardized feedback posting and aggregation
//! - **Validation Registry**: Hooks for independent validator checks
//!
//! # Supported Networks
//!
//! ## EVM Mainnets
//! - Ethereum Mainnet
//! - Base Mainnet
//! - Polygon Mainnet
//! - Arbitrum One
//! - Optimism Mainnet
//! - Celo Mainnet
//! - BSC (BNB Smart Chain)
//! - Monad Mainnet
//! - Avalanche C-Chain
//!
//! ## EVM Testnets
//! - Ethereum Sepolia
//! - Base Sepolia
//! - Polygon Amoy
//! - Arbitrum Sepolia
//! - Optimism Sepolia
//! - Celo Sepolia
//! - Avalanche Fuji
//!
//! ## Solana (QuantuLabs 8004-solana + ATOM Engine)
//! - Solana Mainnet
//! - Solana Devnet
//!
//! # x402 Integration
//!
//! The `8004-reputation` extension enables:
//! 1. **ProofOfPayment**: Settlement responses include cryptographic proof
//! 2. **Feedback Endpoint**: POST /feedback to submit reputation on-chain
//! 3. **Reputation Query**: GET /reputation/:agentId to read reputation
//! 4. **Identity Query**: GET /identity/:agentId to read agent info
//!
//! # Reference
//!
//! - ERC-8004 Specification: <https://eips.ethereum.org/EIPS/eip-8004>
//! - Official Contracts: <https://github.com/erc-8004/erc-8004-contracts>
//! - x402 Extension: `8004-reputation`

mod abi;
pub mod solana;
mod types;

pub use abi::*;
pub use types::*;

use crate::network::Network;
use alloy::primitives::Address;

/// x402 extension identifier for ERC-8004 reputation
pub const EXTENSION_ID: &str = "8004-reputation";

// ============================================================================
// Contract Addresses by Network
// ============================================================================

/// ERC-8004 contract addresses for a specific network
#[derive(Debug, Clone, Copy)]
pub struct Erc8004Contracts {
    pub identity_registry: Address,
    pub reputation_registry: Address,
    pub validation_registry: Option<Address>,
}

// Ethereum Mainnet - Official deployment (January 29, 2026)
pub const ETHEREUM_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None, // Not yet deployed
};

// Ethereum Sepolia - Official testnet deployment
pub const ETHEREUM_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// ============================================================================
// Mainnet Contracts - All use CREATE2 deterministic addresses
// ============================================================================

// Base Mainnet - Official deployment (February 4, 2026)
pub const BASE_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Polygon Mainnet - Official deployment (February 2026)
pub const POLYGON_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Arbitrum One Mainnet - Official deployment (February 2026)
pub const ARBITRUM_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Optimism Mainnet - Official deployment (February 9, 2026)
pub const OPTIMISM_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Celo Mainnet - Official deployment (February 2026)
pub const CELO_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// BSC (BNB Smart Chain) Mainnet - Official deployment (February 2026)
pub const BSC_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Monad Mainnet - Official deployment (February 2026)
pub const MONAD_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// Avalanche C-Chain Mainnet - Official deployment (February 2026)
pub const AVALANCHE_MAINNET_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"),
    reputation_registry: alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"),
    validation_registry: None,
};

// ============================================================================
// Testnet Contracts - All use same testnet addresses
// ============================================================================

// Base Sepolia - Official testnet deployment
pub const BASE_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// Polygon Amoy Testnet - Official testnet deployment
pub const POLYGON_AMOY_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// Arbitrum Sepolia Testnet - Official testnet deployment
pub const ARBITRUM_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// Optimism Sepolia Testnet - Official testnet deployment (February 9, 2026)
pub const OPTIMISM_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// Celo Sepolia Testnet - Official testnet deployment
pub const CELO_SEPOLIA_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

// Avalanche Fuji Testnet - Official testnet deployment
pub const AVALANCHE_FUJI_CONTRACTS: Erc8004Contracts = Erc8004Contracts {
    identity_registry: alloy::primitives::address!("8004A818BFB912233c491871b3d84c89A494BD9e"),
    reputation_registry: alloy::primitives::address!("8004B663056A597Dffe9eCcC1965A193B7388713"),
    validation_registry: Some(alloy::primitives::address!(
        "8004Cb1BF31DAf7788923b405b754f57acEB4272"
    )),
};

/// Get ERC-8004 contract addresses for a network
pub fn get_contracts(network: &Network) -> Option<Erc8004Contracts> {
    match network {
        // Mainnets
        Network::Ethereum => Some(ETHEREUM_MAINNET_CONTRACTS),
        Network::Base => Some(BASE_MAINNET_CONTRACTS),
        Network::Polygon => Some(POLYGON_MAINNET_CONTRACTS),
        Network::Arbitrum => Some(ARBITRUM_MAINNET_CONTRACTS),
        Network::Optimism => Some(OPTIMISM_MAINNET_CONTRACTS),
        Network::Celo => Some(CELO_MAINNET_CONTRACTS),
        Network::Bsc => Some(BSC_MAINNET_CONTRACTS),
        Network::Monad => Some(MONAD_MAINNET_CONTRACTS),
        Network::Avalanche => Some(AVALANCHE_MAINNET_CONTRACTS),
        // Testnets
        Network::EthereumSepolia => Some(ETHEREUM_SEPOLIA_CONTRACTS),
        Network::BaseSepolia => Some(BASE_SEPOLIA_CONTRACTS),
        Network::PolygonAmoy => Some(POLYGON_AMOY_CONTRACTS),
        Network::ArbitrumSepolia => Some(ARBITRUM_SEPOLIA_CONTRACTS),
        Network::OptimismSepolia => Some(OPTIMISM_SEPOLIA_CONTRACTS),
        Network::CeloSepolia => Some(CELO_SEPOLIA_CONTRACTS),
        Network::AvalancheFuji => Some(AVALANCHE_FUJI_CONTRACTS),
        _ => None,
    }
}

/// Check if ERC-8004 is supported on a network (EVM or Solana)
pub fn is_erc8004_supported(network: &Network) -> bool {
    get_contracts(network).is_some() || solana::is_solana_erc8004_supported(network)
}

/// Get list of all networks with ERC-8004 support
pub fn supported_networks() -> Vec<Network> {
    vec![
        // EVM Mainnets
        Network::Ethereum,
        Network::Base,
        Network::Polygon,
        Network::Arbitrum,
        Network::Optimism,
        Network::Celo,
        Network::Bsc,
        Network::Monad,
        Network::Avalanche,
        // EVM Testnets
        Network::EthereumSepolia,
        Network::BaseSepolia,
        Network::PolygonAmoy,
        Network::ArbitrumSepolia,
        Network::OptimismSepolia,
        Network::CeloSepolia,
        Network::AvalancheFuji,
        // Solana (QuantuLabs 8004-solana + ATOM Engine)
        Network::Solana,
        Network::SolanaDevnet,
    ]
}

/// Get list of supported network names for API responses.
/// Derived from `supported_networks()` to avoid name mismatches.
pub fn supported_network_names() -> Vec<String> {
    supported_networks().iter().map(|n| n.to_string()).collect()
}

// ============================================================================
// Legacy compatibility - Global config (deprecated)
// ============================================================================

use once_cell::sync::Lazy;
use std::str::FromStr;

/// Placeholder address used when contracts are not configured
const PLACEHOLDER_ADDRESS: Address =
    alloy::primitives::address!("0000000000000000000000000000000000000000");

/// Legacy global configuration (deprecated - use get_contracts() instead)
pub struct Erc8004Config {
    pub identity_registry: Address,
    pub reputation_registry: Address,
    pub validation_registry: Address,
    pub is_configured: bool,
}

impl Erc8004Config {
    fn from_env() -> Self {
        // Try environment variables first (for custom deployments)
        let identity_registry = std::env::var("ERC8004_IDENTITY_REGISTRY")
            .ok()
            .and_then(|s| Address::from_str(&s).ok())
            .unwrap_or(ETHEREUM_MAINNET_CONTRACTS.identity_registry);

        let reputation_registry = std::env::var("ERC8004_REPUTATION_REGISTRY")
            .ok()
            .and_then(|s| Address::from_str(&s).ok())
            .unwrap_or(ETHEREUM_MAINNET_CONTRACTS.reputation_registry);

        let validation_registry = std::env::var("ERC8004_VALIDATION_REGISTRY")
            .ok()
            .and_then(|s| Address::from_str(&s).ok())
            .unwrap_or(PLACEHOLDER_ADDRESS);

        let is_configured = reputation_registry != PLACEHOLDER_ADDRESS;

        if is_configured {
            tracing::info!(
                identity = %identity_registry,
                reputation = %reputation_registry,
                validation = %validation_registry,
                "ERC-8004 contracts configured"
            );
        }

        Self {
            identity_registry,
            reputation_registry,
            validation_registry,
            is_configured,
        }
    }
}

/// Global configuration (legacy - use get_contracts() for multi-network)
pub static ERC8004_CONFIG: Lazy<Erc8004Config> = Lazy::new(Erc8004Config::from_env);

/// Get the Identity Registry address for legacy code
#[deprecated(note = "Use get_contracts(network) instead")]
pub fn identity_registry_address() -> Address {
    ERC8004_CONFIG.identity_registry
}

/// Get the Reputation Registry address for legacy code
pub fn reputation_registry_address() -> Address {
    ERC8004_CONFIG.reputation_registry
}

/// Get the Validation Registry address for legacy code
#[deprecated(note = "Use get_contracts(network) instead")]
pub fn validation_registry_address() -> Address {
    ERC8004_CONFIG.validation_registry
}

/// Check if ERC-8004 is configured (legacy)
pub fn is_configured() -> bool {
    ERC8004_CONFIG.is_configured
}

// Keep old constants for backward compatibility
pub const IDENTITY_REGISTRY_ADDRESS: Address =
    alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432");
pub const REPUTATION_REGISTRY_ADDRESS: Address =
    alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63");
pub const VALIDATION_REGISTRY_ADDRESS: Address =
    alloy::primitives::address!("0000000000000000000000000000000000000000");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ethereum_mainnet_supported() {
        assert!(is_erc8004_supported(&Network::Ethereum));
        let contracts = get_contracts(&Network::Ethereum).unwrap();
        assert_eq!(
            contracts.identity_registry,
            ETHEREUM_MAINNET_CONTRACTS.identity_registry
        );
    }

    #[test]
    fn test_ethereum_sepolia_supported() {
        assert!(is_erc8004_supported(&Network::EthereumSepolia));
        let contracts = get_contracts(&Network::EthereumSepolia).unwrap();
        assert!(contracts.validation_registry.is_some());
    }

    #[test]
    fn test_base_mainnet_supported() {
        assert!(is_erc8004_supported(&Network::Base));
        let contracts = get_contracts(&Network::Base).unwrap();
        // Same addresses as Ethereum (CREATE2 deterministic deployment)
        assert_eq!(
            contracts.identity_registry,
            ETHEREUM_MAINNET_CONTRACTS.identity_registry
        );
        assert_eq!(
            contracts.reputation_registry,
            ETHEREUM_MAINNET_CONTRACTS.reputation_registry
        );
    }

    #[test]
    fn test_all_mainnets_use_deterministic_addresses() {
        let mainnet_networks = vec![
            Network::Ethereum,
            Network::Base,
            Network::Polygon,
            Network::Arbitrum,
            Network::Optimism,
            Network::Celo,
            Network::Bsc,
            Network::Monad,
            Network::Avalanche,
        ];

        for network in mainnet_networks {
            assert!(
                is_erc8004_supported(&network),
                "Network {:?} should be supported",
                network
            );
            let contracts = get_contracts(&network).unwrap();
            assert_eq!(
                contracts.identity_registry, ETHEREUM_MAINNET_CONTRACTS.identity_registry,
                "Network {:?} should use deterministic identity address",
                network
            );
            assert_eq!(
                contracts.reputation_registry, ETHEREUM_MAINNET_CONTRACTS.reputation_registry,
                "Network {:?} should use deterministic reputation address",
                network
            );
        }
    }

    #[test]
    fn test_all_testnets_use_testnet_addresses() {
        let testnet_networks = vec![
            Network::EthereumSepolia,
            Network::BaseSepolia,
            Network::PolygonAmoy,
            Network::ArbitrumSepolia,
            Network::OptimismSepolia,
            Network::CeloSepolia,
            Network::AvalancheFuji,
        ];

        for network in testnet_networks {
            assert!(
                is_erc8004_supported(&network),
                "Network {:?} should be supported",
                network
            );
            let contracts = get_contracts(&network).unwrap();
            assert_eq!(
                contracts.identity_registry, ETHEREUM_SEPOLIA_CONTRACTS.identity_registry,
                "Network {:?} should use testnet identity address",
                network
            );
            assert!(
                contracts.validation_registry.is_some(),
                "Network {:?} should have validation registry",
                network
            );
        }
    }

    #[test]
    fn test_unsupported_network() {
        assert!(!is_erc8004_supported(&Network::HyperEvm));
        assert!(get_contracts(&Network::HyperEvm).is_none());
    }

    #[test]
    fn test_supported_networks_list() {
        let networks = supported_networks();
        // EVM Mainnets
        assert!(networks.contains(&Network::Ethereum));
        assert!(networks.contains(&Network::Base));
        assert!(networks.contains(&Network::Polygon));
        assert!(networks.contains(&Network::Arbitrum));
        assert!(networks.contains(&Network::Optimism));
        assert!(networks.contains(&Network::Celo));
        assert!(networks.contains(&Network::Bsc));
        assert!(networks.contains(&Network::Monad));
        assert!(networks.contains(&Network::Avalanche));
        // EVM Testnets
        assert!(networks.contains(&Network::EthereumSepolia));
        assert!(networks.contains(&Network::BaseSepolia));
        assert!(networks.contains(&Network::PolygonAmoy));
        assert!(networks.contains(&Network::ArbitrumSepolia));
        assert!(networks.contains(&Network::OptimismSepolia));
        assert!(networks.contains(&Network::CeloSepolia));
        assert!(networks.contains(&Network::AvalancheFuji));
        // Solana
        assert!(networks.contains(&Network::Solana));
        assert!(networks.contains(&Network::SolanaDevnet));
        // Total count: 16 EVM + 2 Solana = 18
        assert_eq!(networks.len(), 18);
    }

    #[test]
    fn test_supported_network_names() {
        let names = supported_network_names();
        // EVM names must match Network::Display (what serde/FromStr accept)
        assert!(names.contains(&"ethereum".to_string()));
        assert!(names.contains(&"base".to_string())); // NOT "base-mainnet"
        assert!(names.contains(&"polygon".to_string()));
        assert!(names.contains(&"arbitrum".to_string()));
        assert!(names.contains(&"optimism".to_string()));
        assert!(names.contains(&"celo".to_string()));
        assert!(names.contains(&"bsc".to_string()));
        assert!(names.contains(&"monad".to_string()));
        assert!(names.contains(&"avalanche".to_string()));
        assert!(names.contains(&"ethereum-sepolia".to_string()));
        assert!(names.contains(&"base-sepolia".to_string()));
        assert!(names.contains(&"polygon-amoy".to_string()));
        assert!(names.contains(&"arbitrum-sepolia".to_string()));
        assert!(names.contains(&"optimism-sepolia".to_string()));
        assert!(names.contains(&"celo-sepolia".to_string()));
        assert!(names.contains(&"avalanche-fuji".to_string()));
        // Solana names
        assert!(names.contains(&"solana".to_string()));
        assert!(names.contains(&"solana-devnet".to_string()));
        assert_eq!(names.len(), 18);
    }

    #[test]
    fn test_solana_erc8004_supported() {
        assert!(is_erc8004_supported(&Network::Solana));
        assert!(is_erc8004_supported(&Network::SolanaDevnet));
    }
}
