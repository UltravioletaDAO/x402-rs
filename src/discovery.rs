//! Bazaar Discovery Registry for x402 v2.
//!
//! This module implements an in-memory registry for discoverable paid API endpoints.
//! Resource providers can register their endpoints, and clients can query the registry
//! to find available paid services.
//!
//! # Resource Types
//!
//! The registry supports the following resource types:
//! - `http`: Standard HTTP API endpoints that accept x402 payments
//! - `mcp`: Model Context Protocol endpoints
//! - `a2a`: Agent-to-Agent protocol endpoints
//! - `facilitator`: x402 payment facilitator services (do not require payments themselves)
//!
//! # Architecture
//!
//! The registry uses `Arc<RwLock<HashMap>>` for thread-safe concurrent access:
//! - Reads (discovery queries) can happen concurrently
//! - Writes (registration) acquire exclusive lock briefly
//!
//! # Example
//!
//! ```rust,ignore
//! use x402_rs::discovery::DiscoveryRegistry;
//! use x402_rs::types_v2::{DiscoveryResource, RegisterResourceRequest};
//!
//! let registry = DiscoveryRegistry::new();
//!
//! // Register a resource
//! registry.register(resource).await?;
//!
//! // Query resources
//! let response = registry.list(10, 0, None).await;
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::types_v2::{
    DiscoveryFilters, DiscoveryResource, DiscoveryResponse, Pagination,
};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during discovery operations.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Resource with this URL already exists
    #[error("Resource already registered: {0}")]
    AlreadyExists(String),

    /// Resource not found
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Invalid URL format
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Invalid resource type
    #[error("Invalid resource type: {0}. Expected: http, mcp, or a2a")]
    InvalidResourceType(String),

    /// No payment methods specified
    #[error("At least one payment method must be specified in 'accepts'")]
    NoPaymentMethods,
}

// ============================================================================
// Discovery Registry
// ============================================================================

/// In-memory registry for discoverable paid resources.
///
/// Thread-safe using `Arc<RwLock>` for concurrent read access with
/// exclusive write access during registration.
#[derive(Debug, Clone)]
pub struct DiscoveryRegistry {
    /// Map of URL -> DiscoveryResource
    resources: Arc<RwLock<HashMap<String, DiscoveryResource>>>,
}

impl Default for DiscoveryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryRegistry {
    /// Create a new empty discovery registry.
    pub fn new() -> Self {
        info!("Initializing Bazaar discovery registry");
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new resource in the registry.
    ///
    /// # Errors
    ///
    /// Returns `DiscoveryError::AlreadyExists` if a resource with the same URL
    /// is already registered. Use `update` to modify existing resources.
    pub async fn register(&self, resource: DiscoveryResource) -> Result<(), DiscoveryError> {
        // Validate resource
        self.validate_resource(&resource)?;

        let url_key = resource.url.to_string();

        let mut resources = self.resources.write().await;

        if resources.contains_key(&url_key) {
            warn!(url = %url_key, "Attempted to register duplicate resource");
            return Err(DiscoveryError::AlreadyExists(url_key));
        }

        info!(
            url = %url_key,
            resource_type = %resource.resource_type,
            accepts_count = resource.accepts.len(),
            "Registered new resource in discovery registry"
        );

        resources.insert(url_key, resource);
        Ok(())
    }

    /// Update an existing resource in the registry.
    ///
    /// If the resource doesn't exist, it will be created (upsert behavior).
    pub async fn update(&self, resource: DiscoveryResource) -> Result<(), DiscoveryError> {
        self.validate_resource(&resource)?;

        let url_key = resource.url.to_string();

        let mut resources = self.resources.write().await;
        let existed = resources.contains_key(&url_key);

        resources.insert(url_key.clone(), resource);

        if existed {
            debug!(url = %url_key, "Updated existing resource in registry");
        } else {
            info!(url = %url_key, "Created new resource via update (upsert)");
        }

        Ok(())
    }

    /// Remove a resource from the registry.
    ///
    /// # Errors
    ///
    /// Returns `DiscoveryError::NotFound` if no resource with the given URL exists.
    pub async fn unregister(&self, url: &str) -> Result<DiscoveryResource, DiscoveryError> {
        let mut resources = self.resources.write().await;

        match resources.remove(url) {
            Some(resource) => {
                info!(url = %url, "Unregistered resource from discovery registry");
                Ok(resource)
            }
            None => {
                warn!(url = %url, "Attempted to unregister non-existent resource");
                Err(DiscoveryError::NotFound(url.to_string()))
            }
        }
    }

    /// Get a specific resource by URL.
    pub async fn get(&self, url: &str) -> Option<DiscoveryResource> {
        let resources = self.resources.read().await;
        resources.get(url).cloned()
    }

    /// List resources with pagination and optional filtering.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of resources to return (capped at 100)
    /// * `offset` - Number of resources to skip
    /// * `filters` - Optional filters for category, network, provider, or tag
    pub async fn list(
        &self,
        limit: u32,
        offset: u32,
        filters: Option<DiscoveryFilters>,
    ) -> DiscoveryResponse {
        let resources = self.resources.read().await;

        // Cap limit at 100 to prevent abuse
        let limit = limit.min(100);

        // Collect and filter resources
        let mut filtered: Vec<&DiscoveryResource> = resources
            .values()
            .filter(|r| self.matches_filters(r, &filters))
            .collect();

        // Sort by last_updated descending (newest first)
        filtered.sort_by(|a, b| b.last_updated.cmp(&a.last_updated));

        let total = filtered.len() as u32;

        // Apply pagination
        let items: Vec<DiscoveryResource> = filtered
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();

        debug!(
            total = total,
            returned = items.len(),
            limit = limit,
            offset = offset,
            "Listed discovery resources"
        );

        DiscoveryResponse::new(items, Pagination::new(limit, offset, total))
    }

    /// Get the total count of registered resources.
    pub async fn count(&self) -> usize {
        self.resources.read().await.len()
    }

    /// Check if a resource matches the given filters.
    fn matches_filters(&self, resource: &DiscoveryResource, filters: &Option<DiscoveryFilters>) -> bool {
        let Some(f) = filters else {
            return true;
        };

        // Filter by category
        if let Some(ref category) = f.category {
            let matches = resource
                .metadata
                .as_ref()
                .and_then(|m| m.category.as_ref())
                .map(|c| c.eq_ignore_ascii_case(category))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Filter by network
        if let Some(ref network) = f.network {
            let matches = resource
                .accepts
                .iter()
                .any(|req| req.network.to_string() == *network);
            if !matches {
                return false;
            }
        }

        // Filter by provider
        if let Some(ref provider) = f.provider {
            let matches = resource
                .metadata
                .as_ref()
                .and_then(|m| m.provider.as_ref())
                .map(|p| p.eq_ignore_ascii_case(provider))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Filter by tag
        if let Some(ref tag) = f.tag {
            let matches = resource
                .metadata
                .as_ref()
                .map(|m| m.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        true
    }

    /// Validate a resource before registration.
    fn validate_resource(&self, resource: &DiscoveryResource) -> Result<(), DiscoveryError> {
        // Validate URL scheme
        let scheme = resource.url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(DiscoveryError::InvalidUrl(format!(
                "URL must use http or https scheme, got: {}",
                scheme
            )));
        }

        // Validate resource type
        // "facilitator" is a special type for x402 payment facilitator services
        let valid_types = ["http", "mcp", "a2a", "facilitator"];
        if !valid_types.contains(&resource.resource_type.as_str()) {
            return Err(DiscoveryError::InvalidResourceType(
                resource.resource_type.clone(),
            ));
        }

        // Validate accepts is not empty (except for facilitators, which process payments rather than requiring them)
        if resource.accepts.is_empty() && resource.resource_type != "facilitator" {
            return Err(DiscoveryError::NoPaymentMethods);
        }

        Ok(())
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
    use crate::types_v2::{DiscoveryMetadata, PaymentRequirementsV2};
    use url::Url;

    fn create_test_resource(url: &str, category: Option<&str>) -> DiscoveryResource {
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

        let mut resource = DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "Test resource".to_string(),
            accepts,
        );

        if let Some(cat) = category {
            resource.metadata = Some(DiscoveryMetadata {
                category: Some(cat.to_string()),
                provider: Some("Test Provider".to_string()),
                tags: vec!["test".to_string()],
            });
        }

        resource
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", Some("finance"));

        registry.register(resource.clone()).await.unwrap();

        let retrieved = registry.get("https://api.example.com/data").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().url, resource.url);
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        registry.register(resource.clone()).await.unwrap();

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_update_upsert() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        // Update non-existent resource (upsert)
        registry.update(resource.clone()).await.unwrap();
        assert_eq!(registry.count().await, 1);

        // Update existing resource
        let mut updated = resource.clone();
        updated.description = "Updated description".to_string();
        registry.update(updated).await.unwrap();

        let retrieved = registry.get("https://api.example.com/data").await.unwrap();
        assert_eq!(retrieved.description, "Updated description");
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        registry.register(resource).await.unwrap();
        assert_eq!(registry.count().await, 1);

        registry
            .unregister("https://api.example.com/data")
            .await
            .unwrap();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_not_found() {
        let registry = DiscoveryRegistry::new();

        let result = registry.unregister("https://nonexistent.com").await;
        assert!(matches!(result, Err(DiscoveryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_pagination() {
        let registry = DiscoveryRegistry::new();

        // Register 5 resources
        for i in 0..5 {
            let resource = create_test_resource(
                &format!("https://api{}.example.com/data", i),
                Some("finance"),
            );
            registry.register(resource).await.unwrap();
        }

        // Get first page
        let page1 = registry.list(2, 0, None).await;
        assert_eq!(page1.items.len(), 2);
        assert_eq!(page1.pagination.total, 5);
        assert_eq!(page1.pagination.limit, 2);
        assert_eq!(page1.pagination.offset, 0);

        // Get second page
        let page2 = registry.list(2, 2, None).await;
        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.pagination.offset, 2);

        // Get last page
        let page3 = registry.list(2, 4, None).await;
        assert_eq!(page3.items.len(), 1);
    }

    #[tokio::test]
    async fn test_filter_by_category() {
        let registry = DiscoveryRegistry::new();

        registry
            .register(create_test_resource(
                "https://api1.example.com",
                Some("finance"),
            ))
            .await
            .unwrap();
        registry
            .register(create_test_resource("https://api2.example.com", Some("ai")))
            .await
            .unwrap();
        registry
            .register(create_test_resource(
                "https://api3.example.com",
                Some("finance"),
            ))
            .await
            .unwrap();

        let filters = Some(DiscoveryFilters {
            category: Some("finance".to_string()),
            ..Default::default()
        });

        let response = registry.list(10, 0, filters).await;
        assert_eq!(response.pagination.total, 2);
        assert!(response
            .items
            .iter()
            .all(|r| r.metadata.as_ref().unwrap().category.as_ref().unwrap() == "finance"));
    }

    #[tokio::test]
    async fn test_validation_invalid_url_scheme() {
        let registry = DiscoveryRegistry::new();

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

        let resource = DiscoveryResource::new(
            Url::parse("ftp://invalid.com").unwrap(),
            "http".to_string(),
            "Test".to_string(),
            accepts,
        );

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::InvalidUrl(_))));
    }

    #[tokio::test]
    async fn test_validation_invalid_resource_type() {
        let registry = DiscoveryRegistry::new();

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

        let resource = DiscoveryResource::new(
            Url::parse("https://api.example.com").unwrap(),
            "websocket".to_string(), // Invalid type
            "Test".to_string(),
            accepts,
        );

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::InvalidResourceType(_))));
    }

    #[tokio::test]
    async fn test_validation_no_payment_methods() {
        let registry = DiscoveryRegistry::new();

        let resource = DiscoveryResource::new(
            Url::parse("https://api.example.com").unwrap(),
            "http".to_string(),
            "Test".to_string(),
            vec![], // Empty accepts
        );

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::NoPaymentMethods)));
    }

    #[tokio::test]
    async fn test_limit_capped_at_100() {
        let registry = DiscoveryRegistry::new();

        let response = registry.list(500, 0, None).await;
        assert_eq!(response.pagination.limit, 100);
    }

    #[tokio::test]
    async fn test_facilitator_resource_type() {
        let registry = DiscoveryRegistry::new();

        // Facilitator resources can have empty accepts (they process payments, not require them)
        let resource = DiscoveryResource::new(
            Url::parse("https://facilitator.example.com").unwrap(),
            "facilitator".to_string(),
            "Test Facilitator".to_string(),
            vec![], // Empty accepts is OK for facilitators
        );

        let result = registry.register(resource).await;
        assert!(result.is_ok());

        // Verify it was registered
        let response = registry.list(10, 0, None).await;
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].resource_type, "facilitator");
    }
}
