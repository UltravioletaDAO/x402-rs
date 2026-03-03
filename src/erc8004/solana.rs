//! Solana-specific ERC-8004 (Trustless Agents) integration.
//!
//! This module provides read-only support for the QuantuLabs 8004-solana Anchor program,
//! which implements ERC-8004 on Solana using Metaplex Core NFTs and the ATOM Engine
//! for on-chain reputation scoring.
//!
//! # Architecture Differences from EVM
//!
//! - **Agent IDs**: Base58 Pubkeys (NFT mint addresses) instead of sequential uint256
//! - **Storage**: Event-based feedback with SEAL v1 hash-chain integrity
//! - **Reputation**: ATOM Engine CPI program with HyperLogLog, trust tiers, EMA scoring
//! - **Account model**: PDAs derived from `["agent", asset_pubkey]` seeds
//!
//! # References
//!
//! - [8004-solana](https://github.com/QuantuLabs/8004-solana)
//! - [8004-atom](https://github.com/QuantuLabs/8004-atom)
//! - [Solana Agent Registry](https://solana.com/agent-registry)

use borsh::BorshDeserialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::network::Network;

// ============================================================================
// Program IDs
// ============================================================================

/// Agent Registry program ID (mainnet-beta)
pub const AGENT_REGISTRY_MAINNET: Pubkey =
    solana_sdk::pubkey!("8oo4dC4JvBLwy5tGgiH3WwK4B9PWxL9Z4XjA2jzkQMbQ");

/// Agent Registry program ID (devnet)
pub const AGENT_REGISTRY_DEVNET: Pubkey =
    solana_sdk::pubkey!("8oo4J9tBB3Hna1jRQ3rWvJjojqM5DYTDJo5cejUuJy3C");

/// ATOM Engine program ID (mainnet-beta)
pub const ATOM_ENGINE_MAINNET: Pubkey =
    solana_sdk::pubkey!("AToMw53aiPQ8j7iHVb4fGt6nzUNxUhcPc3tbPBZuzVVb");

/// ATOM Engine program ID (devnet)
pub const ATOM_ENGINE_DEVNET: Pubkey =
    solana_sdk::pubkey!("AToMufS4QD6hEXvcvBDg9m1AHeCLpmZQsyfYa5h9MwAF");

/// Program IDs for a specific Solana network
#[derive(Debug, Clone, Copy)]
pub struct SolanaErc8004Programs {
    pub agent_registry: Pubkey,
    pub atom_engine: Pubkey,
}

/// Get program IDs for a Solana network
pub fn get_program_ids(network: &Network) -> Option<SolanaErc8004Programs> {
    match network {
        Network::Solana => Some(SolanaErc8004Programs {
            agent_registry: AGENT_REGISTRY_MAINNET,
            atom_engine: ATOM_ENGINE_MAINNET,
        }),
        Network::SolanaDevnet => Some(SolanaErc8004Programs {
            agent_registry: AGENT_REGISTRY_DEVNET,
            atom_engine: ATOM_ENGINE_DEVNET,
        }),
        _ => None,
    }
}

// ============================================================================
// Anchor Account Discriminators
// ============================================================================

// Anchor uses the first 8 bytes of SHA256("account:<StructName>") as discriminator.
// These are pre-computed for the known account types.

/// Discriminator for AgentAccount: SHA256("account:AgentAccount")[..8]
const AGENT_ACCOUNT_DISCRIMINATOR: [u8; 8] = [241, 119, 69, 140, 233, 9, 112, 50];

/// Discriminator for AtomStats: SHA256("account:AtomStats")[..8]
const ATOM_STATS_DISCRIMINATOR: [u8; 8] = [190, 187, 50, 59, 203, 39, 136, 244];

/// Discriminator for RegistryConfig: SHA256("account:RegistryConfig")[..8]
const REGISTRY_CONFIG_DISCRIMINATOR: [u8; 8] = [23, 118, 10, 246, 173, 231, 243, 156];

// ============================================================================
// Borsh-Deserialized Account Structures
// ============================================================================

/// AgentAccount (313 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["agent", asset.key()]`
///
/// The primary identity record for a Solana-registered agent.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct AgentAccount {
    /// NFT owner address
    pub owner: [u8; 32],
    /// Metaplex Core NFT mint address (unique agent identifier)
    pub asset: [u8; 32],
    /// PDA bump seed
    pub bump: u8,
    /// URI to agent registration file (IPFS/HTTPS)
    pub agent_uri: String,
    /// Human-readable agent name
    pub nft_name: String,
    /// Rolling hash chain for feedback integrity (SEAL v1)
    pub feedback_digest: [u8; 32],
    /// Total feedback received
    pub feedback_count: u64,
    /// Rolling hash chain for responses (SEAL v1)
    pub response_digest: [u8; 32],
    /// Total responses appended
    pub response_count: u64,
    /// Rolling hash chain for revocations (SEAL v1)
    pub revoke_digest: [u8; 32],
    /// Total feedback revocations
    pub revoke_count: u64,
}

/// AtomStats (460 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["atom_stats", asset.key()]`
///
/// ATOM Engine reputation analytics computed on-chain via CPI.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct AtomStats {
    /// Registry collection address
    pub collection: [u8; 32],
    /// Agent NFT mint address
    pub asset: [u8; 32],
    /// Total feedback count
    pub feedback_count: u32,
    /// Positive feedback count
    pub positive_count: u32,
    /// Negative feedback count
    pub negative_count: u32,
    /// EMA quality score (centered at 0, positive = above-average)
    pub quality_score: i32,
    /// Slot of most recent feedback
    pub last_feedback_slot: u64,
    /// HyperLogLog registers (256 x 4-bit packed, for unique client estimation)
    pub hll_packed: [u8; 128],
    /// Per-agent salt for HLL grinding prevention
    pub hll_salt: u64,
    /// Ring buffer for burst detection (24 x 56-bit fingerprints)
    pub recent_callers: [u64; 24],
    /// Ring buffer cursor
    pub eviction_cursor: u8,
    /// Trust tier (0-4): Unknown, New, Established, Trusted, Legendary
    pub trust_tier: u8,
    /// Statistical confidence (0-100)
    pub confidence: u8,
    /// Risk assessment (0-100)
    pub risk_score: u8,
    /// Client diversity measure from HyperLogLog (0-100)
    pub diversity_ratio: u8,
    /// PDA bump seed
    pub bump: u8,
}

/// RegistryConfig (78 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["config"]`
///
/// Global registry configuration.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct RegistryConfig {
    /// Metaplex Core collection address
    pub collection: [u8; 32],
    /// Registry type identifier
    pub registry_type: u8,
    /// Upgrade authority
    pub authority: [u8; 32],
    /// Total registered agents (sequential counter)
    pub base_index: u32,
    /// PDA bump seed
    pub bump: u8,
}

// ============================================================================
// PDA Derivation
// ============================================================================

/// Derive the AgentAccount PDA for a given asset (NFT mint) pubkey.
///
/// Seeds: `["agent", asset.key()]`
pub fn derive_agent_pda(asset: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"agent", asset.as_ref()], program_id)
}

/// Derive the AtomStats PDA for a given asset (NFT mint) pubkey.
///
/// Seeds: `["atom_stats", asset.key()]`
pub fn derive_atom_stats_pda(asset: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"atom_stats", asset.as_ref()], program_id)
}

/// Derive the RegistryConfig PDA.
///
/// Seeds: `["config"]`
pub fn derive_registry_config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"config"], program_id)
}

/// Derive the MetadataEntryPda for a given asset and metadata key.
///
/// Seeds: `["agent_meta", asset.key(), sha256(key)[0..8]]`
pub fn derive_metadata_pda(
    asset: &Pubkey,
    metadata_key: &str,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    use sha2::{Digest, Sha256};
    let key_hash = Sha256::digest(metadata_key.as_bytes());
    Pubkey::find_program_address(
        &[b"agent_meta", asset.as_ref(), &key_hash[..8]],
        program_id,
    )
}

// ============================================================================
// RPC Read Helpers
// ============================================================================

/// Error type for Solana ERC-8004 read operations
#[derive(Debug, thiserror::Error)]
pub enum SolanaErc8004Error {
    #[error("Account not found: {0}")]
    AccountNotFound(String),

    #[error("Invalid account data: {0}")]
    InvalidAccountData(String),

    #[error("Invalid agent ID (expected base58 pubkey): {0}")]
    InvalidAgentId(String),

    #[error("Network not supported for Solana ERC-8004: {0}")]
    UnsupportedNetwork(String),

    #[error("RPC error: {0}")]
    RpcError(String),
}

/// Parse a base58 agent ID string into a Pubkey
pub fn parse_agent_id(agent_id: &str) -> Result<Pubkey, SolanaErc8004Error> {
    Pubkey::from_str(agent_id)
        .map_err(|e| SolanaErc8004Error::InvalidAgentId(format!("{}: {}", agent_id, e)))
}

/// Read and deserialize an AgentAccount from the chain.
pub async fn read_agent_account(
    rpc_client: &RpcClient,
    asset_pubkey: &Pubkey,
    program_id: &Pubkey,
) -> Result<AgentAccount, SolanaErc8004Error> {
    let (pda, _bump) = derive_agent_pda(asset_pubkey, program_id);

    let account_data = rpc_client
        .get_account_data(&pda)
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("AccountNotFound") || err_str.contains("could not find account") {
                SolanaErc8004Error::AccountNotFound(format!(
                    "Agent {} not found (PDA: {})",
                    asset_pubkey, pda
                ))
            } else {
                SolanaErc8004Error::RpcError(err_str)
            }
        })?;

    // Verify Anchor discriminator (first 8 bytes)
    if account_data.len() < 8 {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Account data too short for Anchor discriminator".to_string(),
        ));
    }

    if account_data[..8] != AGENT_ACCOUNT_DISCRIMINATOR {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Invalid Anchor discriminator for AgentAccount".to_string(),
        ));
    }

    // Deserialize from bytes after the 8-byte discriminator
    AgentAccount::try_from_slice(&account_data[8..]).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!("Failed to deserialize AgentAccount: {}", e))
    })
}

/// Read and deserialize AtomStats from the chain.
pub async fn read_atom_stats(
    rpc_client: &RpcClient,
    asset_pubkey: &Pubkey,
    atom_program_id: &Pubkey,
) -> Result<AtomStats, SolanaErc8004Error> {
    let (pda, _bump) = derive_atom_stats_pda(asset_pubkey, atom_program_id);

    let account_data = rpc_client
        .get_account_data(&pda)
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("AccountNotFound") || err_str.contains("could not find account") {
                SolanaErc8004Error::AccountNotFound(format!(
                    "ATOM stats not found for agent {} (PDA: {})",
                    asset_pubkey, pda
                ))
            } else {
                SolanaErc8004Error::RpcError(err_str)
            }
        })?;

    // Verify Anchor discriminator
    if account_data.len() < 8 {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Account data too short for Anchor discriminator".to_string(),
        ));
    }

    if account_data[..8] != ATOM_STATS_DISCRIMINATOR {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Invalid Anchor discriminator for AtomStats".to_string(),
        ));
    }

    AtomStats::try_from_slice(&account_data[8..]).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!("Failed to deserialize AtomStats: {}", e))
    })
}

/// Read and deserialize RegistryConfig from the chain.
pub async fn read_registry_config(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
) -> Result<RegistryConfig, SolanaErc8004Error> {
    let (pda, _bump) = derive_registry_config_pda(program_id);

    let account_data = rpc_client
        .get_account_data(&pda)
        .await
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("AccountNotFound") || err_str.contains("could not find account") {
                SolanaErc8004Error::AccountNotFound(format!(
                    "Registry config not found (PDA: {})",
                    pda
                ))
            } else {
                SolanaErc8004Error::RpcError(err_str)
            }
        })?;

    // Verify Anchor discriminator
    if account_data.len() < 8 {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Account data too short for Anchor discriminator".to_string(),
        ));
    }

    if account_data[..8] != REGISTRY_CONFIG_DISCRIMINATOR {
        return Err(SolanaErc8004Error::InvalidAccountData(
            "Invalid Anchor discriminator for RegistryConfig".to_string(),
        ));
    }

    RegistryConfig::try_from_slice(&account_data[8..]).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!(
            "Failed to deserialize RegistryConfig: {}",
            e
        ))
    })
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert a trust tier value (0-4) to its human-readable name.
pub fn trust_tier_name(tier: u8) -> &'static str {
    match tier {
        0 => "Unknown",
        1 => "New",
        2 => "Established",
        3 => "Trusted",
        4 => "Legendary",
        _ => "Unknown",
    }
}

/// Convert a 32-byte array to a Solana Pubkey
pub fn bytes_to_pubkey(bytes: &[u8; 32]) -> Pubkey {
    Pubkey::from(*bytes)
}

/// Check if a Solana network supports ERC-8004
pub fn is_solana_erc8004_supported(network: &Network) -> bool {
    matches!(network, Network::Solana | Network::SolanaDevnet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_program_ids() {
        let mainnet = get_program_ids(&Network::Solana).unwrap();
        assert_eq!(mainnet.agent_registry, AGENT_REGISTRY_MAINNET);
        assert_eq!(mainnet.atom_engine, ATOM_ENGINE_MAINNET);

        let devnet = get_program_ids(&Network::SolanaDevnet).unwrap();
        assert_eq!(devnet.agent_registry, AGENT_REGISTRY_DEVNET);
        assert_eq!(devnet.atom_engine, ATOM_ENGINE_DEVNET);

        assert!(get_program_ids(&Network::Ethereum).is_none());
    }

    #[test]
    fn test_pda_derivation() {
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();

        // Agent PDA
        let (agent_pda, bump) = derive_agent_pda(&asset, &AGENT_REGISTRY_MAINNET);
        assert_ne!(agent_pda, Pubkey::default());
        assert!(bump <= 255);

        // AtomStats PDA
        let (atom_pda, bump) = derive_atom_stats_pda(&asset, &ATOM_ENGINE_MAINNET);
        assert_ne!(atom_pda, Pubkey::default());
        assert!(bump <= 255);

        // PDAs should be different
        assert_ne!(agent_pda, atom_pda);
    }

    #[test]
    fn test_registry_config_pda() {
        let (config_pda, bump) = derive_registry_config_pda(&AGENT_REGISTRY_MAINNET);
        assert_ne!(config_pda, Pubkey::default());
        assert!(bump <= 255);
    }

    #[test]
    fn test_metadata_pda() {
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let (pda1, _) = derive_metadata_pda(&asset, "x402Support", &AGENT_REGISTRY_MAINNET);
        let (pda2, _) = derive_metadata_pda(&asset, "protocol", &AGENT_REGISTRY_MAINNET);

        // Different keys should produce different PDAs
        assert_ne!(pda1, pda2);
    }

    #[test]
    fn test_trust_tier_names() {
        assert_eq!(trust_tier_name(0), "Unknown");
        assert_eq!(trust_tier_name(1), "New");
        assert_eq!(trust_tier_name(2), "Established");
        assert_eq!(trust_tier_name(3), "Trusted");
        assert_eq!(trust_tier_name(4), "Legendary");
        assert_eq!(trust_tier_name(5), "Unknown"); // out of range
    }

    #[test]
    fn test_parse_agent_id() {
        // Valid base58 pubkey
        let result = parse_agent_id("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv");
        assert!(result.is_ok());

        // Invalid base58
        let result = parse_agent_id("not-a-valid-pubkey!!!");
        assert!(result.is_err());

        // Empty string
        let result = parse_agent_id("");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_solana_erc8004_supported() {
        assert!(is_solana_erc8004_supported(&Network::Solana));
        assert!(is_solana_erc8004_supported(&Network::SolanaDevnet));
        assert!(!is_solana_erc8004_supported(&Network::Ethereum));
        assert!(!is_solana_erc8004_supported(&Network::Base));
    }

    #[test]
    fn test_bytes_to_pubkey() {
        let bytes = [0u8; 32];
        let pubkey = bytes_to_pubkey(&bytes);
        assert_eq!(pubkey, Pubkey::default());
    }
}
