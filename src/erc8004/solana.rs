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

/// Discriminator for RootConfig: SHA256("account:RootConfig")[..8]
const ROOT_CONFIG_DISCRIMINATOR: [u8; 8] = [42, 216, 8, 82, 19, 209, 223, 246];

/// Discriminator for MetadataEntryPda: SHA256("account:MetadataEntryPda")[..8]
const METADATA_ENTRY_DISCRIMINATOR: [u8; 8] = [48, 145, 12, 249, 176, 141, 197, 187];

/// Metaplex Core account discriminator byte for `CollectionV1` (first byte of the account).
const MPL_CORE_COLLECTION_V1_KEY: u8 = 5;

// ============================================================================
// Borsh-Deserialized Account Structures
// ============================================================================

/// AgentAccount (variable size; ~748 bytes as minted today, 8-byte discriminator included)
///
/// PDA Seeds: `["agent", asset.key()]`
///
/// The primary identity record for a Solana-registered agent.
///
/// Field order matters and is not obvious: the four pubkeys lead, the variable-length
/// strings trail, and two `Option<Pubkey>` tags sit in between. An earlier revision of
/// this struct omitted `collection`, `creator`, `atom_enabled`, `agent_wallet`,
/// `parent_asset`, `parent_locked`, `col_locked` and `col`, and put the strings in the
/// middle, so every read failed with "Unexpected length of input".
#[derive(Debug, Clone, BorshDeserialize)]
pub struct AgentAccount {
    /// Collection this agent belongs to
    pub collection: [u8; 32],
    /// Immutable creator snapshot
    pub creator: [u8; 32],
    /// NFT owner address (cached from the Core asset)
    pub owner: [u8; 32],
    /// Metaplex Core NFT mint address (unique agent identifier)
    pub asset: [u8; 32],
    /// PDA bump seed
    pub bump: u8,
    /// Whether the ATOM Engine is enabled for this agent
    pub atom_enabled: u8,
    /// Operational wallet (Ed25519 verified), if set
    pub agent_wallet: Option<[u8; 32]>,
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
    /// Parent agent asset, for hierarchical agents
    pub parent_asset: Option<[u8; 32]>,
    /// Whether the parent link is locked
    pub parent_locked: u8,
    /// Whether the collection pointer is locked
    pub col_locked: u8,
    /// URI to agent registration file (IPFS/HTTPS)
    pub agent_uri: String,
    /// Human-readable agent name
    pub nft_name: String,
    /// Canonical collection pointer (`c1:<cid>`)
    pub col: String,
}

/// MetadataEntryPda (variable size, 8-byte Anchor discriminator included)
///
/// PDA Seeds: `["agent_meta", asset.key(), sha256(key)[0..16]]`
#[derive(Debug, Clone, BorshDeserialize)]
pub struct MetadataEntryPda {
    /// Agent NFT mint this entry belongs to
    pub asset: [u8; 32],
    /// Whether the entry can still be changed
    pub immutable: u8,
    /// PDA bump seed
    pub bump: u8,
    /// Metadata key
    pub metadata_key: String,
    /// Metadata value bytes
    pub metadata_value: Vec<u8>,
}

/// AtomStats (561 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["atom_stats", asset.key()]`
///
/// ATOM Engine reputation analytics computed on-chain via CPI.
///
/// The engine's real record is far wider than the summary this facilitator exposes,
/// and every field has to be declared to reach the ones at the end (`trust_tier`,
/// `confidence`, `bump` all live past byte 540). A previous 16-field version of this
/// struct sized ~430 bytes and never deserialized, which surfaced as a permanently
/// null `atomStats` in `/reputation`.
///
/// Note there are no positive/negative counters: the engine tracks quality through
/// EMA scores, not tallies.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct AtomStats {
    /// Registry collection address
    pub collection: [u8; 32],
    /// Agent NFT mint address
    pub asset: [u8; 32],
    /// Slot of the first feedback ever recorded
    pub first_feedback_slot: u64,
    /// Slot of most recent feedback
    pub last_feedback_slot: u64,
    /// Total feedback count
    pub feedback_count: u64,
    /// Fast exponential moving average of the score
    pub ema_score_fast: u16,
    /// Slow exponential moving average of the score
    pub ema_score_slow: u16,
    /// EMA of score volatility
    pub ema_volatility: u16,
    /// EMA of the log of arrival intervals
    pub ema_arrival_log: u16,
    /// Highest EMA reached
    pub peak_ema: u16,
    /// Largest drawdown from the peak
    pub max_drawdown: u16,
    /// Number of epochs observed
    pub epoch_count: u16,
    /// Current epoch
    pub current_epoch: u16,
    /// Lowest score seen
    pub min_score: u8,
    /// Highest score seen
    pub max_score: u8,
    /// First score recorded
    pub first_score: u8,
    /// Most recent score recorded
    pub last_score: u8,
    /// HyperLogLog registers (256 x 4-bit packed, for unique client estimation)
    pub hll_packed: [u8; 128],
    /// Per-agent salt for HLL grinding prevention
    pub hll_salt: u64,
    /// Ring buffer for burst detection (24 x 56-bit fingerprints)
    pub recent_callers: [u64; 24],
    /// Burst pressure accumulator
    pub burst_pressure: u8,
    /// Updates observed since the HLL last changed
    pub updates_since_hll_change: u8,
    /// Negative-feedback pressure accumulator
    pub neg_pressure: u8,
    /// Ring buffer cursor
    pub eviction_cursor: u8,
    /// Base slot for MRT eviction protection
    pub ring_base_slot: u64,
    /// Rate of quality change, for the circuit breaker
    pub quality_velocity: u16,
    /// Epoch the velocity was measured in
    pub velocity_epoch: u16,
    /// Epochs remaining in a quality freeze
    pub freeze_epochs: u8,
    /// Lower bound enforced on quality
    pub quality_floor: u8,
    /// Number of bypasses recorded
    pub bypass_count: u8,
    /// Average score across bypasses
    pub bypass_score_avg: u8,
    /// Fingerprints retained to support revocation
    pub bypass_fingerprints: [u64; 10],
    /// Bypass fingerprint ring cursor
    pub bypass_fp_cursor: u8,
    /// Cached loyalty score
    pub loyalty_score: u16,
    /// Cached quality score
    pub quality_score: u16,
    /// Risk assessment (0-100)
    pub risk_score: u8,
    /// Client diversity measure from HyperLogLog (0-100)
    pub diversity_ratio: u8,
    /// Trust tier (0-4): Unknown, New, Established, Trusted, Legendary
    pub trust_tier: u8,
    /// Tier awaiting vesting confirmation
    pub tier_candidate: u8,
    /// Epoch the candidate tier was proposed in
    pub tier_candidate_epoch: u16,
    /// Tier confirmed after vesting
    pub tier_confirmed: u8,
    /// Bit flags
    pub flags: u8,
    /// Statistical confidence (0-100)
    pub confidence: u16,
    /// PDA bump seed
    pub bump: u8,
    /// On-chain schema version
    pub schema_version: u8,
}

/// RootConfig (73 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["root_config"]`
///
/// Global singleton pointing at the base Metaplex Core collection. Introduced in
/// program v0.3.0; this is the entry point for resolving every other registry account.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct RootConfig {
    /// Base Metaplex Core collection address
    pub base_collection: [u8; 32],
    /// Upgrade authority
    pub authority: [u8; 32],
    /// PDA bump seed
    pub bump: u8,
}

/// RegistryConfig (73 bytes on-chain, including 8-byte Anchor discriminator)
///
/// PDA Seeds: `["registry_config", collection]`
///
/// Per-collection registry configuration.
///
/// NOTE: this account carries no agent counter. Earlier revisions of this module
/// declared `registry_type` and `base_index` fields that do not exist on-chain,
/// which made every deserialization fail. Agent totals come from the Metaplex Core
/// collection instead - see [`CollectionSupply`].
#[derive(Debug, Clone, BorshDeserialize)]
pub struct RegistryConfig {
    /// Metaplex Core collection address
    pub collection: [u8; 32],
    /// Registry authority
    pub authority: [u8; 32],
    /// PDA bump seed
    pub bump: u8,
}

/// Agent totals read from the Metaplex Core `CollectionV1` account.
///
/// The registry has no on-chain counter of its own; the collection is the golden
/// source. `num_minted` only ever increases, `current_size` decreases on burn.
#[derive(Debug, Clone, BorshDeserialize)]
pub struct CollectionSupply {
    /// Metaplex Core account discriminator byte (5 = CollectionV1)
    pub key: u8,
    /// Collection update authority
    pub update_authority: [u8; 32],
    /// Collection name
    pub name: String,
    /// Collection metadata URI
    pub uri: String,
    /// Agents minted since genesis (monotonic)
    pub num_minted: u32,
    /// Agents currently in the collection (net of burns)
    pub current_size: u32,
}

/// Resolved registry accounts for one Solana network.
///
/// Building any identity instruction needs both config PDAs plus the collection,
/// and the collection is only knowable after reading `root_config`. Resolve once
/// with [`read_registry_context`] and thread this through.
#[derive(Debug, Clone, Copy)]
pub struct RegistryContext {
    /// `["root_config"]` PDA
    pub root_config: Pubkey,
    /// `["registry_config", collection]` PDA
    pub registry_config: Pubkey,
    /// Metaplex Core collection holding the agent NFTs
    pub collection: Pubkey,
    /// Registry authority
    pub authority: Pubkey,
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

/// Derive the RootConfig PDA.
///
/// Seeds: `["root_config"]`
pub fn derive_root_config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"root_config"], program_id)
}

/// Derive the RegistryConfig PDA for a collection.
///
/// Seeds: `["registry_config", collection]`
///
/// The collection is not a constant - read it from the RootConfig account first
/// (see [`read_registry_context`]). The legacy `["config"]` seed used before
/// program v0.3.0 derives an address that has never been initialized.
pub fn derive_registry_config_pda(program_id: &Pubkey, collection: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"registry_config", collection.as_ref()], program_id)
}

/// Derive the AtomConfig PDA on the ATOM Engine program.
///
/// Seeds: `["atom_config"]`
pub fn derive_atom_config_pda(atom_program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"atom_config"], atom_program_id)
}

/// Derive the ATOM CPI authority PDA on the Agent Registry program.
///
/// Seeds: `["atom_cpi_authority"]`
///
/// The registry signs its CPI calls into the ATOM Engine with this PDA; the engine
/// rejects feedback whose `registry_authority` account does not match.
pub fn derive_atom_cpi_authority_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"atom_cpi_authority"], program_id)
}

/// SHA-256 prefix of a metadata key, as used in both the PDA seed and the
/// instruction payload.
///
/// 16 bytes, not 8: the program widened this in its v1.9 security update.
fn metadata_key_hash(metadata_key: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(metadata_key.as_bytes());
    digest[..16].try_into().expect("sha256 yields 32 bytes")
}

/// Derive the MetadataEntryPda for a given asset and metadata key.
///
/// Seeds: `["agent_meta", asset.key(), sha256(key)[0..16]]`
pub fn derive_metadata_pda(
    asset: &Pubkey,
    metadata_key: &str,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    let key_hash = metadata_key_hash(metadata_key);
    Pubkey::find_program_address(&[b"agent_meta", asset.as_ref(), &key_hash], program_id)
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

    let account_data = rpc_client.get_account_data(&pda).await.map_err(|e| {
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

    // Deserialize from bytes after the 8-byte discriminator. The account is allocated
    // for max-length strings, so it carries slack past the struct: do not use
    // try_from_slice, which rejects trailing bytes.
    AgentAccount::deserialize(&mut &account_data[8..]).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!("Failed to deserialize AgentAccount: {}", e))
    })
}

/// Read and deserialize a MetadataEntryPda from the chain.
pub async fn read_metadata_entry(
    rpc_client: &RpcClient,
    asset_pubkey: &Pubkey,
    metadata_key: &str,
    program_id: &Pubkey,
) -> Result<MetadataEntryPda, SolanaErc8004Error> {
    let (pda, _bump) = derive_metadata_pda(asset_pubkey, metadata_key, program_id);
    let payload = fetch_anchor_account(
        rpc_client,
        &pda,
        &METADATA_ENTRY_DISCRIMINATOR,
        "Metadata entry",
    )
    .await?;

    MetadataEntryPda::deserialize(&mut payload.as_slice()).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!(
            "Failed to deserialize MetadataEntryPda: {}",
            e
        ))
    })
}

/// Read and deserialize AtomStats from the chain.
pub async fn read_atom_stats(
    rpc_client: &RpcClient,
    asset_pubkey: &Pubkey,
    atom_program_id: &Pubkey,
) -> Result<AtomStats, SolanaErc8004Error> {
    let (pda, _bump) = derive_atom_stats_pda(asset_pubkey, atom_program_id);

    let account_data = rpc_client.get_account_data(&pda).await.map_err(|e| {
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

    AtomStats::deserialize(&mut &account_data[8..]).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!("Failed to deserialize AtomStats: {}", e))
    })
}

/// Fetch an Anchor account, checking its discriminator, and return the payload after it.
///
/// Deserialization tolerates trailing bytes: Anchor accounts may be allocated larger
/// than the struct currently occupies.
async fn fetch_anchor_account(
    rpc_client: &RpcClient,
    pda: &Pubkey,
    discriminator: &[u8; 8],
    label: &str,
) -> Result<Vec<u8>, SolanaErc8004Error> {
    let account_data = rpc_client.get_account_data(pda).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("AccountNotFound") || err_str.contains("could not find account") {
            SolanaErc8004Error::AccountNotFound(format!("{} not found (PDA: {})", label, pda))
        } else {
            SolanaErc8004Error::RpcError(err_str)
        }
    })?;

    if account_data.len() < 8 {
        return Err(SolanaErc8004Error::InvalidAccountData(format!(
            "{} data too short for Anchor discriminator",
            label
        )));
    }

    if account_data[..8] != *discriminator {
        return Err(SolanaErc8004Error::InvalidAccountData(format!(
            "Invalid Anchor discriminator for {}",
            label
        )));
    }

    Ok(account_data[8..].to_vec())
}

/// Read and deserialize the RootConfig singleton from the chain.
pub async fn read_root_config(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
) -> Result<RootConfig, SolanaErc8004Error> {
    let (pda, _bump) = derive_root_config_pda(program_id);
    let payload =
        fetch_anchor_account(rpc_client, &pda, &ROOT_CONFIG_DISCRIMINATOR, "Root config").await?;

    RootConfig::deserialize(&mut payload.as_slice()).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!("Failed to deserialize RootConfig: {}", e))
    })
}

/// Read and deserialize the RegistryConfig for a collection from the chain.
pub async fn read_registry_config(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    collection: &Pubkey,
) -> Result<RegistryConfig, SolanaErc8004Error> {
    let (pda, _bump) = derive_registry_config_pda(program_id, collection);
    let payload = fetch_anchor_account(
        rpc_client,
        &pda,
        &REGISTRY_CONFIG_DISCRIMINATOR,
        "Registry config",
    )
    .await?;

    RegistryConfig::deserialize(&mut payload.as_slice()).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!(
            "Failed to deserialize RegistryConfig: {}",
            e
        ))
    })
}

/// Resolve every registry account needed to build identity instructions.
///
/// Costs two RPC round trips: the collection lives in RootConfig and seeds the
/// RegistryConfig PDA.
pub async fn read_registry_context(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
) -> Result<RegistryContext, SolanaErc8004Error> {
    let root = read_root_config(rpc_client, program_id).await?;
    let collection = bytes_to_pubkey(&root.base_collection);

    let (root_config, _) = derive_root_config_pda(program_id);
    let (registry_config, _) = derive_registry_config_pda(program_id, &collection);

    let config = read_registry_config(rpc_client, program_id, &collection).await?;

    Ok(RegistryContext {
        root_config,
        registry_config,
        collection,
        authority: bytes_to_pubkey(&config.authority),
    })
}

/// Read agent totals from the Metaplex Core collection account.
pub async fn read_collection_supply(
    rpc_client: &RpcClient,
    collection: &Pubkey,
) -> Result<CollectionSupply, SolanaErc8004Error> {
    let account_data = rpc_client.get_account_data(collection).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("AccountNotFound") || err_str.contains("could not find account") {
            SolanaErc8004Error::AccountNotFound(format!("Collection not found: {}", collection))
        } else {
            SolanaErc8004Error::RpcError(err_str)
        }
    })?;

    if account_data.first() != Some(&MPL_CORE_COLLECTION_V1_KEY) {
        return Err(SolanaErc8004Error::InvalidAccountData(format!(
            "Account {} is not a Metaplex Core CollectionV1",
            collection
        )));
    }

    // Trailing plugin data after the header is expected, so do not use try_from_slice.
    CollectionSupply::deserialize(&mut account_data.as_slice()).map_err(|e| {
        SolanaErc8004Error::InvalidAccountData(format!(
            "Failed to deserialize CollectionV1 {}: {}",
            collection, e
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

/// 16 bytes, matching `seal.rs` on-chain.
const DOMAIN_SEAL_V1: &[u8] = b"8004_SEAL_V1____";
/// 16 bytes, matching `seal.rs` on-chain.
const DOMAIN_LEAF_V1: &[u8] = b"8004_LEAF_V1____";

/// Max byte lengths enforced by the program before it hashes (`state.rs`).
const MAX_TAG_LEN: usize = 32;
const MAX_ENDPOINT_LEN: usize = 250;
const MAX_URI_LEN: usize = 250;

// ============================================================================
// SEAL v1 Hash Computation
// ============================================================================

/// The feedback content a SEAL v1 hash commits to.
#[derive(Debug, Clone)]
pub struct SealParams<'a> {
    /// Feedback value (fixed point)
    pub value: i128,
    /// Decimal places for `value` (0-18)
    pub value_decimals: u8,
    /// Optional score (0-100)
    pub score: Option<u8>,
    /// Optional hash of the off-chain feedback file
    pub feedback_file_hash: Option<[u8; 32]>,
    /// Primary tag
    pub tag1: &'a str,
    /// Secondary tag
    pub tag2: &'a str,
    /// Endpoint that was used
    pub endpoint: &'a str,
    /// URI of the off-chain feedback file
    pub feedback_uri: &'a str,
}

/// Compute the SEAL v1 hash of a feedback, byte-identical to the on-chain routine.
///
/// This is keccak256, not SHA-256, and it commits to the feedback *content* only -
/// no agent or client pubkey enters the hash. An earlier version of this module had
/// three separate SHA-256 functions with invented domain constants
/// (`8004_FEED_V1___`, `8004_REVK_V1___`, `8004_RESP_V1___`); none of those domains
/// exist in the program, and no hash they produced could ever be accepted.
///
/// Layout: fixed 36-byte prefix, then the dynamic tail.
///
/// - `DOMAIN_SEAL_V1` (16) | `value` i128 LE (16) | `value_decimals` (1)
/// - `score_flag` (1) | `score_value` (1) | `file_hash_flag` (1)
/// - `file_hash` (32, only when present)
/// - for each of tag1, tag2, endpoint, feedback_uri: `len` u16 LE (2) + UTF-8 bytes
///
/// Returns `None` when an input exceeds the on-chain limits, since the program
/// would reject such a feedback before hashing it.
pub fn compute_seal_hash(params: &SealParams<'_>) -> Option<[u8; 32]> {
    if params.tag1.len() > MAX_TAG_LEN
        || params.tag2.len() > MAX_TAG_LEN
        || params.endpoint.len() > MAX_ENDPOINT_LEN
        || params.feedback_uri.len() > MAX_URI_LEN
        || params.value_decimals > 18
    {
        return None;
    }
    if let Some(score) = params.score {
        if score > 100 {
            return None;
        }
    }

    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(DOMAIN_SEAL_V1);
    buf.extend_from_slice(&params.value.to_le_bytes());
    buf.push(params.value_decimals);
    match params.score {
        // Score always occupies two bytes: flag then value (0 as placeholder).
        Some(score) => buf.extend_from_slice(&[1, score]),
        None => buf.extend_from_slice(&[0, 0]),
    }
    buf.push(u8::from(params.feedback_file_hash.is_some()));
    if let Some(hash) = params.feedback_file_hash {
        buf.extend_from_slice(&hash);
    }
    for s in [
        params.tag1,
        params.tag2,
        params.endpoint,
        params.feedback_uri,
    ] {
        buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    Some(alloy::primitives::keccak256(&buf).0)
}

/// Compute the SEAL v1 feedback leaf, which binds a seal hash to its context.
///
/// `keccak256(DOMAIN_LEAF_V1 || asset || client || feedback_index LE || seal_hash || slot LE)`
pub fn compute_feedback_leaf_v1(
    asset: &Pubkey,
    client: &Pubkey,
    feedback_index: u64,
    seal_hash: &[u8; 32],
    slot: u64,
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(DOMAIN_LEAF_V1);
    buf.extend_from_slice(asset.as_ref());
    buf.extend_from_slice(client.as_ref());
    buf.extend_from_slice(&feedback_index.to_le_bytes());
    buf.extend_from_slice(seal_hash);
    buf.extend_from_slice(&slot.to_le_bytes());
    alloy::primitives::keccak256(&buf).0
}

// ============================================================================
// Instruction Builders (Phase 2: Feedback)
// ============================================================================

use solana_sdk::instruction::{AccountMeta, Instruction};

/// Build a `give_feedback` instruction for the Agent Registry program.
///
/// Accounts:
/// 0. [signer, writable] client (feedback author / fee payer)
/// 1. [writable] agent PDA (["agent", asset])
/// 2. [] asset (NFT mint)
/// 3. [] collection (Core Collection)
/// 4. [] system_program
/// 5. [] atom_config PDA (["atom_config"]) on ATOM Engine
/// 6. [writable] atom_stats PDA (["atom_stats", asset]) on ATOM Engine
/// 7. [] atom_engine program
/// 8. [] registry ATOM CPI authority PDA (["atom_cpi_authority"])
pub fn build_give_feedback_ix(
    programs: &SolanaErc8004Programs,
    collection: &Pubkey,
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
    let (atom_config_pda, _) = derive_atom_config_pda(&programs.atom_engine);
    let (atom_stats_pda, _) = derive_atom_stats_pda(asset, &programs.atom_engine);
    let (atom_cpi_authority, _) = derive_atom_cpi_authority_pda(&programs.agent_registry);

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
            AccountMeta::new(*client, true),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new_readonly(*collection, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(atom_config_pda, false),
            AccountMeta::new(atom_stats_pda, false),
            AccountMeta::new_readonly(programs.atom_engine, false),
            AccountMeta::new_readonly(atom_cpi_authority, false),
        ],
        data,
    }
}

/// Build a `revoke_feedback` instruction.
///
/// Accounts:
/// 0. [signer, writable] client (revoker / fee payer)
/// 1. [writable] agent PDA
/// 2. [] asset
/// 3. [] system_program
/// 4. [] atom_config PDA on ATOM Engine
/// 5. [writable] atom_stats PDA on ATOM Engine
/// 6. [] atom_engine program
/// 7. [] registry ATOM CPI authority PDA
pub fn build_revoke_feedback_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    client: &Pubkey,
    feedback_index: u64,
    seal_hash: [u8; 32],
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);
    let (atom_config_pda, _) = derive_atom_config_pda(&programs.atom_engine);
    let (atom_stats_pda, _) = derive_atom_stats_pda(asset, &programs.atom_engine);
    let (atom_cpi_authority, _) = derive_atom_cpi_authority_pda(&programs.agent_registry);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_REVOKE_FEEDBACK);
    data.extend_from_slice(&feedback_index.to_le_bytes());
    data.extend_from_slice(&seal_hash);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new(*client, true),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(atom_config_pda, false),
            AccountMeta::new(atom_stats_pda, false),
            AccountMeta::new_readonly(programs.atom_engine, false),
            AccountMeta::new_readonly(atom_cpi_authority, false),
        ],
        data,
    }
}

/// Build an `append_response` instruction.
///
/// Accounts:
/// 0. [signer] responder
/// 1. [writable] agent PDA
/// 2. [] asset
///
/// The original feedback author travels in the instruction payload, not as an account.
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
    data.extend_from_slice(client_address.as_ref());
    data.extend_from_slice(&feedback_index.to_le_bytes());
    borsh_write_string(&mut data, response_uri);
    data.extend_from_slice(&response_hash);
    data.extend_from_slice(&seal_hash);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new_readonly(*responder, true),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new_readonly(*asset, false),
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
/// 0. [] root_config PDA (["root_config"])
/// 1. [] registry_config PDA (["registry_config", collection])
/// 2. [writable] agent PDA (["agent", asset])
/// 3. [signer, writable] asset (new NFT keypair)
/// 4. [writable] collection (Core Collection - mpl-core bumps its counters)
/// 5. [signer, writable] owner (registrant / fee payer)
/// 6. [] system_program
/// 7. [] metaplex_core program
pub fn build_register_ix(
    programs: &SolanaErc8004Programs,
    ctx: &RegistryContext,
    asset: &Pubkey,
    owner: &Pubkey,
    agent_uri: &str,
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_REGISTER);
    borsh_write_string(&mut data, agent_uri);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new_readonly(ctx.root_config, false),
            AccountMeta::new_readonly(ctx.registry_config, false),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new(*asset, true),
            AccountMeta::new(ctx.collection, false),
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
/// 0. [] registry_config PDA (["registry_config", collection])
/// 1. [writable] agent PDA
/// 2. [writable] asset (NFT)
/// 3. [writable] collection (Core Collection)
/// 4. [signer, writable] owner
/// 5. [] system_program
/// 6. [] metaplex_core program
pub fn build_set_agent_uri_ix(
    programs: &SolanaErc8004Programs,
    ctx: &RegistryContext,
    asset: &Pubkey,
    owner: &Pubkey,
    new_uri: &str,
) -> Instruction {
    let (agent_pda, _) = derive_agent_pda(asset, &programs.agent_registry);

    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&IX_SET_AGENT_URI);
    borsh_write_string(&mut data, new_uri);

    Instruction {
        program_id: programs.agent_registry,
        accounts: vec![
            AccountMeta::new_readonly(ctx.registry_config, false),
            AccountMeta::new(agent_pda, false),
            AccountMeta::new(*asset, false),
            AccountMeta::new(ctx.collection, false),
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
/// 0. [writable] metadata_entry PDA (["agent_meta", asset, key_hash[0..16]])
/// 1. [] agent PDA
/// 2. [] asset
/// 3. [signer, writable] owner
/// 4. [] system_program
pub fn build_set_metadata_pda_ix(
    programs: &SolanaErc8004Programs,
    asset: &Pubkey,
    owner: &Pubkey,
    key: &str,
    value: &[u8],
    immutable: bool,
) -> Instruction {
    let key_hash_prefix = metadata_key_hash(key);

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
            AccountMeta::new(metadata_pda, false),
            AccountMeta::new_readonly(agent_pda, false),
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

/// Read the collection pubkey from the RootConfig PDA.
pub async fn read_collection_pubkey(
    rpc_client: &RpcClient,
    program_id: &Pubkey,
) -> Result<Pubkey, SolanaErc8004Error> {
    let root = read_root_config(rpc_client, program_id).await?;
    Ok(bytes_to_pubkey(&root.base_collection))
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

    /// Base collection held by RootConfig on mainnet (verified on-chain 2026-08-07).
    const MAINNET_COLLECTION: &str = "DbjsWo7iUs7QZyJxLgNyVxvAAjQZCXroJHoGok8h8Umg";
    /// Base collection held by RootConfig on devnet (verified on-chain 2026-08-07).
    const DEVNET_COLLECTION: &str = "6CTyGPcn8dMwKEqgtvx2XCpkGUd7uqCVK6937RSM5bhA";

    /// Config PDAs are pinned to addresses read from the live registries. An
    /// `assert_ne!(pda, default)` check passes even with a wrong seed, which is how
    /// the deprecated `["config"]` seed survived here undetected.
    #[test]
    fn test_root_config_pda_matches_mainnet() {
        let (pda, _) = derive_root_config_pda(&AGENT_REGISTRY_MAINNET);
        assert_eq!(
            pda.to_string(),
            "FkmKMw5a8HfE733zJ1qCaLVNR7iMFFhEU5dHWxkfBCue"
        );

        let (pda, _) = derive_root_config_pda(&AGENT_REGISTRY_DEVNET);
        assert_eq!(
            pda.to_string(),
            "GGQfKNpXq8HchNxecLfXi8D7xz9PDppdPAPgr5Fx4Nvd"
        );
    }

    #[test]
    fn test_registry_config_pda_matches_mainnet() {
        let collection = Pubkey::from_str(MAINNET_COLLECTION).unwrap();
        let (pda, _) = derive_registry_config_pda(&AGENT_REGISTRY_MAINNET, &collection);
        assert_eq!(
            pda.to_string(),
            "BXnjUb5ZqEXovwTCrRzh6JPRxFydprLERJV2JJyCFZUz"
        );

        let collection = Pubkey::from_str(DEVNET_COLLECTION).unwrap();
        let (pda, _) = derive_registry_config_pda(&AGENT_REGISTRY_DEVNET, &collection);
        assert_eq!(
            pda.to_string(),
            "Djy4TKPvFyEumcVTDCqJUHWErKqcaeRj4ULWwaPkedor"
        );
    }

    /// The legacy seed must never come back: it derives an address that was never
    /// initialized, so reads 404 and writes fail account validation.
    #[test]
    fn test_legacy_config_seed_is_not_used() {
        let legacy = Pubkey::find_program_address(&[b"config"], &AGENT_REGISTRY_MAINNET).0;
        assert_eq!(
            legacy.to_string(),
            "C9uJoGDyNQFp3gFrdYY27JsCd8utGF9TZmgH1vdzeVHL"
        );

        let (root, _) = derive_root_config_pda(&AGENT_REGISTRY_MAINNET);
        let collection = Pubkey::from_str(MAINNET_COLLECTION).unwrap();
        let (registry, _) = derive_registry_config_pda(&AGENT_REGISTRY_MAINNET, &collection);
        assert_ne!(legacy, root);
        assert_ne!(legacy, registry);
    }

    #[test]
    fn test_atom_pdas() {
        let (atom_config, _) = derive_atom_config_pda(&ATOM_ENGINE_MAINNET);
        assert_eq!(
            atom_config.to_string(),
            "7mFFwuy7ryTnrRMKK246be2LwD8rgZ9DvWiZioQtuCtA"
        );

        // Derived on the registry program, not the engine.
        let (cpi_authority, _) = derive_atom_cpi_authority_pda(&AGENT_REGISTRY_MAINNET);
        assert_eq!(
            cpi_authority.to_string(),
            "BropKd6eEHTiTSsbecKKHr9d1zwW94BC9JXmGYg22BJx"
        );
        assert_ne!(atom_config, cpi_authority);
    }

    #[test]
    fn test_registry_config_layout_is_73_bytes() {
        // disc(8) + collection(32) + authority(32) + bump(1)
        let mut account = Vec::new();
        account.extend_from_slice(&REGISTRY_CONFIG_DISCRIMINATOR);
        account.extend_from_slice(&[7u8; 32]);
        account.extend_from_slice(&[9u8; 32]);
        account.push(255);
        assert_eq!(account.len(), 73);

        let config = RegistryConfig::deserialize(&mut &account[8..]).unwrap();
        assert_eq!(config.collection, [7u8; 32]);
        assert_eq!(config.authority, [9u8; 32]);
        assert_eq!(config.bump, 255);
    }

    #[test]
    fn test_root_config_layout_is_73_bytes() {
        let mut account = Vec::new();
        account.extend_from_slice(&ROOT_CONFIG_DISCRIMINATOR);
        account.extend_from_slice(&[3u8; 32]);
        account.extend_from_slice(&[4u8; 32]);
        account.push(254);
        assert_eq!(account.len(), 73);

        let root = RootConfig::deserialize(&mut &account[8..]).unwrap();
        assert_eq!(root.base_collection, [3u8; 32]);
        assert_eq!(root.authority, [4u8; 32]);
        assert_eq!(root.bump, 254);
    }

    /// Byte-for-byte replay of the live mainnet collection account header.
    #[test]
    fn test_collection_supply_parses_mpl_core_header() {
        let name = "8004 Agent Registry";
        let mut account = Vec::new();
        account.push(MPL_CORE_COLLECTION_V1_KEY);
        account.extend_from_slice(&[1u8; 32]);
        account.extend_from_slice(&(name.len() as u32).to_le_bytes());
        account.extend_from_slice(name.as_bytes());
        account.extend_from_slice(&0u32.to_le_bytes()); // empty uri
        account.extend_from_slice(&1465u32.to_le_bytes());
        account.extend_from_slice(&1391u32.to_le_bytes());
        assert_eq!(account.len(), 68);

        let supply = CollectionSupply::deserialize(&mut account.as_slice()).unwrap();
        assert_eq!(supply.name, name);
        assert_eq!(supply.num_minted, 1465);
        assert_eq!(supply.current_size, 1391);

        // Trailing plugin bytes must not break the header parse.
        account.extend_from_slice(&[0xAB; 16]);
        let supply = CollectionSupply::deserialize(&mut account.as_slice()).unwrap();
        assert_eq!(supply.current_size, 1391);
    }

    /// Replays the on-chain field order. The struct that shipped before this had the
    /// strings in the middle and eight fields missing, so `/identity/{net}/{agent}`
    /// answered 500 for every agent that existed.
    #[test]
    fn test_agent_account_layout() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[1u8; 32]); // collection
        payload.extend_from_slice(&[2u8; 32]); // creator
        payload.extend_from_slice(&[3u8; 32]); // owner
        payload.extend_from_slice(&[4u8; 32]); // asset
        payload.push(253); // bump
        payload.push(1); // atom_enabled
        payload.push(1); // agent_wallet: Some
        payload.extend_from_slice(&[5u8; 32]);
        payload.extend_from_slice(&[6u8; 32]); // feedback_digest
        payload.extend_from_slice(&7u64.to_le_bytes()); // feedback_count
        payload.extend_from_slice(&[8u8; 32]); // response_digest
        payload.extend_from_slice(&9u64.to_le_bytes()); // response_count
        payload.extend_from_slice(&[10u8; 32]); // revoke_digest
        payload.extend_from_slice(&11u64.to_le_bytes()); // revoke_count
        payload.push(0); // parent_asset: None
        payload.push(0); // parent_locked
        payload.push(0); // col_locked
        borsh_write_string(&mut payload, "https://example.com/agent.json");
        borsh_write_string(&mut payload, "Agent");
        borsh_write_string(&mut payload, "c1:bafy");

        let agent = AgentAccount::deserialize(&mut payload.as_slice()).unwrap();
        assert_eq!(agent.collection, [1u8; 32]);
        assert_eq!(agent.creator, [2u8; 32]);
        assert_eq!(agent.owner, [3u8; 32]);
        assert_eq!(agent.asset, [4u8; 32]);
        assert_eq!(agent.atom_enabled, 1);
        assert_eq!(agent.agent_wallet, Some([5u8; 32]));
        assert_eq!(agent.feedback_count, 7);
        assert_eq!(agent.response_count, 9);
        assert_eq!(agent.revoke_count, 11);
        assert_eq!(agent.parent_asset, None);
        assert_eq!(agent.agent_uri, "https://example.com/agent.json");
        assert_eq!(agent.nft_name, "Agent");
        assert_eq!(agent.col, "c1:bafy");

        // Accounts are allocated for max-length strings, so slack must not break the read.
        payload.extend_from_slice(&[0u8; 64]);
        let agent = AgentAccount::deserialize(&mut payload.as_slice()).unwrap();
        assert_eq!(agent.nft_name, "Agent");
    }

    /// The engine account is fixed-size: if our struct does not total exactly the
    /// on-chain 561 bytes, the fields at the tail (trust_tier, confidence, bump) are
    /// silently misread or the parse fails outright.
    #[test]
    fn test_atom_stats_is_561_bytes() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[1u8; 32]); // collection
        payload.extend_from_slice(&[2u8; 32]); // asset
        payload.extend_from_slice(&10u64.to_le_bytes()); // first_feedback_slot
        payload.extend_from_slice(&20u64.to_le_bytes()); // last_feedback_slot
        payload.extend_from_slice(&3u64.to_le_bytes()); // feedback_count
        for v in [11u16, 12, 13, 14, 15, 16, 17, 18] {
            payload.extend_from_slice(&v.to_le_bytes()); // ema/peak/drawdown/epochs
        }
        payload.extend_from_slice(&[40u8, 90, 50, 95]); // min/max/first/last score
        payload.extend_from_slice(&[0u8; 128]); // hll_packed
        payload.extend_from_slice(&99u64.to_le_bytes()); // hll_salt
        payload.extend_from_slice(&[0u8; 192]); // recent_callers
        payload.extend_from_slice(&[1u8, 2, 3, 4]); // burst/updates/neg/eviction
        payload.extend_from_slice(&77u64.to_le_bytes()); // ring_base_slot
        payload.extend_from_slice(&5u16.to_le_bytes()); // quality_velocity
        payload.extend_from_slice(&6u16.to_le_bytes()); // velocity_epoch
        payload.extend_from_slice(&[0u8, 30]); // freeze_epochs, quality_floor
        payload.extend_from_slice(&[0u8, 0]); // bypass_count, bypass_score_avg
        payload.extend_from_slice(&[0u8; 80]); // bypass_fingerprints
        payload.push(0); // bypass_fp_cursor
        payload.extend_from_slice(&700u16.to_le_bytes()); // loyalty_score
        payload.extend_from_slice(&850u16.to_le_bytes()); // quality_score
        payload.extend_from_slice(&[12u8, 64, 3]); // risk, diversity, trust_tier
        payload.extend_from_slice(&[3u8]); // tier_candidate
        payload.extend_from_slice(&8u16.to_le_bytes()); // tier_candidate_epoch
        payload.extend_from_slice(&[3u8, 0]); // tier_confirmed, flags
        payload.extend_from_slice(&88u16.to_le_bytes()); // confidence
        payload.extend_from_slice(&[254u8, 1]); // bump, schema_version

        // 561 on-chain minus the 8-byte Anchor discriminator.
        assert_eq!(payload.len(), 553);

        let stats = AtomStats::deserialize(&mut payload.as_slice()).unwrap();
        assert_eq!(stats.feedback_count, 3);
        assert_eq!(stats.last_feedback_slot, 20);
        assert_eq!(stats.min_score, 40);
        assert_eq!(stats.max_score, 90);
        assert_eq!(stats.last_score, 95);
        assert_eq!(stats.loyalty_score, 700);
        assert_eq!(stats.quality_score, 850);
        assert_eq!(stats.risk_score, 12);
        assert_eq!(stats.diversity_ratio, 64);
        assert_eq!(stats.trust_tier, 3);
        assert_eq!(stats.confidence, 88);
        assert_eq!(stats.bump, 254);
        assert_eq!(trust_tier_name(stats.trust_tier), "Trusted");
    }

    #[test]
    fn test_metadata_entry_layout() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[12u8; 32]); // asset
        payload.push(0); // immutable
        payload.push(255); // bump
        borsh_write_string(&mut payload, "x402Support");
        payload.extend_from_slice(&4u32.to_le_bytes()); // Vec<u8> len
        payload.extend_from_slice(b"true");

        let entry = MetadataEntryPda::deserialize(&mut payload.as_slice()).unwrap();
        assert_eq!(entry.asset, [12u8; 32]);
        assert_eq!(entry.immutable, 0);
        assert_eq!(entry.metadata_key, "x402Support");
        assert_eq!(entry.metadata_value, b"true".to_vec());
        assert_eq!(String::from_utf8(entry.metadata_value).unwrap(), "true");
    }

    #[test]
    fn test_metadata_pda() {
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let (pda1, _) = derive_metadata_pda(&asset, "x402Support", &AGENT_REGISTRY_MAINNET);
        let (pda2, _) = derive_metadata_pda(&asset, "protocol", &AGENT_REGISTRY_MAINNET);

        // Different keys should produce different PDAs
        assert_ne!(pda1, pda2);
    }

    /// The program widened the metadata key hash from 8 to 16 bytes in its v1.9
    /// security update; seed and payload must both carry 16.
    #[test]
    fn test_metadata_key_hash_is_16_bytes() {
        use sha2::{Digest, Sha256};
        let key = "x402Support";
        let hash = metadata_key_hash(key);
        assert_eq!(hash.len(), 16);
        assert_eq!(hash[..], Sha256::digest(key.as_bytes())[..16]);
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

    /// Build a RegistryContext from the verified mainnet addresses.
    fn mainnet_ctx() -> RegistryContext {
        let collection = Pubkey::from_str(MAINNET_COLLECTION).unwrap();
        let (root_config, _) = derive_root_config_pda(&AGENT_REGISTRY_MAINNET);
        let (registry_config, _) = derive_registry_config_pda(&AGENT_REGISTRY_MAINNET, &collection);
        RegistryContext {
            root_config,
            registry_config,
            collection,
            authority: Pubkey::new_unique(),
        }
    }

    #[test]
    fn test_give_feedback_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let collection = Pubkey::from_str(MAINNET_COLLECTION).unwrap();
        let client = Pubkey::new_unique();

        let ix = build_give_feedback_ix(
            &programs,
            &collection,
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
        // client, agent, asset, collection, system + the 4-account ATOM group
        assert_eq!(ix.accounts.len(), 9);
        assert_eq!(&ix.data[..8], &IX_GIVE_FEEDBACK);

        assert_eq!(ix.accounts[0].pubkey, client);
        assert!(ix.accounts[0].is_signer);
        assert_eq!(ix.accounts[3].pubkey, collection);

        let (atom_config, _) = derive_atom_config_pda(&ATOM_ENGINE_MAINNET);
        let (atom_stats, _) = derive_atom_stats_pda(&asset, &ATOM_ENGINE_MAINNET);
        let (cpi_authority, _) = derive_atom_cpi_authority_pda(&AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts[5].pubkey, atom_config);
        assert_eq!(ix.accounts[6].pubkey, atom_stats);
        assert!(ix.accounts[6].is_writable);
        assert_eq!(ix.accounts[7].pubkey, ATOM_ENGINE_MAINNET);
        assert_eq!(ix.accounts[8].pubkey, cpi_authority);
    }

    #[test]
    fn test_revoke_feedback_instruction() {
        let programs = get_program_ids(&Network::SolanaDevnet).unwrap();
        let asset = Pubkey::from_str("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv").unwrap();
        let client = Pubkey::new_unique();
        let seal_hash = [0xABu8; 32];

        let ix = build_revoke_feedback_ix(&programs, &asset, &client, 1, seal_hash);

        assert_eq!(ix.program_id, AGENT_REGISTRY_DEVNET);
        assert_eq!(ix.accounts.len(), 8);
        assert_eq!(&ix.data[..8], &IX_REVOKE_FEEDBACK);
        assert_eq!(ix.accounts[0].pubkey, client);
        assert!(ix.accounts[0].is_signer);

        let (cpi_authority, _) = derive_atom_cpi_authority_pda(&AGENT_REGISTRY_DEVNET);
        assert_eq!(ix.accounts[7].pubkey, cpi_authority);
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
        assert_eq!(ix.accounts.len(), 3);
        assert_eq!(&ix.data[..8], &IX_APPEND_RESPONSE);
        assert_eq!(ix.accounts[0].pubkey, responder);
        assert!(ix.accounts[0].is_signer);
        // The feedback author rides in the payload, right after the discriminator.
        assert_eq!(&ix.data[8..40], client.as_ref());
    }

    #[test]
    fn test_register_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let ctx = mainnet_ctx();
        let asset = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let ix = build_register_ix(&programs, &ctx, &asset, &owner, "ipfs://QmAgentSpec");

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 8);
        assert_eq!(&ix.data[..8], &IX_REGISTER);

        assert_eq!(ix.accounts[0].pubkey, ctx.root_config);
        assert_eq!(ix.accounts[1].pubkey, ctx.registry_config);
        assert!(ix.accounts[3].is_signer); // asset
        assert_eq!(ix.accounts[4].pubkey, ctx.collection);
        // mpl-core bumps num_minted/current_size, so the collection must be writable.
        assert!(ix.accounts[4].is_writable);
        assert!(ix.accounts[5].is_signer); // owner
    }

    #[test]
    fn test_set_metadata_pda_instruction() {
        let programs = get_program_ids(&Network::Solana).unwrap();
        let asset = Pubkey::new_unique();
        let owner = Pubkey::new_unique();

        let ix =
            build_set_metadata_pda_ix(&programs, &asset, &owner, "x402Support", b"true", false);

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(&ix.data[..8], &IX_SET_METADATA_PDA);

        let (metadata_pda, _) = derive_metadata_pda(&asset, "x402Support", &AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts[0].pubkey, metadata_pda);
        assert!(ix.accounts[3].is_signer); // owner

        // 16-byte key hash sits between the discriminator and the key string.
        assert_eq!(&ix.data[8..24], &metadata_key_hash("x402Support"));
        assert_eq!(
            &ix.data[24..28],
            &("x402Support".len() as u32).to_le_bytes()
        );
    }

    /// Vectors produced by `computeSealHash` in 8004-solana@0.8.3 (`dist/core/seal.js`),
    /// which the SDK documents as byte-identical to the on-chain routine. Pinning the
    /// SDK's own output means a drift in our implementation fails here rather than
    /// on-chain. Testing our function against itself is what let three entirely
    /// fabricated SHA-256 seal functions pass CI indefinitely.
    #[test]
    fn test_seal_hash_matches_sdk_vectors() {
        // No score, no file hash.
        let hash = compute_seal_hash(&SealParams {
            value: 95,
            value_decimals: 0,
            score: None,
            feedback_file_hash: None,
            tag1: "uptime",
            tag2: "verify",
            endpoint: "https://facilitator.ultravioletadao.xyz/verify",
            feedback_uri: "https://facilitator.ultravioletadao.xyz/.well-known/feedback.json",
        })
        .unwrap();
        assert_eq!(
            hex::encode(hash),
            "e8b95971b2423b4345835044f2f7f4b4573011374fa482c5f3905d2a79f74158"
        );

        // Score present, file hash present, negative i128, empty strings.
        let hash = compute_seal_hash(&SealParams {
            value: -12345,
            value_decimals: 6,
            score: Some(77),
            feedback_file_hash: Some([0xAB; 32]),
            tag1: "",
            tag2: "x",
            endpoint: "",
            feedback_uri: "ipfs://Qm",
        })
        .unwrap();
        assert_eq!(
            hex::encode(hash),
            "ce2aa6bd7761378846463a3f80455c9dd1fa602ece035262453fa66ae0b5284b"
        );
    }

    #[test]
    fn test_feedback_leaf_matches_sdk_vector() {
        let seal = hex::decode("e8b95971b2423b4345835044f2f7f4b4573011374fa482c5f3905d2a79f74158")
            .unwrap();
        let leaf = compute_feedback_leaf_v1(
            &Pubkey::from_str("DmhTrXVF9ikJHpNeAgxMNx8aMaP8J8jLhzFwGMZ9A5vZ").unwrap(),
            &Pubkey::from_str("6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv").unwrap(),
            3,
            &seal.try_into().unwrap(),
            987_654,
        );
        assert_eq!(
            hex::encode(leaf),
            "d52395913e4ce3384c2635e6add34c169948109d1878edb8164fea7be13768c6"
        );
    }

    /// The program validates these bounds before hashing, so a hash we would compute
    /// past them could never be accepted.
    #[test]
    fn test_seal_hash_rejects_out_of_bounds_input() {
        let base = SealParams {
            value: 1,
            value_decimals: 0,
            score: None,
            feedback_file_hash: None,
            tag1: "a",
            tag2: "b",
            endpoint: "e",
            feedback_uri: "u",
        };
        assert!(compute_seal_hash(&base).is_some());

        let long_tag = "x".repeat(MAX_TAG_LEN + 1);
        assert!(compute_seal_hash(&SealParams {
            tag1: &long_tag,
            ..base.clone()
        })
        .is_none());

        let long_uri = "x".repeat(MAX_URI_LEN + 1);
        assert!(compute_seal_hash(&SealParams {
            feedback_uri: &long_uri,
            ..base.clone()
        })
        .is_none());

        assert!(compute_seal_hash(&SealParams {
            value_decimals: 19,
            ..base.clone()
        })
        .is_none());

        assert!(compute_seal_hash(&SealParams {
            score: Some(101),
            ..base.clone()
        })
        .is_none());
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

        let ctx = mainnet_ctx();
        let ix = build_set_agent_uri_ix(&programs, &ctx, &asset, &owner, "ipfs://QmNewUri");

        assert_eq!(ix.program_id, AGENT_REGISTRY_MAINNET);
        assert_eq!(ix.accounts.len(), 7);
        assert_eq!(&ix.data[..8], &IX_SET_AGENT_URI);
        assert_eq!(ix.accounts[0].pubkey, ctx.registry_config);
        assert_eq!(ix.accounts[3].pubkey, ctx.collection);
        assert!(ix.accounts[4].is_signer); // owner
    }
}
