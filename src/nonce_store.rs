//! Nonce Store abstraction for replay protection.
//!
//! This module provides persistent storage for tracking used nonces to prevent
//! replay attacks on Stellar and Algorand chains. Unlike EVM which has on-chain
//! nonce tracking via EIP-3009, these chains require off-chain tracking.
//!
//! # Architecture
//!
//! ```text
//! StellarProvider / AlgorandProvider
//!        |
//!        v
//! NonceStore (trait) <-- DynamoNonceStore, MemoryNonceStore
//!        |
//!        v
//! DynamoDB (production) / HashMap (development)
//! ```
//!
//! # DynamoDB Schema
//!
//! Table: `facilitator-nonces` (configurable via NONCE_STORE_TABLE_NAME)
//!
//! | Attribute | Type | Description |
//! |-----------|------|-------------|
//! | pk | S | Partition key: `{chain}#{address}#{nonce}` or `{chain}#group#{group_id_hex}` |
//! | chain | S | Chain identifier (stellar, stellar-testnet, algorand, algorand-testnet) |
//! | created_at | N | Unix timestamp when the nonce was recorded |
//! | expires_at | N | TTL attribute - Unix timestamp for automatic deletion |
//!
//! # TTL Strategy
//!
//! - Stellar: TTL = signature_expiration_ledger * 5 seconds + 1 hour buffer
//! - Algorand: TTL = (last_valid_round - current_round) * 4 seconds + 1 hour buffer

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during nonce store operations.
#[derive(Debug, thiserror::Error)]
pub enum NonceStoreError {
    /// Nonce has already been used (replay attempt)
    #[error("Nonce already used: {0}")]
    NonceAlreadyUsed(String),

    /// Failed to connect to storage backend
    #[error("Storage connection failed: {0}")]
    ConnectionFailed(String),

    /// Failed to read from storage
    #[error("Read error: {0}")]
    ReadError(String),

    /// Failed to write to storage
    #[error("Write error: {0}")]
    WriteError(String),

    /// Storage not configured
    #[error("Storage not configured: {0}")]
    NotConfigured(String),
}

// ============================================================================
// Nonce Store Trait
// ============================================================================

/// Trait for persistent storage of used nonces.
///
/// Implementations must be thread-safe and provide atomic check-and-mark operations
/// to prevent race conditions in replay protection.
#[async_trait]
pub trait NonceStore: Send + Sync + std::fmt::Debug {
    /// Atomically check if a nonce is unused and mark it as used.
    ///
    /// This MUST be an atomic operation to prevent race conditions where two
    /// concurrent requests both pass the check before either marks the nonce.
    ///
    /// # Arguments
    ///
    /// * `key` - Unique identifier for the nonce (chain#address#nonce or chain#group#id)
    /// * `ttl_seconds` - Time-to-live in seconds for automatic cleanup
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Nonce was unused and is now marked as used
    /// * `Err(NonceAlreadyUsed)` - Nonce was already used (replay attempt)
    /// * `Err(...)` - Storage error
    async fn check_and_mark_used(&self, key: &str, ttl_seconds: u64) -> Result<(), NonceStoreError>;

    /// Check if a nonce has been used (read-only).
    ///
    /// Use this for verification without marking. For settlement, use check_and_mark_used().
    async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError>;

    /// Check if the store is healthy and accessible.
    async fn health_check(&self) -> Result<(), NonceStoreError>;

    /// Get the store type name for logging.
    fn store_type(&self) -> &'static str;
}

// ============================================================================
// Key Generation Helpers
// ============================================================================

/// Generate a nonce key for Stellar.
///
/// Format: `stellar#{address}#{nonce}` or `stellar-testnet#{address}#{nonce}`
pub fn stellar_nonce_key(chain: &str, address: &str, nonce: u64) -> String {
    format!("{}#{}#{}", chain, address, nonce)
}

/// Generate a nonce key for Algorand.
///
/// Format: `algorand#group#{group_id_hex}` or `algorand-testnet#group#{group_id_hex}`
pub fn algorand_nonce_key(chain: &str, group_id: &[u8; 32]) -> String {
    format!("{}#group#{}", chain, hex::encode(group_id))
}

/// Calculate TTL for Stellar nonces.
///
/// Based on ledger expiration: ~5 seconds per ledger + 1 hour buffer
pub fn stellar_ttl_seconds(current_ledger: u32, expiration_ledger: u32) -> u64 {
    let ledgers_until_expiry = expiration_ledger.saturating_sub(current_ledger);
    let seconds_until_expiry = (ledgers_until_expiry as u64) * 5;
    // Add 1 hour buffer for safety
    seconds_until_expiry + 3600
}

/// Calculate TTL for Algorand nonces.
///
/// Based on round validity: ~4 seconds per round + 1 hour buffer
pub fn algorand_ttl_seconds(current_round: u64, last_valid_round: u64) -> u64 {
    let rounds_until_expiry = last_valid_round.saturating_sub(current_round);
    let seconds_until_expiry = rounds_until_expiry * 4;
    // Add 1 hour buffer for safety
    seconds_until_expiry + 3600
}

// ============================================================================
// In-Memory Store (for development/testing)
// ============================================================================

/// In-memory nonce store for development and testing.
///
/// Does not persist data across restarts. Not suitable for production
/// as it allows replay attacks after facilitator restart.
#[derive(Debug, Default)]
pub struct MemoryNonceStore {
    data: Arc<RwLock<HashMap<String, u64>>>, // key -> expires_at timestamp
}

impl MemoryNonceStore {
    /// Create a new empty in-memory nonce store.
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[async_trait]
impl NonceStore for MemoryNonceStore {
    async fn check_and_mark_used(&self, key: &str, ttl_seconds: u64) -> Result<(), NonceStoreError> {
        let now = Self::current_timestamp();
        let mut data = self.data.write().await;

        // Check if key exists and hasn't expired
        if let Some(&expires_at) = data.get(key) {
            if expires_at > now {
                return Err(NonceStoreError::NonceAlreadyUsed(key.to_string()));
            }
            // Expired entry, remove it
            data.remove(key);
        }

        // Mark as used
        let expires_at = now + ttl_seconds;
        data.insert(key.to_string(), expires_at);
        debug!(key = %key, ttl_seconds = %ttl_seconds, "Marked nonce as used (memory)");
        Ok(())
    }

    async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError> {
        let now = Self::current_timestamp();
        let data = self.data.read().await;

        if let Some(&expires_at) = data.get(key) {
            return Ok(expires_at > now);
        }
        Ok(false)
    }

    async fn health_check(&self) -> Result<(), NonceStoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "memory"
    }
}

// ============================================================================
// DynamoDB Store
// ============================================================================

/// DynamoDB-based persistent nonce store for production.
///
/// Uses conditional PutItem for atomic check-and-mark operations.
/// TTL attribute enables automatic cleanup of expired nonces.
///
/// # Configuration
///
/// Environment variables:
/// - `NONCE_STORE_TABLE_NAME`: DynamoDB table name (default: "facilitator-nonces")
/// - `AWS_REGION`: AWS region (uses default from environment)
#[derive(Debug)]
pub struct DynamoNonceStore {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
}

impl DynamoNonceStore {
    /// Create a new DynamoDB nonce store.
    pub fn new(client: aws_sdk_dynamodb::Client, table_name: String) -> Self {
        info!(table_name = %table_name, "Initialized DynamoDB nonce store");
        Self { client, table_name }
    }

    /// Create a new DynamoDB nonce store from environment variables.
    pub async fn from_env() -> Result<Self, NonceStoreError> {
        let table_name = std::env::var("NONCE_STORE_TABLE_NAME")
            .unwrap_or_else(|_| "facilitator-nonces".to_string());

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_dynamodb::Client::new(&config);

        Ok(Self::new(client, table_name))
    }

    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[async_trait]
impl NonceStore for DynamoNonceStore {
    async fn check_and_mark_used(&self, key: &str, ttl_seconds: u64) -> Result<(), NonceStoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let now = Self::current_timestamp();
        let expires_at = now + ttl_seconds;

        // Extract chain from key (format: chain#...)
        let chain = key.split('#').next().unwrap_or("unknown");

        // Atomic conditional put - fails if key already exists and hasn't expired
        let result = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(key.to_string()))
            .item("chain", AttributeValue::S(chain.to_string()))
            .item("created_at", AttributeValue::N(now.to_string()))
            .item("expires_at", AttributeValue::N(expires_at.to_string()))
            // Condition: item doesn't exist OR has expired
            .condition_expression(
                "attribute_not_exists(pk) OR expires_at < :now"
            )
            .expression_attribute_values(":now", AttributeValue::N(now.to_string()))
            .send()
            .await;

        match result {
            Ok(_) => {
                debug!(
                    key = %key,
                    ttl_seconds = %ttl_seconds,
                    expires_at = %expires_at,
                    "Marked nonce as used (DynamoDB)"
                );
                Ok(())
            }
            Err(err) => {
                let service_err = err.into_service_error();
                // Check if it's a conditional check failure (nonce already used)
                if service_err.is_conditional_check_failed_exception() {
                    warn!(key = %key, "Replay attempt detected - nonce already used");
                    return Err(NonceStoreError::NonceAlreadyUsed(key.to_string()));
                }
                error!(error = %service_err, key = %key, "DynamoDB put_item failed");
                Err(NonceStoreError::WriteError(service_err.to_string()))
            }
        }
    }

    async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let now = Self::current_timestamp();

        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(key.to_string()))
            .projection_expression("expires_at")
            .send()
            .await
            .map_err(|e| NonceStoreError::ReadError(e.to_string()))?;

        if let Some(item) = result.item {
            if let Some(AttributeValue::N(expires_at_str)) = item.get("expires_at") {
                if let Ok(expires_at) = expires_at_str.parse::<u64>() {
                    return Ok(expires_at > now);
                }
            }
        }

        Ok(false)
    }

    async fn health_check(&self) -> Result<(), NonceStoreError> {
        // Try to describe the table to verify connectivity
        self.client
            .describe_table()
            .table_name(&self.table_name)
            .send()
            .await
            .map_err(|e| NonceStoreError::ConnectionFailed(e.to_string()))?;
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "dynamodb"
    }
}

// ============================================================================
// Factory Function
// ============================================================================

/// Create the appropriate nonce store based on configuration.
///
/// - If `NONCE_STORE_TABLE_NAME` is set, uses DynamoDB
/// - Otherwise, falls back to in-memory store (with warning)
pub async fn create_nonce_store() -> Arc<dyn NonceStore> {
    match std::env::var("NONCE_STORE_TABLE_NAME") {
        Ok(table_name) if !table_name.is_empty() => {
            match DynamoNonceStore::from_env().await {
                Ok(store) => {
                    info!(
                        table_name = %table_name,
                        "Using DynamoDB nonce store for replay protection"
                    );
                    Arc::new(store)
                }
                Err(e) => {
                    error!(error = %e, "Failed to initialize DynamoDB nonce store, falling back to memory");
                    warn!("WARNING: In-memory nonce store does not survive restarts - replay attacks possible!");
                    Arc::new(MemoryNonceStore::new())
                }
            }
        }
        _ => {
            warn!("NONCE_STORE_TABLE_NAME not set - using in-memory nonce store");
            warn!("WARNING: In-memory nonce store does not survive restarts - replay attacks possible!");
            Arc::new(MemoryNonceStore::new())
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_store_check_and_mark() {
        let store = MemoryNonceStore::new();
        let key = "stellar#GABC123#12345";

        // First use should succeed
        assert!(store.check_and_mark_used(key, 3600).await.is_ok());

        // Second use should fail (replay)
        let result = store.check_and_mark_used(key, 3600).await;
        assert!(matches!(result, Err(NonceStoreError::NonceAlreadyUsed(_))));
    }

    #[tokio::test]
    async fn test_memory_store_is_used() {
        let store = MemoryNonceStore::new();
        let key = "algorand#group#abcd1234";

        // Not used initially
        assert!(!store.is_used(key).await.unwrap());

        // Mark as used
        store.check_and_mark_used(key, 3600).await.unwrap();

        // Now it's used
        assert!(store.is_used(key).await.unwrap());
    }

    #[test]
    fn test_stellar_nonce_key() {
        let key = stellar_nonce_key("stellar", "GABC123", 12345);
        assert_eq!(key, "stellar#GABC123#12345");
    }

    #[test]
    fn test_algorand_nonce_key() {
        let group_id = [0xab; 32];
        let key = algorand_nonce_key("algorand", &group_id);
        assert!(key.starts_with("algorand#group#"));
        assert!(key.ends_with(&hex::encode([0xab; 32])));
    }

    #[test]
    fn test_stellar_ttl_seconds() {
        // 100 ledgers until expiry = 500 seconds + 3600 buffer = 4100
        let ttl = stellar_ttl_seconds(1000, 1100);
        assert_eq!(ttl, 4100);
    }

    #[test]
    fn test_algorand_ttl_seconds() {
        // 100 rounds until expiry = 400 seconds + 3600 buffer = 4000
        let ttl = algorand_ttl_seconds(1000, 1100);
        assert_eq!(ttl, 4000);
    }
}
