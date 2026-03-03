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

// ============================================================================
// Metaplex Core Program
// ============================================================================

/// Metaplex Core program ID (mainnet/devnet share the same program)
pub const METAPLEX_CORE_PROGRAM: Pubkey =
    solana_sdk::pubkey!("CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d");

// ============================================================================
// Anchor Instruction Discriminators (SHA256("global:<fn_name>")[..8])
// ============================================================================

const IX_GIVE_FEEDBACK: [u8; 8] = [145, 136, 123, 3, 215, 165, 98, 41];
const IX_REVOKE_FEEDBACK: [u8; 8] = [211, 37, 230, 82, 118, 216, 137, 206];
const IX_APPEND_RESPONSE: [u8; 8] = [162, 210, 186, 50, 180, 4, 47, 104];
const IX_REGISTER: [u8; 8] = [211, 124, 67, 15, 211, 194, 178, 240];
const IX_SET_AGENT_URI: [u8; 8] = [43, 254, 168, 104, 192, 51, 39, 46];
const IX_SET_METADATA_PDA: [u8; 8] = [236, 60, 23, 48, 138, 69, 196, 153];

// ============================================================================
// SEAL v1 Domain Constants
// ============================================================================

const DOMAIN_SEAL_V1: &[u8] = b"8004_SEAL_V1____";
const DOMAIN_FEEDBACK: &[u8] = b"8004_FEED_V1___";
const DOMAIN_RESPONSE: &[u8] = b"8004_RESP_V1___";
const DOMAIN_REVOKE: &[u8] = b"8004_REVK_V1___";

// ============================================================================
// SEAL v1 Hash Computation
// ============================================================================

/// Compute the SEAL v1 hash for a feedback submission.
///
/// seal_hash = SHA256(DOMAIN_SEAL_V1 || DOMAIN_FEEDBACK || agent_key || client_key
///                    || feedback_count_le || feedback_uri || feedback_hash)
pub fn compute_feedback_seal_hash(
    agent_pubkey: &Pubkey,
    client_pubkey: &Pubkey,
    feedback_count: u64,
    feedback_uri: &str,
    feedback_hash: &[u8; 32],
) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEAL_V1);
    hasher.update(DOMAIN_FEEDBACK);
    hasher.update(agent_pubkey.as_ref());
    hasher.update(client_pubkey.as_ref());
    hasher.update(feedback_count.to_le_bytes());
    hasher.update(feedback_uri.as_bytes());
    hasher.update(feedback_hash);
    hasher.finalize().into()
}

/// Compute the SEAL v1 hash for a revocation.
///
/// seal_hash = SHA256(DOMAIN_SEAL_V1 || DOMAIN_REVOKE || agent_key || client_key
///                    || feedback_index_le || revoke_count_le)
pub fn compute_revoke_seal_hash(
    agent_pubkey: &Pubkey,
    client_pubkey: &Pubkey,
    feedback_index: u64,
    revoke_count: u64,
) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEAL_V1);
    hasher.update(DOMAIN_REVOKE);
    hasher.update(agent_pubkey.as_ref());
    hasher.update(client_pubkey.as_ref());
    hasher.update(feedback_index.to_le_bytes());
    hasher.update(revoke_count.to_le_bytes());
    hasher.finalize().into()
}

/// Compute the SEAL v1 hash for a response.
///
/// seal_hash = SHA256(DOMAIN_SEAL_V1 || DOMAIN_RESPONSE || agent_key || responder_key
///                    || response_count_le || response_uri || response_hash)
pub fn compute_response_seal_hash(
    agent_pubkey: &Pubkey,
    responder_pubkey: &Pubkey,
    response_count: u64,
    response_uri: &str,
    response_hash: &[u8; 32],
) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEAL_V1);
    hasher.update(DOMAIN_RESPONSE);
    hasher.update(agent_pubkey.as_ref());
    hasher.update(responder_pubkey.as_ref());
    hasher.update(response_count.to_le_bytes());
    hasher.update(response_uri.as_bytes());
    hasher.update(response_hash);
    hasher.finalize().into()
}

// ============================================================================
// Instruction Builders (Phase 2: Feedback)
// ============================================================================

use solana_sdk::instruction::{AccountMeta, Instruction};

/// Build a `give_feedback` instruction for the Agent Registry program.
///
/// Accounts:
/// 0. [writable] agent PDA (["agent", asset])
/// 1. [] asset (NFT mint)
/// 2. [signer] client (feedback author / fee payer)
/// 3. [writable] atom_stats PDA (["atom_stats", asset]) on ATOM Engine
/// 4. [] atom_engine program
/// 5. [] system_program
pub fn build_give_feedback_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    client: &Pubkey,
    value: i128,
    value_decimals: u8,
    score: Option<u8>,
    tag1: &str,
    tag2: &str,
    endpoint: &str,
    feedback_uri: &str,
    feedback_hash: Option<[u8; 32]>,
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);
    let (atom_stats_pda, _) = derive_atom_stats_pda(asset, &programs.atom_engine);

    // Serialize args using Borsh (Anchor format)
    let mut data = Vec::with_capacity(256);
    data.extend_from_slice(&IX_GIVE_FEEDBACK);
    // i128 as 16 bytes LE
    data.extend_from_slice(&value.to_le_bytes());
    // u8
    data.push(value_decimals);
    // Option<u8>
    match score {
        Some(s) => {
            data.push(1);
            data.push(s);
        }
        None => data.push(0),
    }
    // Option<[u8; 32]>
    match feedback_hash {
        Some(h) => {
            data.push(1);
            data.extend_from_slice(&h);
        }
        None => data.push(0),
    }
    // String (4-byte LE length prefix + bytes)
    borsh_write_string(&mut data, tag1);
    borsh_write_string(&mut data, tag2);
    borsh_write_string(&mut data, endpoint);
    borsh_write_string(&mut data, feedback_uri);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new(*client, true),
            AccountMeta::new(atom_stats_pda, false),
            AccountMeta::new_readonly(programs.atom_engine, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    }
}

/// Build a `revoke_feedback` instruction.
///
/// Accounts:
/// 0. [writable] agent PDA
/// 1. [] asset
/// 2. [writable] atom_stats PDA on ATOM Engine
/// 3. [] atom_engine program
/// 4. [] system_program
pub fn build_revoke_feedback_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    feedback_index: u64,
    seal_hash: [u8; 32],
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);
    let (atom_stats_pda, _) = derive_atom_stats_pda(asset, &programs.atom_engine);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_REVOKE_FEEDBACK);
    data.extend_from_slice(&feedback_index.to_le_bytes());
    data.extend_from_slice(&seal_hash);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new(atom_stats_pda, false),
            AccountMeta::new_readonly(programs.atom_engine, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    }
}

/// Build an `append_response` instruction.
///
/// Accounts:
/// 0. [writable] agent PDA
/// 1. [] asset
/// 2. [] client_address (original feedback author)
/// 3. [signer] responder
/// 4. [] system_program
pub fn build_append_response_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    client_address: &Pubkey,
    responder: &Pubkey,
    feedback_index: u64,
    response_uri: &str,
    response_hash: [u8; 32],
    seal_hash: [u8; 32],
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);

    let mut data = Vec::with_capacity(128);
    data.extend_from_slice(&IX_APPEND_RESPONSE);
    data.extend_from_slice(&feedback_index.to_le_bytes());
    borsh_write_string(&mut data, response_uri);
    data.extend_from_slice(&response_hash);
    data.extend_from_slice(&seal_hash);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new_readonly(*client_address, false),
            AccountMeta::new(*responder, true),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    }
}

// ============================================================================
// Instruction Builders (Phase 3: Registration)
// ============================================================================

/// Build a `register` instruction to mint a new agent NFT.
///
/// Accounts:
/// 0. [writable] config PDA (["config"])
/// 1. [writable] agent PDA (["agent", asset])
/// 2. [] collection (Core Collection)
/// 3. [writable] asset (new NFT keypair - must be signer)
/// 4. [signer] owner (registrant / fee payer)
/// 5. [] system_program
/// 6. [] metaplex_core program
pub fn build_register_ix(
    programs: &SolanaErc8004Programs,
    collection: &Pubkey,
    asset: &Pubkey,
    owner: &Pubkey,
    agent_uri: &str,
) -> Instruction {
    let (config_pda, _) = derive_registry_config_pda(&programs.agent_registry);
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_REGISTER);
    borsh_write_string(&mut data, agent_uri);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(config_pda, false),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*collection, false),
            AccountMeta::new(*asset, true),
            AccountMeta::new(*owner, true),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(METAPLEX_CORE_PROGRAM, false),
        ],
        data,
    }
}

/// Build a `set_agent_uri` instruction.
///
/// Accounts:
/// 0. [writable] config PDA
/// 1. [writable] agent PDA
/// 2. [writable] asset (NFT)
/// 3. [signer] owner
/// 4. [] system_program
/// 5. [] metaplex_core program
pub fn build_set_agent_uri_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    owner: &Pubkey,
    new_uri: &str,
) -> Instruction {
    let (config_pda, _) = derive_registry_config_pda(&programs.agent_registry);
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_SET_AGENT_URI);
    borsh_write_string(&mut data, new_uri);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(config_pda, false),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new(*asset, false),
            AccountMeta::new(*owner, true),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(METAPLEX_CORE_PROGRAM, false),
        ],
        data,
    }
}

/// Build a `set_metadata_pda` instruction.
///
/// Accounts:
/// 0. [writable] agent PDA
/// 1. [writable] metadata_entry PDA (["agent_meta", asset, key_hash[0..8]])
/// 2. [] asset
/// 3. [signer] owner
/// 4. [] system_program
pub fn build_set_metadata_pda_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    owner: &Pubkey,
    key: &str,
    value: &[u8],
    immutable: bool,
) -> Instruction {
    use sha2::{Digest, Sha256};
    let key_hash = Sha256::digest(key.as_bytes());
    let key_hash_prefix: [u8; 8] = key_hash[..8].try_into().unwrap();

    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);
    let (metadata_pda, _) = derive_metadata_pda(asset, key, &programs.agent_registry);

    let mut data = Vec::with_capacity(128);
    data.extend_from_slice(&IX_SET_METADATA_PDA);
    data.extend_from_slice(&key_hash_prefix);
    borsh_write_string(&mut data, key);
    // Vec<u8> (4-byte LE length prefix + raw bytes)
    data.extend_from_slice(&(value.len() as u32).to_le_bytes());
    data.extend_from_slice(value);
    // bool
    data.push(if immutable { 1 } else { 0 });

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(agent_pda, false),
            AccountMeta::new(metadata_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new(*owner, true),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    }
}

// ============================================================================
// Transaction Helpers
// ============================================================================

use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

/// Build, sign, send, and confirm a single-instruction transaction.
///
/// The facilitator keypair is used as both the fee payer and signer.
pub async fn send_erc8004_transaction(
    rpc_client: &RpcClient,
    keypair: &Keypair,
    instructions: Vec<Instruction>,
) -> Result<Signature, SolanaErc8004Error> {
    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .await
        .map_err(|e| SolanaErc8004Error::RpcError(format!("Failed to get blockhash: {}", e)))?;

    let tx = Transaction::new_signed_with_payer(
        &instructions,
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );

    rpc_client
        .send_and_confirm_transaction(&tx)
        .await
        .map_err(|e| SolanaErc8004Error::RpcError(format!("Transaction failed: {}", e)))
}

/// Build, sign, send, and confirm a transaction that requires multiple signers.
///
/// Used for register() where the new NFT asset keypair must also sign.
pub async fn send_erc8004_transaction_with_signers(
    rpc_client: &RpcClient,
    fee_payer: &Keypair,
    signers: &[&Keypair],
    instructions: Vec<Instruction>,
) -> Result<Signature, SolanaErc8004Error> {
    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .await
        .map_err(|e| SolanaErc8004Error::RpcError(format!("Failed to get blockhash: {}", e)))?;

    let tx = Transaction::new_signed_with_payer(
        &instructions,
        Some(&fee_payer.pubkey()),
        signers,
        recent_blockhash,
    );

    rpc_client
        .send_and_confirm_transaction(&tx)
        .await
        .map_err(|e| SolanaErc8004Error::RpcError(format!("Transaction failed: {}", e)))
}

/// Read the collection pubkey from the RegistryConfig PDA.
pub async fn read_collection_pubkey(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
) -> Result<Pubkey, SolanaErc8004Error> {
    let config = read_registry_config(rpc_client, program_id).await?;
    Ok(bytes_to_pubkey(&config.collection))
}

// ============================================================================
// Borsh Serialization Helper
// ============================================================================

/// Write a Borsh-encoded string (4-byte LE length prefix + UTF-8 bytes).
fn borsh_write_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
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

    // ====================================================================
    // Phase 2 + 3 Tests
    // ====================================================================

    #[test]
    fn test_give_feedback_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let client = Pubkey::new_unique();

        let ix = build_give_feedback_ix(
            &programs,
            &asset,
            &client,
            87,
            0,
            Some(85),
            "quality",
            "api",
            "https://api.example.com",
            "ipfs://QmFeedback",
            None,
        );

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 6);
        // First 8 bytes should be discriminator
        assert_eq!(&ix.data[..8], &IX_GIVE_FEEDBACK);
        // client should be signer
        assert!(ix.accounts[2].is_signer);
    }

    #[test]
    fn test_revoke_feedback_instruction() {
        let programs = get_program_ids(&Network::SolanaDevnet).unwrap();
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let seal_hash = [0xABu8; 32];

        let ix = build_revoke_feedback_ix(&programs, &asset, 1, seal_hash);

        assert_eq!(ix.program_id, AGENT_REGISTRY_DEVNET);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(&ix.data[..8], &IX_REVOKE_FEEDBACK);
    }

    #[test]
    fn test_append_response_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::new_unique();
        let client = Pubkey::new_unique();
        let responder = Pubkey::new_unique();

        let ix = build_append_response_ix(
            &programs,
            &asset,
            &client,
            &responder,
            1,
            "ipfs://QmResponse",
            [0u8; 32],
            [0u8; 32],
        );

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(&ix.data[..8], &IX_APPEND_RESPONSE);
        assert!(ix.accounts[3].is_signer); // responder
    }

    #[test]
    fn test_register_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let collection = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let ix = build_register_ix(
            &programs,
            &collection,
            &asset,
            &owner,
            "ipfs://QmAgentSpec",
        );

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 7);
        assert_eq!(&ix.data[..8], &IX_REGISTER);
        assert!(ix.accounts[3].is_signer); // asset
        assert!(ix.accounts[4].is_signer); // owner
    }

    #[test]
    fn test_set_metadata_pda_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let ix = build_set_metadata_pda_ix(
            &programs,
            &asset,
            &owner,
            "x402Support",
            b"true",
            false,
        );

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(&ix.data[..8], &IX_SET_METADATA_PDA);
        assert!(ix.accounts[3].is_signer); // owner
    }

    #[test]
    fn test_seal_hash_deterministic() {
        let agent = Pubkey::new_unique();
        let client = Pubkey::new_unique();
        let hash1 = compute_feedback_seal_hash(
            &agent, &client, 0, "ipfs://QmTest", &[0u8; 32],
        );
        let hash2 = compute_feedback_seal_hash(
            &agent, &client, 0, "ipfs://QmTest", &[0u8; 32],
        );
        assert_eq!(hash1, hash2);

        // Different feedback_count should produce different hash
        let hash3 = compute_feedback_seal_hash(
            &agent, &client, 1, "ipfs://QmTest", &[0u8; 32],
        );
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_seal_hash_domains() {
        let agent = Pubkey::new_unique();
        let client = Pubkey::new_unique();

        let feedback_hash = compute_feedback_seal_hash(
            &agent, &client, 0, "uri", &[0u8; 32],
        );
        let revoke_hash = compute_revoke_seal_hash(
            &agent, &client, 0, 0,
        );
        let response_hash = compute_response_seal_hash(
            &agent, &client, 0, "uri", &[0u8; 32],
        );

        // All three should be different due to different domains
        assert_ne!(feedback_hash, revoke_hash);
        assert_ne!(feedback_hash, response_hash);
        assert_ne!(revoke_hash, response_hash);
    }

    #[test]
    fn test_borsh_write_string() {
        let mut buf = Vec::new();
        borsh_write_string(&mut buf, "hello");
        // 4-byte LE length (5) + 5 bytes of "hello"
        assert_eq!(buf.len(), 9);
        assert_eq!(&buf[..4], &5u32.to_le_bytes());
        assert_eq!(&buf[4..], b"hello");
    }

    #[test]
    fn test_set_agent_uri_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let ix = build_set_agent_uri_ix(
            &programs, &asset, &owner, "ipfs://QmNewUri",
        );

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 6);
        assert_eq!(&ix.data[..8], &IX_SET_AGENT_URI);
    }
}
