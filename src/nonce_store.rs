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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
    async fn check_and_mark_used(&self, key: &str, ttl_seconds: u64)
        -> Result<(), NonceStoreError>;

    /// Check if a nonce has been used (read-only).
    ///
    /// Use this for verification without marking. For settlement, use check_and_mark_used().
    async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError>;

    /// Give a claimed key back, best-effort.
    ///
    /// For flows that must claim BEFORE doing the irreversible thing (the only
    /// way to avoid a check-then-act race) and then discover the irreversible
    /// thing did not happen. Without this, a failed on-chain write would burn
    /// the claim and the caller could never retry.
    ///
    /// Best-effort by design: a release that fails leaves the key claimed,
    /// which is the safe direction -- it costs a retry, never a double spend.
    /// The default is a no-op so an implementation that cannot delete simply
    /// keeps the stricter behaviour.
    async fn release(&self, key: &str) -> Result<(), NonceStoreError> {
        let _ = key;
        Ok(())
    }

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
    async fn check_and_mark_used(
        &self,
        key: &str,
        ttl_seconds: u64,
    ) -> Result<(), NonceStoreError> {
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

    async fn release(&self, key: &str) -> Result<(), NonceStoreError> {
        self.data.write().await.remove(key);
        debug!(key = %key, "Released nonce claim (memory)");
        Ok(())
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

/// Environment variable overriding [`DEFAULT_OPERATION_TIMEOUT_MS`].
const ENV_OPERATION_TIMEOUT_MS: &str = "NONCE_STORE_OPERATION_TIMEOUT_MS";

/// Bound on one nonce store call, retries included.
///
/// The SDK sets none, so a DynamoDB that takes the connection and never
/// answers holds a verify or a settle for as long as its caller waits. Every
/// caller already treats a store error as a rejection, so the bound only turns
/// that hang into the same rejection sooner. In-region reads and conditional
/// puts answer in milliseconds; the rest is room for the SDK's own retries.
const DEFAULT_OPERATION_TIMEOUT_MS: u64 = 3_000;

/// Accepted range for the override. Below the floor, ordinary jitter would
/// start rejecting payments; above the ceiling the bound outlasts the RPC
/// timeouts on the same paths and stops meaning anything.
const MIN_OPERATION_TIMEOUT_MS: u64 = 250;
const MAX_OPERATION_TIMEOUT_MS: u64 = 30_000;

/// Read the operation timeout from the environment, falling back to the default.
///
/// A bad value warns and falls back; it never panics, and the range is checked
/// as well as the type.
fn operation_timeout_from_env() -> Duration {
    let raw = match std::env::var(ENV_OPERATION_TIMEOUT_MS) {
        Ok(v) => v,
        Err(_) => return Duration::from_millis(DEFAULT_OPERATION_TIMEOUT_MS),
    };

    let millis = match raw.trim().parse::<u64>() {
        Ok(ms) if (MIN_OPERATION_TIMEOUT_MS..=MAX_OPERATION_TIMEOUT_MS).contains(&ms) => ms,
        Ok(ms) => {
            warn!(
                value = ms,
                min = MIN_OPERATION_TIMEOUT_MS,
                max = MAX_OPERATION_TIMEOUT_MS,
                default = DEFAULT_OPERATION_TIMEOUT_MS,
                "{ENV_OPERATION_TIMEOUT_MS} out of range, using default"
            );
            DEFAULT_OPERATION_TIMEOUT_MS
        }
        Err(_) => {
            warn!(
                value = %raw,
                default = DEFAULT_OPERATION_TIMEOUT_MS,
                "{ENV_OPERATION_TIMEOUT_MS} is not a number, using default"
            );
            DEFAULT_OPERATION_TIMEOUT_MS
        }
    };
    Duration::from_millis(millis)
}

/// A DynamoDB client whose every operation is bounded by `operation_timeout`.
///
/// `ambient` is the timeout config the loaded AWS config already carries (the
/// connect timeout, for one); it is kept, and only the operation timeout is set.
fn bounded_client(
    builder: aws_sdk_dynamodb::config::Builder,
    ambient: Option<&aws_sdk_dynamodb::config::timeout::TimeoutConfig>,
    operation_timeout: Duration,
) -> aws_sdk_dynamodb::Client {
    use aws_sdk_dynamodb::config::timeout::TimeoutConfig;

    let timeouts = ambient
        .map(TimeoutConfig::to_builder)
        .unwrap_or_else(TimeoutConfig::builder)
        .operation_timeout(operation_timeout)
        .build();
    aws_sdk_dynamodb::Client::from_conf(builder.timeout_config(timeouts).build())
}

/// DynamoDB-based persistent nonce store for production.
///
/// Uses conditional PutItem for atomic check-and-mark operations.
/// TTL attribute enables automatic cleanup of expired nonces.
///
/// # Configuration
///
/// Environment variables:
/// - `NONCE_STORE_TABLE_NAME`: DynamoDB table name (default: "facilitator-nonces")
/// - `NONCE_STORE_OPERATION_TIMEOUT_MS`: bound on each call, retries included
///   (default 3000, accepted 250-30000)
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
        let operation_timeout = operation_timeout_from_env();
        let client = bounded_client(
            aws_sdk_dynamodb::config::Builder::from(&config),
            config.timeout_config(),
            operation_timeout,
        );
        info!(
            operation_timeout_ms = operation_timeout.as_millis() as u64,
            "DynamoDB nonce store operation timeout"
        );

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
    async fn check_and_mark_used(
        &self,
        key: &str,
        ttl_seconds: u64,
    ) -> Result<(), NonceStoreError> {
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
            .condition_expression("attribute_not_exists(pk) OR expires_at < :now")
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
            // A claim written just before must be visible here; an eventually
            // consistent read can miss it.
            .consistent_read(true)
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

    async fn release(&self, key: &str) -> Result<(), NonceStoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(key.to_string()))
            .send()
            .await
            .map_err(|e| NonceStoreError::WriteError(e.into_service_error().to_string()))?;
        debug!(key = %key, "Released nonce claim (DynamoDB)");
        Ok(())
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
        Ok(table_name) if !table_name.is_empty() => match DynamoNonceStore::from_env().await {
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
        },
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

    /// A claim that is released can be claimed again.
    ///
    /// This is what lets a caller retry after the on-chain write it claimed for
    /// never landed -- without it, one failed submission would burn that
    /// payment's right to rate forever.
    #[tokio::test]
    async fn test_memory_store_release_allows_a_retry() {
        let store = MemoryNonceStore::new();
        let key = "erc8004-proof#base#abc#42";

        store.check_and_mark_used(key, 3600).await.unwrap();
        assert!(store.check_and_mark_used(key, 3600).await.is_err());

        store.release(key).await.unwrap();
        assert!(!store.is_used(key).await.unwrap());
        assert!(store.check_and_mark_used(key, 3600).await.is_ok());
    }

    /// Releasing something never claimed is not an error: the caller releases
    /// on a failure path where it cannot know whether the claim went through.
    #[tokio::test]
    async fn test_memory_store_release_is_idempotent() {
        let store = MemoryNonceStore::new();
        assert!(store.release("never#claimed#key").await.is_ok());
        assert!(store.release("never#claimed#key").await.is_ok());
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

    // ------------------------------------------------------------------------
    // DynamoDB client: consistent reads and the operation timeout
    // ------------------------------------------------------------------------

    /// Run `f` with `ENV_OPERATION_TIMEOUT_MS` set to `value` (or unset for
    /// `None`), then restore whatever was there. CI runs this suite with
    /// `--test-threads=1`, which is what makes touching process env safe here.
    fn with_timeout_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var(ENV_OPERATION_TIMEOUT_MS).ok();
        match value {
            Some(v) => std::env::set_var(ENV_OPERATION_TIMEOUT_MS, v),
            None => std::env::remove_var(ENV_OPERATION_TIMEOUT_MS),
        }
        let out = f();
        match previous {
            Some(v) => std::env::set_var(ENV_OPERATION_TIMEOUT_MS, v),
            None => std::env::remove_var(ENV_OPERATION_TIMEOUT_MS),
        }
        out
    }

    #[test]
    fn operation_timeout_defaults_and_reads_the_override() {
        let default = Duration::from_millis(DEFAULT_OPERATION_TIMEOUT_MS);
        assert_eq!(with_timeout_env(None, operation_timeout_from_env), default);
        assert_eq!(
            with_timeout_env(Some("1500"), operation_timeout_from_env),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            with_timeout_env(Some(" 800 "), operation_timeout_from_env),
            Duration::from_millis(800)
        );
        // The bounds themselves are accepted.
        assert_eq!(
            with_timeout_env(Some("250"), operation_timeout_from_env),
            Duration::from_millis(MIN_OPERATION_TIMEOUT_MS)
        );
        assert_eq!(
            with_timeout_env(Some("30000"), operation_timeout_from_env),
            Duration::from_millis(MAX_OPERATION_TIMEOUT_MS)
        );
    }

    #[test]
    fn operation_timeout_rejects_garbage_and_out_of_range() {
        let default = Duration::from_millis(DEFAULT_OPERATION_TIMEOUT_MS);
        for bad in ["not-a-number", "", "-5", "3s", "0", "249", "30001"] {
            assert_eq!(
                with_timeout_env(Some(bad), operation_timeout_from_env),
                default,
                "{bad:?} should fall back to the default"
            );
        }
    }

    #[test]
    fn bounded_client_keeps_the_ambient_timeouts() {
        use aws_sdk_dynamodb::config::timeout::TimeoutConfig;
        use aws_sdk_dynamodb::config::{BehaviorVersion, Builder, Region};

        let ambient = TimeoutConfig::builder()
            .connect_timeout(Duration::from_millis(3_100))
            .build();
        let client = bounded_client(
            Builder::new()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-2")),
            Some(&ambient),
            Duration::from_millis(1_234),
        );

        let timeouts = client.config().timeout_config().expect("timeouts are set");
        assert_eq!(
            timeouts.operation_timeout(),
            Some(Duration::from_millis(1_234))
        );
        assert_eq!(
            timeouts.connect_timeout(),
            Some(Duration::from_millis(3_100))
        );
    }

    /// A DynamoDB endpoint on localhost. Records every request body and answers
    /// `{}` (no item), or, with `hang`, never answers.
    async fn dynamo_stub(
        hang: bool,
        seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) -> String {
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |body: axum::body::Bytes| {
                let seen = seen.clone();
                async move {
                    seen.lock()
                        .unwrap()
                        .push(serde_json::from_slice(&body).unwrap_or_default());
                    if hang {
                        tokio::time::sleep(Duration::from_secs(3_600)).await;
                    }
                    (
                        [(
                            axum::http::header::CONTENT_TYPE,
                            "application/x-amz-json-1.0",
                        )],
                        "{}",
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn stub_store(endpoint: String, operation_timeout: Duration) -> DynamoNonceStore {
        use aws_sdk_dynamodb::config::{BehaviorVersion, Builder, Credentials, Region};

        let builder = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new("us-east-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"));
        DynamoNonceStore::new(
            bounded_client(builder, None, operation_timeout),
            "facilitator-nonces".to_string(),
        )
    }

    #[tokio::test]
    async fn dynamo_is_used_reads_consistently() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let store = stub_store(
            dynamo_stub(false, seen.clone()).await,
            Duration::from_secs(5),
        );

        assert!(!store.is_used("stellar#GABC123#12345").await.unwrap());

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1, "got {requests:?}");
        assert_eq!(requests[0]["Key"]["pk"]["S"], "stellar#GABC123#12345");
        assert_eq!(
            requests[0]["ConsistentRead"], true,
            "is_used must read consistently, got {}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn dynamo_calls_give_up_at_the_operation_timeout() {
        let store = stub_store(
            dynamo_stub(true, Default::default()).await,
            Duration::from_millis(200),
        );

        let read = tokio::time::timeout(
            Duration::from_secs(5),
            store.is_used("stellar#GABC123#12345"),
        )
        .await
        .expect("the read must return on its own timeout, not hang");
        assert!(
            matches!(read, Err(NonceStoreError::ReadError(_))),
            "got {read:?}"
        );

        let claim = tokio::time::timeout(
            Duration::from_secs(5),
            store.check_and_mark_used("stellar#GABC123#12346", 60),
        )
        .await
        .expect("the conditional put must return on its own timeout, not hang");
        assert!(
            matches!(claim, Err(NonceStoreError::WriteError(_))),
            "got {claim:?}"
        );
    }
}
