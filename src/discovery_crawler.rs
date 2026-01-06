//! Discovery Crawler for x402 v2.
//!
//! This module implements a crawler that discovers x402-enabled resources by fetching
//! `/.well-known/x402` endpoints from known domains. Discovered resources are registered
//! in the Bazaar discovery registry with `source: Crawled`.
//!
//! # Architecture
//!
//! ```text
//! Seed URLs                    Discovery Crawler
//! +------------------+        +-------------------------+
//! | api.example.com  |------->| Fetch /.well-known/x402 |
//! | data.service.io  |        |   |                     |
//! +------------------+        |   v                     |
//!                             | Parse response          |
//!                             |   |                     |
//!                             |   v                     |
//!                             | DiscoveryRegistry       |
//!                             | (source: Crawled)       |
//!                             +-------------------------+
//! ```
//!
//! # Well-Known Format
//!
//! The `/.well-known/x402` endpoint should return a JSON object:
//!
//! ```json
//! {
//!   "x402Version": 2,
//!   "resources": [
//!     {
//!       "url": "https://api.example.com/premium",
//!       "type": "http",
//!       "description": "Premium API endpoint",
//!       "accepts": [
//!         {
//!           "scheme": "exact",
//!           "network": "eip155:8453",
//!           "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
//!           "amount": "100000",
//!           "payTo": "0x...",
//!           "maxTimeoutSeconds": 300
//!         }
//!       ]
//!     }
//!   ]
//! }
//! ```

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::discovery::DiscoveryRegistry;
use crate::types_v2::{DiscoveryResource, DiscoverySource, PaymentRequirementsV2};

// ============================================================================
// Well-Known Response Types
// ============================================================================

/// Response format for `/.well-known/x402` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WellKnownX402Response {
    /// x402 protocol version (should be 2)
    #[serde(default = "default_version")]
    pub x402_version: u8,

    /// List of resources available at this domain
    pub resources: Vec<WellKnownResource>,
}

fn default_version() -> u8 {
    2
}

/// A resource declared in the well-known file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WellKnownResource {
    /// URL of the paid resource
    pub url: Url,

    /// Resource type (http, mcp, a2a)
    #[serde(rename = "type")]
    pub resource_type: String,

    /// Human-readable description
    pub description: String,

    /// Payment methods accepted
    pub accepts: Vec<PaymentRequirementsV2>,

    /// Optional metadata
    #[serde(default)]
    pub metadata: Option<WellKnownMetadata>,
}

/// Optional metadata in well-known resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WellKnownMetadata {
    pub category: Option<String>,
    pub provider: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

// ============================================================================
// Discovery Crawler
// ============================================================================

/// Configuration for a seed URL to crawl.
#[derive(Debug, Clone)]
pub struct CrawlTarget {
    /// Base URL of the domain to crawl
    pub base_url: Url,
    /// Optional name for logging
    pub name: Option<String>,
}

impl CrawlTarget {
    pub fn new(base_url: Url) -> Self {
        Self {
            base_url,
            name: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Get the well-known URL for this target.
    pub fn well_known_url(&self) -> Result<Url, url::ParseError> {
        self.base_url.join("/.well-known/x402")
    }
}

/// Crawler for discovering x402 resources from well-known endpoints.
pub struct DiscoveryCrawler {
    client: Client,
    targets: Vec<CrawlTarget>,
    timeout: Duration,
}

impl DiscoveryCrawler {
    /// Create a new crawler with default settings.
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("x402-facilitator/1.0 (discovery-crawler)")
                .build()
                .expect("Failed to create HTTP client"),
            targets: Vec::new(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Add a crawl target.
    pub fn add_target(mut self, target: CrawlTarget) -> Self {
        self.targets.push(target);
        self
    }

    /// Add multiple crawl targets.
    pub fn add_targets(mut self, targets: Vec<CrawlTarget>) -> Self {
        self.targets.extend(targets);
        self
    }

    /// Set the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get the list of default seed URLs.
    ///
    /// These are well-known x402-enabled services that can be crawled.
    pub fn default_targets() -> Vec<CrawlTarget> {
        vec![
            // Add known x402-enabled domains here
            // Example: CrawlTarget::new(Url::parse("https://api.example.com").unwrap())
            //     .with_name("Example API"),
        ]
    }

    /// Fetch and parse a single well-known endpoint.
    async fn fetch_well_known(&self, target: &CrawlTarget) -> Result<WellKnownX402Response, CrawlError> {
        let url = target.well_known_url()
            .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

        let target_name = target.name.as_deref().unwrap_or(target.base_url.as_str());
        debug!(url = %url, target = %target_name, "Fetching well-known x402");

        let response = self.client
            .get(url.clone())
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| CrawlError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(CrawlError::HttpError(response.status().as_u16()));
        }

        let body = response.text().await
            .map_err(|e| CrawlError::NetworkError(e.to_string()))?;

        let parsed: WellKnownX402Response = serde_json::from_str(&body)
            .map_err(|e| CrawlError::ParseError(e.to_string()))?;

        info!(
            url = %url,
            target = %target_name,
            resource_count = parsed.resources.len(),
            "Successfully fetched well-known x402"
        );

        Ok(parsed)
    }

    /// Convert a well-known resource to a discovery resource.
    fn to_discovery_resource(resource: WellKnownResource, source_domain: &str) -> DiscoveryResource {
        use crate::types_v2::DiscoveryMetadata;
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let metadata = resource.metadata.map(|m| DiscoveryMetadata {
            category: m.category,
            provider: m.provider,
            tags: m.tags,
        });

        DiscoveryResource {
            url: resource.url,
            resource_type: resource.resource_type,
            x402_version: 2,
            description: resource.description,
            accepts: resource.accepts,
            last_updated: now,
            metadata,
            source: DiscoverySource::Crawled,
            source_facilitator: Some(source_domain.to_string()),
            first_seen: Some(now),
            settlement_count: None,
        }
    }

    /// Crawl all targets and import discovered resources.
    pub async fn crawl_all(&self, registry: &DiscoveryRegistry) -> CrawlSummary {
        let mut summary = CrawlSummary::default();

        for target in &self.targets {
            let target_name = target.name.as_deref()
                .unwrap_or(target.base_url.host_str().unwrap_or("unknown"));

            match self.fetch_well_known(target).await {
                Ok(response) => {
                    let source_domain = target.base_url.host_str()
                        .unwrap_or("unknown")
                        .to_string();

                    let resources: Vec<DiscoveryResource> = response.resources
                        .into_iter()
                        .map(|r| Self::to_discovery_resource(r, &source_domain))
                        .collect();

                    let resource_count = resources.len();

                    match registry.bulk_import(resources, true).await {
                        Ok((added, updated, skipped)) => {
                            summary.targets_crawled += 1;
                            summary.resources_added += added;
                            summary.resources_updated += updated;
                            summary.resources_skipped += skipped;

                            info!(
                                target = %target_name,
                                resources = resource_count,
                                added = added,
                                updated = updated,
                                skipped = skipped,
                                "Crawled target successfully"
                            );
                        }
                        Err(e) => {
                            summary.targets_failed += 1;
                            error!(
                                target = %target_name,
                                error = %e,
                                "Failed to import crawled resources"
                            );
                        }
                    }
                }
                Err(e) => {
                    summary.targets_failed += 1;
                    match &e {
                        CrawlError::HttpError(404) => {
                            debug!(
                                target = %target_name,
                                "No well-known x402 endpoint (404)"
                            );
                        }
                        _ => {
                            warn!(
                                target = %target_name,
                                error = %e,
                                "Failed to crawl target"
                            );
                        }
                    }
                }
            }
        }

        info!(
            targets_crawled = summary.targets_crawled,
            targets_failed = summary.targets_failed,
            resources_added = summary.resources_added,
            resources_updated = summary.resources_updated,
            "Crawl completed"
        );

        summary
    }
}

impl Default for DiscoveryCrawler {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during crawling.
#[derive(Debug, thiserror::Error)]
pub enum CrawlError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("HTTP error: {0}")]
    HttpError(u16),

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Summary of a crawl operation.
#[derive(Debug, Default)]
pub struct CrawlSummary {
    pub targets_crawled: usize,
    pub targets_failed: usize,
    pub resources_added: usize,
    pub resources_updated: usize,
    pub resources_skipped: usize,
}

// ============================================================================
// Background Task
// ============================================================================

/// Start a background task that periodically crawls well-known endpoints.
///
/// # Arguments
///
/// * `registry` - The discovery registry to import resources into
/// * `targets` - List of URLs to crawl
/// * `interval_secs` - How often to crawl (in seconds)
///
/// # Returns
///
/// A JoinHandle for the background task.
pub fn start_crawl_task(
    registry: DiscoveryRegistry,
    targets: Vec<CrawlTarget>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let crawler = DiscoveryCrawler::new()
            .add_targets(targets);

        // Initial crawl
        info!("Starting initial well-known crawl");
        crawler.crawl_all(&registry).await;

        // Periodic crawl
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.tick().await; // Skip first tick (already did initial crawl)

        loop {
            interval.tick().await;
            info!("Starting periodic well-known crawl");
            crawler.crawl_all(&registry).await;
        }
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crawl_target_well_known_url() {
        let target = CrawlTarget::new(Url::parse("https://api.example.com").unwrap());
        let url = target.well_known_url().unwrap();
        assert_eq!(url.as_str(), "https://api.example.com/.well-known/x402");
    }

    #[test]
    fn test_crawl_target_with_path() {
        let target = CrawlTarget::new(Url::parse("https://api.example.com/v1/").unwrap());
        let url = target.well_known_url().unwrap();
        // Note: join with absolute path replaces the path
        assert_eq!(url.as_str(), "https://api.example.com/.well-known/x402");
    }

    #[test]
    fn test_parse_well_known_response() {
        let json = r#"{
            "x402Version": 2,
            "resources": [
                {
                    "url": "https://api.example.com/premium",
                    "type": "http",
                    "description": "Premium API",
                    "accepts": [
                        {
                            "scheme": "exact",
                            "network": "eip155:8453",
                            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                            "amount": "100000",
                            "payTo": "0x1234567890123456789012345678901234567890",
                            "maxTimeoutSeconds": 300
                        }
                    ]
                }
            ]
        }"#;

        let response: WellKnownX402Response = serde_json::from_str(json).unwrap();
        assert_eq!(response.x402_version, 2);
        assert_eq!(response.resources.len(), 1);
        assert_eq!(response.resources[0].resource_type, "http");
    }

    #[test]
    fn test_default_crawler() {
        let crawler = DiscoveryCrawler::new();
        assert!(crawler.targets.is_empty());
    }
}
