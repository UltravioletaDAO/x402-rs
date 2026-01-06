//! Bazaar Discovery Aggregator for Meta-Bazaar architecture.
//!
//! This module aggregates discoverable resources from external facilitators,
//! enabling the Ultravioleta facilitator to serve as a "Meta-Bazaar" that
//! indexes services from across the x402 ecosystem.
//!
//! # Supported Sources
//!
//! - **Coinbase CDP**: `https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources`
//! - Future: Other x402-compatible facilitators
//!
//! # Architecture
//!
//! ```text
//! External Facilitators          Ultravioleta Facilitator
//! ┌─────────────────┐           ┌─────────────────────────┐
//! │ Coinbase Bazaar │──fetch──▶ │ DiscoveryAggregator     │
//! │ 1,700+ services │           │   │                     │
//! └─────────────────┘           │   ▼                     │
//!                               │ Convert to v2 format    │
//! ┌─────────────────┐           │   │                     │
//! │ Other Facilitator│──fetch──▶│   ▼                     │
//! └─────────────────┘           │ DiscoveryRegistry       │
//!                               │ (source: Aggregated)    │
//!                               └─────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use x402_rs::discovery_aggregator::{DiscoveryAggregator, FacilitatorConfig};
//!
//! let aggregator = DiscoveryAggregator::new();
//! let resources = aggregator.fetch_all().await?;
//! registry.bulk_import(resources, true).await?;
//! ```

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use url::Url;

use alloy::primitives::U256;
use std::str::FromStr;

use crate::caip2::Caip2NetworkId;
use crate::types::{MixedAddress, Scheme, TokenAmount};
use crate::types_v2::{DiscoveryMetadata, DiscoveryResource, PaymentRequirementsV2};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during aggregation.
#[derive(Debug, thiserror::Error)]
pub enum AggregatorError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    /// Failed to parse response
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Invalid URL in response
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Facilitator returned error
    #[error("Facilitator error: {0}")]
    FacilitatorError(String),
}

// ============================================================================
// Facilitator Configuration
// ============================================================================

/// Configuration for an external facilitator to aggregate from.
#[derive(Debug, Clone)]
pub struct FacilitatorConfig {
    /// Unique identifier for this facilitator
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Base URL for the discovery API
    pub discovery_url: String,
    /// Whether this facilitator is enabled
    pub enabled: bool,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl FacilitatorConfig {
    /// Create Coinbase CDP facilitator config
    pub fn coinbase() -> Self {
        Self {
            id: "coinbase".to_string(),
            name: "Coinbase CDP".to_string(),
            discovery_url: "https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }
}

// ============================================================================
// Coinbase Response Types
// ============================================================================

/// Coinbase discovery resource format (v1-style).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbaseResource {
    /// Resource URL
    pub url: String,
    /// Resource type (http, mcp, etc.)
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    /// Description
    pub description: Option<String>,
    /// Payment requirements (v1 format)
    #[serde(default)]
    pub accepts: Vec<CoinbasePaymentRequirement>,
    /// Last updated timestamp
    pub last_updated: Option<u64>,
    /// Metadata
    #[serde(default)]
    pub metadata: Option<CoinbaseMetadata>,
}

/// Coinbase payment requirement (v1-style network names).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbasePaymentRequirement {
    /// Payment scheme
    pub scheme: Option<String>,
    /// Network name (v1 format like "base-mainnet" or "base")
    pub network: Option<String>,
    /// Token asset address
    pub asset: Option<String>,
    /// Amount required
    #[serde(alias = "maxAmountRequired")]
    pub amount: Option<String>,
    /// Pay-to address
    pub pay_to: Option<String>,
    /// Max timeout
    pub max_timeout_seconds: Option<u64>,
}

/// Coinbase metadata format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbaseMetadata {
    pub category: Option<String>,
    pub provider: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Coinbase discovery response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbaseDiscoveryResponse {
    pub items: Vec<CoinbaseResource>,
    pub pagination: Option<CoinbasePagination>,
}

/// Coinbase pagination info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbasePagination {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub total: Option<u32>,
}

// ============================================================================
// Discovery Aggregator
// ============================================================================

/// Aggregates discoverable resources from external facilitators.
#[derive(Debug, Clone)]
pub struct DiscoveryAggregator {
    client: Client,
    facilitators: Vec<FacilitatorConfig>,
}

impl Default for DiscoveryAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryAggregator {
    /// Create a new aggregator with default facilitators.
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("x402-rs-aggregator/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            facilitators: vec![FacilitatorConfig::coinbase()],
        }
    }

    /// Create an aggregator with custom facilitator configs.
    pub fn with_facilitators(facilitators: Vec<FacilitatorConfig>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("x402-rs-aggregator/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self { client, facilitators }
    }

    /// Fetch resources from all enabled facilitators.
    pub async fn fetch_all(&self) -> Vec<DiscoveryResource> {
        let mut all_resources = Vec::new();

        for config in &self.facilitators {
            if !config.enabled {
                debug!(facilitator = %config.id, "Skipping disabled facilitator");
                continue;
            }

            match self.fetch_from_facilitator(config).await {
                Ok(resources) => {
                    info!(
                        facilitator = %config.id,
                        count = resources.len(),
                        "Fetched resources from facilitator"
                    );
                    all_resources.extend(resources);
                }
                Err(e) => {
                    error!(
                        facilitator = %config.id,
                        error = %e,
                        "Failed to fetch from facilitator"
                    );
                }
            }
        }

        info!(total = all_resources.len(), "Total resources aggregated");
        all_resources
    }

    /// Fetch resources from a specific facilitator.
    async fn fetch_from_facilitator(
        &self,
        config: &FacilitatorConfig,
    ) -> Result<Vec<DiscoveryResource>, AggregatorError> {
        info!(facilitator = %config.id, url = %config.discovery_url, "Fetching from facilitator");

        // Fetch with pagination - try to get all resources
        let mut all_resources = Vec::new();
        let mut offset = 0;
        let limit = 100;

        loop {
            let url = format!("{}?limit={}&offset={}", config.discovery_url, limit, offset);

            let response = self
                .client
                .get(&url)
                .timeout(Duration::from_secs(config.timeout_secs))
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(AggregatorError::FacilitatorError(format!(
                    "HTTP {}: {}",
                    response.status(),
                    response.text().await.unwrap_or_default()
                )));
            }

            let body = response.text().await?;

            // Try to parse as Coinbase format
            let parsed: CoinbaseDiscoveryResponse = serde_json::from_str(&body)
                .map_err(|e| AggregatorError::ParseError(format!("{}: {}", e, &body[..500.min(body.len())])))?;

            let batch_count = parsed.items.len();

            // Convert to our format
            let resources = self.convert_coinbase_resources(parsed.items, &config.id);
            all_resources.extend(resources);

            // Check if we need to fetch more
            let total = parsed.pagination.as_ref().and_then(|p| p.total).unwrap_or(0);
            offset += batch_count as u32;

            if batch_count < limit as usize || offset >= total {
                break;
            }

            debug!(offset = offset, total = total, "Fetching next page");
        }

        Ok(all_resources)
    }

    /// Convert Coinbase resources to our v2 format.
    fn convert_coinbase_resources(
        &self,
        resources: Vec<CoinbaseResource>,
        facilitator_id: &str,
    ) -> Vec<DiscoveryResource> {
        let mut converted = Vec::new();

        for cb_resource in resources {
            match self.convert_single_resource(cb_resource, facilitator_id) {
                Ok(resource) => converted.push(resource),
                Err(e) => {
                    debug!(error = %e, "Skipping resource due to conversion error");
                }
            }
        }

        converted
    }

    /// Convert a single Coinbase resource.
    fn convert_single_resource(
        &self,
        cb: CoinbaseResource,
        facilitator_id: &str,
    ) -> Result<DiscoveryResource, AggregatorError> {
        // Parse URL
        let url = Url::parse(&cb.url)
            .map_err(|e| AggregatorError::InvalidUrl(format!("{}: {}", cb.url, e)))?;

        // Convert payment requirements
        let accepts: Vec<PaymentRequirementsV2> = cb
            .accepts
            .into_iter()
            .filter_map(|req| self.convert_payment_requirement(req))
            .collect();

        // Use default timestamp if not provided
        let last_updated = cb.last_updated.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });

        // Create resource with aggregation source
        let mut resource = DiscoveryResource::from_aggregation(
            url,
            cb.resource_type.unwrap_or_else(|| "http".to_string()),
            cb.description.unwrap_or_default(),
            accepts,
            facilitator_id.to_string(),
            last_updated,
        );

        // Convert metadata if present
        if let Some(meta) = cb.metadata {
            resource.metadata = Some(DiscoveryMetadata {
                category: meta.category,
                provider: meta.provider,
                tags: meta.tags,
            });
        }

        Ok(resource)
    }

    /// Convert a Coinbase payment requirement to v2 format.
    fn convert_payment_requirement(&self, req: CoinbasePaymentRequirement) -> Option<PaymentRequirementsV2> {
        // Parse network - Coinbase uses v1 names like "base", "base-mainnet"
        let network_str = req.network.as_deref()?;
        let network = self.parse_network_to_caip2(network_str)?;

        // Parse asset address
        let asset_str = req.asset.as_deref()?;
        let asset = self.parse_address(asset_str)?;

        // Parse pay_to address
        let pay_to_str = req.pay_to.as_deref()?;
        let pay_to = self.parse_address(pay_to_str)?;

        // Parse amount (assumed to be in smallest units, e.g., 1000000 = 1 USDC)
        let amount_str = req.amount.as_deref().unwrap_or("0");
        let amount = U256::from_str(amount_str)
            .map(TokenAmount::from)
            .unwrap_or_else(|_| TokenAmount::from(0u64));

        Some(PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network,
            asset,
            amount,
            pay_to,
            max_timeout_seconds: req.max_timeout_seconds.unwrap_or(300),
            extra: None,
        })
    }

    /// Parse a v1 network name to CAIP-2 format.
    fn parse_network_to_caip2(&self, network: &str) -> Option<Caip2NetworkId> {
        // Handle common v1 network names
        let chain_id = match network.to_lowercase().as_str() {
            "base" | "base-mainnet" => 8453,
            "base-sepolia" => 84532,
            "ethereum" | "mainnet" | "ethereum-mainnet" => 1,
            "sepolia" | "ethereum-sepolia" => 11155111,
            "polygon" | "polygon-mainnet" | "matic" => 137,
            "polygon-amoy" | "amoy" => 80002,
            "optimism" | "optimism-mainnet" => 10,
            "optimism-sepolia" => 11155420,
            "arbitrum" | "arbitrum-mainnet" | "arbitrum-one" => 42161,
            "arbitrum-sepolia" => 421614,
            "avalanche" | "avalanche-mainnet" | "avalanche-c-chain" => 43114,
            "avalanche-fuji" | "fuji" => 43113,
            "celo" | "celo-mainnet" => 42220,
            "celo-alfajores" | "alfajores" => 44787,
            _ => {
                // Try to parse as CAIP-2 directly
                if network.starts_with("eip155:") {
                    return Caip2NetworkId::parse(network).ok();
                }
                // Try to parse as number
                network.parse::<u64>().ok()?
            }
        };

        Some(Caip2NetworkId::eip155(chain_id))
    }

    /// Parse an address string to MixedAddress.
    fn parse_address(&self, addr: &str) -> Option<MixedAddress> {
        // Try EVM address first
        if addr.starts_with("0x") && addr.len() == 42 {
            addr.parse().ok().map(MixedAddress::Evm)
        } else {
            // Could be Solana or other - for now just skip non-EVM
            None
        }
    }
}

// ============================================================================
// Background Aggregation Task
// ============================================================================

/// Start a background task that periodically aggregates from external facilitators.
///
/// # Arguments
///
/// * `registry` - The discovery registry to import into
/// * `interval_secs` - How often to run aggregation (in seconds)
///
/// Returns a handle that can be used to abort the task.
pub fn start_aggregation_task(
    registry: crate::discovery::DiscoveryRegistry,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    info!(interval_secs = interval_secs, "Starting discovery aggregation background task");

    tokio::spawn(async move {
        let aggregator = DiscoveryAggregator::new();
        let interval = Duration::from_secs(interval_secs);

        // Run immediately on startup
        run_aggregation(&aggregator, &registry).await;

        // Then run periodically
        loop {
            tokio::time::sleep(interval).await;
            run_aggregation(&aggregator, &registry).await;
        }
    })
}

/// Run a single aggregation cycle.
async fn run_aggregation(
    aggregator: &DiscoveryAggregator,
    registry: &crate::discovery::DiscoveryRegistry,
) {
    info!("Running discovery aggregation cycle");

    let resources = aggregator.fetch_all().await;

    if resources.is_empty() {
        warn!("No resources fetched from external facilitators");
        return;
    }

    match registry.bulk_import(resources, true).await {
        Ok((added, updated, skipped)) => {
            info!(
                added = added,
                updated = updated,
                skipped = skipped,
                "Discovery aggregation cycle completed"
            );
        }
        Err(e) => {
            error!(error = %e, "Failed to import aggregated resources");
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_network_to_caip2() {
        let aggregator = DiscoveryAggregator::new();

        // Test common network names
        assert_eq!(
            aggregator.parse_network_to_caip2("base").unwrap().to_string(),
            "eip155:8453"
        );
        assert_eq!(
            aggregator.parse_network_to_caip2("base-mainnet").unwrap().to_string(),
            "eip155:8453"
        );
        assert_eq!(
            aggregator.parse_network_to_caip2("ethereum").unwrap().to_string(),
            "eip155:1"
        );
        assert_eq!(
            aggregator.parse_network_to_caip2("polygon").unwrap().to_string(),
            "eip155:137"
        );

        // Test CAIP-2 passthrough
        assert_eq!(
            aggregator.parse_network_to_caip2("eip155:8453").unwrap().to_string(),
            "eip155:8453"
        );
    }

    #[test]
    fn test_parse_address() {
        let aggregator = DiscoveryAggregator::new();

        // Valid EVM address
        let addr = aggregator.parse_address("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
        assert!(addr.is_some());
        assert!(matches!(addr.unwrap(), MixedAddress::Evm(_)));

        // Invalid address
        assert!(aggregator.parse_address("invalid").is_none());
        assert!(aggregator.parse_address("0x123").is_none()); // Too short
    }

    #[test]
    fn test_facilitator_config() {
        let config = FacilitatorConfig::coinbase();
        assert_eq!(config.id, "coinbase");
        assert!(config.enabled);
        assert!(config.discovery_url.contains("coinbase"));
    }
}
