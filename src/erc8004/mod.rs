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
//! - Ethereum Mainnet (production)
//! - Ethereum Sepolia (testnet)
//! - Base Mainnet (when contracts are deployed)
//! - Base Sepolia (when contracts are deployed)
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
mod types;

pub use abi::*;
pub use types::*;

use alloy::primitives::Address;
use crate::network::Network;

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
    validation_registry: Some(alloy::primitives::address!("8004Cb1BF31DAf7788923b405b754f57acEB4272")),
};

// Base Mainnet - Pending deployment
// Using placeholder until official deployment
pub const BASE_MAINNET_CONTRACTS: Option<Erc8004Contracts> = None;

// Base Sepolia - Pending official deployment
// Reference implementation exists but not canonical addresses
pub const BASE_SEPOLIA_CONTRACTS: Option<Erc8004Contracts> = None;

/// Get ERC-8004 contract addresses for a network
pub fn get_contracts(network: &Network) -> Option<Erc8004Contracts> {
    match network {
        Network::Ethereum => Some(ETHEREUM_MAINNET_CONTRACTS),
        Network::EthereumSepolia => Some(ETHEREUM_SEPOLIA_CONTRACTS),
        Network::Base => BASE_MAINNET_CONTRACTS,
        Network::BaseSepolia => BASE_SEPOLIA_CONTRACTS,
        _ => None,
    }
}

/// Check if ERC-8004 is supported on a network
pub fn is_erc8004_supported(network: &Network) -> bool {
    get_contracts(network).is_some()
}

/// Get list of all networks with ERC-8004 support
pub fn supported_networks() -> Vec<Network> {
    vec![
        Network::Ethereum,
        Network::EthereumSepolia,
        // Add more networks here as contracts are deployed
    ]
}

/// Get list of supported network names for API responses
pub fn supported_network_names() -> Vec<&'static str> {
    vec![
        "ethereum",
        "ethereum-sepolia",
        // Add more as deployed
    ]
}

// ============================================================================
// Legacy compatibility - Global config (deprecated)
// ============================================================================

use once_cell::sync::Lazy;
use std::str::FromStr;

/// Placeholder address used when contracts are not configured
const PLACEHOLDER_ADDRESS: Address = alloy::primitives::address!("0000000000000000000000000000000000000000");

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
pub const IDENTITY_REGISTRY_ADDRESS: Address = alloy::primitives::address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432");
pub const REPUTATION_REGISTRY_ADDRESS: Address = alloy::primitives::address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63");
pub const VALIDATION_REGISTRY_ADDRESS: Address = alloy::primitives::address!("0000000000000000000000000000000000000000");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ethereum_mainnet_supported() {
        assert!(is_erc8004_supported(&Network::Ethereum));
        let contracts = get_contracts(&Network::Ethereum).unwrap();
        assert_eq!(contracts.identity_registry, ETHEREUM_MAINNET_CONTRACTS.identity_registry);
    }

    #[test]
    fn test_ethereum_sepolia_supported() {
        assert!(is_erc8004_supported(&Network::EthereumSepolia));
        let contracts = get_contracts(&Network::EthereumSepolia).unwrap();
        assert!(contracts.validation_registry.is_some());
    }

    #[test]
    fn test_unsupported_network() {
        assert!(!is_erc8004_supported(&Network::Avalanche));
        assert!(get_contracts(&Network::Avalanche).is_none());
    }

    #[test]
    fn test_supported_networks_list() {
        let networks = supported_networks();
        assert!(networks.contains(&Network::Ethereum));
        assert!(networks.contains(&Network::EthereumSepolia));
    }
}
