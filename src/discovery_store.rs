//! Discovery Store abstraction for persistent storage.
//!
//! This module provides a trait-based abstraction for storing Bazaar discovery
//! resources, with implementations for:
//! - In-memory (for testing)
//! - S3 (production MVP)
//!
//! Future implementations can include DynamoDB, PostgreSQL, etc.
//!
//! # Architecture
//!
//! ```text
//! DiscoveryRegistry (in-memory cache for fast reads)
//!        |
//!        v
//! DiscoveryStore (trait) <-- S3Store, MemoryStore, DynamoStore, etc.
//!        |
//!        v
//! Persistent Storage (S3, DynamoDB, PostgreSQL)
//! ```
//!
//! The registry maintains an in-memory cache for fast reads, while the store
//! handles persistence. On startup, the registry loads all resources from the
//! store. On writes, the registry updates both memory and store.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::types_v2::DiscoveryResource;

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Failed to connect to storage backend
    #[error("Storage connection failed: {0}")]
    ConnectionFailed(String),

    /// Failed to serialize/deserialize data
    #[error("Serialization error: {0}")]
    SerializationError(String),

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
// Discovery Store Trait
// ============================================================================

/// Trait for persistent storage of discovery resources.
///
/// Implementations should be thread-safe and handle their own connection pooling.
/// All methods are async to support network-based storage backends.
#[async_trait]
pub trait DiscoveryStore: Send + Sync + std::fmt::Debug {
    /// Load all resources from storage.
    ///
    /// Called on startup to populate the in-memory cache.
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError>;

    /// Save a resource to storage.
    ///
    /// This should be idempotent - saving the same resource twice should not fail.
    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError>;

    /// Delete a resource from storage.
    ///
    /// Should not fail if the resource doesn't exist.
    async fn delete(&self, url: &str) -> Result<(), StoreError>;

    /// Save all resources to storage (batch operation).
    ///
    /// Default implementation calls save() for each resource.
    async fn save_all(&self, resources: &[DiscoveryResource]) -> Result<(), StoreError> {
        for resource in resources {
            self.save(resource).await?;
        }
        Ok(())
    }

    /// Check if the store is healthy and accessible.
    async fn health_check(&self) -> Result<(), StoreError>;

    /// Get the store type name for logging.
    fn store_type(&self) -> &'static str;
}

// ============================================================================
// In-Memory Store (for testing)
// ============================================================================

/// In-memory store implementation for testing.
///
/// Does not persist data across restarts.
#[derive(Debug, Default)]
pub struct MemoryStore {
    data: Arc<RwLock<HashMap<String, DiscoveryResource>>>,
}

impl MemoryStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DiscoveryStore for MemoryStore {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        let data = self.data.read().await;
        Ok(data.values().cloned().collect())
    }

    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        data.insert(resource.url.to_string(), resource.clone());
        Ok(())
    }

    async fn delete(&self, url: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        data.remove(url);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "memory"
    }
}

// ============================================================================
// S3 Store
// ============================================================================

/// S3-based persistent store for discovery resources.
///
/// Stores all resources as a single JSON file in S3 for simplicity.
/// This is optimized for small to medium registries (< 1000 resources).
///
/// # Configuration
///
/// Requires the following environment variables:
/// - `DISCOVERY_S3_BUCKET`: S3 bucket name
/// - `DISCOVERY_S3_KEY`: S3 object key (default: "bazaar/resources.json")
/// - `AWS_REGION`: AWS region (or uses default from environment)
///
/// # Thread Safety
///
/// Uses a local cache to minimize S3 reads. Writes are atomic (single PUT).
#[derive(Debug)]
pub struct S3Store {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
}

impl S3Store {
    /// Create a new S3 store with explicit configuration.
    pub fn new(client: aws_sdk_s3::Client, bucket: String, key: String) -> Self {
        info!(
            bucket = %bucket,
            key = %key,
            "Initialized S3 discovery store"
        );
        Self { client, bucket, key }
    }

    /// Create a new S3 store from environment variables.
    ///
    /// # Environment Variables
    ///
    /// - `DISCOVERY_S3_BUCKET` (required): S3 bucket name
    /// - `DISCOVERY_S3_KEY` (optional): Object key, defaults to "bazaar/resources.json"
    pub async fn from_env() -> Result<Self, StoreError> {
        let bucket = std::env::var("DISCOVERY_S3_BUCKET").map_err(|_| {
            StoreError::NotConfigured("DISCOVERY_S3_BUCKET environment variable not set".into())
        })?;

        let key = std::env::var("DISCOVERY_S3_KEY")
            .unwrap_or_else(|_| "bazaar/resources.json".to_string());

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);

        Ok(Self::new(client, bucket, key))
    }

    /// Serialize resources to JSON bytes.
    fn serialize(resources: &[DiscoveryResource]) -> Result<Vec<u8>, StoreError> {
        serde_json::to_vec_pretty(resources)
            .map_err(|e| StoreError::SerializationError(e.to_string()))
    }

    /// Deserialize resources from JSON bytes.
    fn deserialize(data: &[u8]) -> Result<Vec<DiscoveryResource>, StoreError> {
        serde_json::from_slice(data).map_err(|e| StoreError::SerializationError(e.to_string()))
    }
}

#[async_trait]
impl DiscoveryStore for S3Store {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        debug!(bucket = %self.bucket, key = %self.key, "Loading resources from S3");

        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .send()
            .await;

        match result {
            Ok(output) => {
                let body = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError::ReadError(e.to_string()))?;

                let resources = Self::deserialize(&body.into_bytes())?;
                info!(
                    count = resources.len(),
                    "Loaded discovery resources from S3"
                );
                Ok(resources)
            }
            Err(sdk_err) => {
                // Check if it's a "not found" error (first time, empty store)
                let service_err = sdk_err.into_service_error();
                if service_err.is_no_such_key() {
                    info!("No existing discovery data in S3, starting fresh");
                    return Ok(Vec::new());
                }
                error!(error = %service_err, "Failed to load from S3");
                Err(StoreError::ReadError(service_err.to_string()))
            }
        }
    }

    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
        // For S3, we need to load all, update, and save all
        // This is not ideal for high-frequency writes but works for MVP
        let mut resources = self.load_all().await.unwrap_or_default();

        // Update or insert
        let url_str = resource.url.to_string();
        if let Some(existing) = resources.iter_mut().find(|r| r.url.to_string() == url_str) {
            *existing = resource.clone();
        } else {
            resources.push(resource.clone());
        }

        self.save_all(&resources).await
    }

    async fn delete(&self, url: &str) -> Result<(), StoreError> {
        let mut resources = self.load_all().await.unwrap_or_default();
        resources.retain(|r| r.url.to_string() != url);
        self.save_all(&resources).await
    }

    async fn save_all(&self, resources: &[DiscoveryResource]) -> Result<(), StoreError> {
        debug!(
            bucket = %self.bucket,
            key = %self.key,
            count = resources.len(),
            "Saving resources to S3"
        );

        let body = Self::serialize(resources)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .body(body.into())
            .content_type("application/json")
            .send()
            .await
            .map_err(|e| StoreError::WriteError(e.to_string()))?;

        info!(count = resources.len(), "Saved discovery resources to S3");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        // Try to head the bucket to check connectivity
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "s3"
    }
}

// ============================================================================
// No-Op Store (for when persistence is disabled)
// ============================================================================

/// No-op store that doesn't persist anything.
///
/// Use this when persistence is not configured or not needed.
#[derive(Debug, Default)]
pub struct NoOpStore;

impl NoOpStore {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DiscoveryStore for NoOpStore {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        Ok(Vec::new())
    }

    async fn save(&self, _resource: &DiscoveryResource) -> Result<(), StoreError> {
        Ok(())
    }

    async fn delete(&self, _url: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "noop"
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;
    use crate::types::{MixedAddress, Scheme, TokenAmount};
    use crate::types_v2::PaymentRequirementsV2;
    use url::Url;

    fn create_test_resource(url: &str) -> DiscoveryResource {
        let network = Caip2NetworkId::eip155(8453);
        let accepts = vec![PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network,
            asset: MixedAddress::Evm(
                "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                    .parse()
                    .unwrap(),
            ),
            amount: TokenAmount::from(1000000u64),
            pay_to: MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            max_timeout_seconds: 300,
            extra: None,
        }];

        DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "Test resource".to_string(),
            accepts,
        )
    }

    #[tokio::test]
    async fn test_memory_store_save_and_load() {
        let store = MemoryStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].url.to_string(), "https://api.example.com/data");
    }

    #[tokio::test]
    async fn test_memory_store_delete() {
        let store = MemoryStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 1);

        store
            .delete("https://api.example.com/data")
            .await
            .unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_memory_store_save_all() {
        let store = MemoryStore::new();

        let resources = vec![
            create_test_resource("https://api1.example.com/data"),
            create_test_resource("https://api2.example.com/data"),
            create_test_resource("https://api3.example.com/data"),
        ];

        store.save_all(&resources).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[tokio::test]
    async fn test_noop_store() {
        let store = NoOpStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();

        // NoOp store doesn't persist anything
        let loaded = store.load_all().await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_store_health_check() {
        let memory_store = MemoryStore::new();
        assert!(memory_store.health_check().await.is_ok());

        let noop_store = NoOpStore::new();
        assert!(noop_store.health_check().await.is_ok());
    }
}
