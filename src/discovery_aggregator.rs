//! Bazaar Discovery Aggregator for Meta-Bazaar architecture.
//!
//! This module aggregates discoverable resources from external facilitators,
//! enabling the Ultravioleta facilitator to serve as a "Meta-Bazaar" that
//! indexes services from across the x402 ecosystem.
//!
//! # Supported Sources
//!
//! - **Coinbase CDP**: `https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources`
//! - **PayAI**: `https://facilitator.payai.network/discovery/resources`
//! - **Thirdweb**: `https://api.thirdweb.com/v1/payments/x402/discovery/resources`
//! - **QuestFlow**: `https://facilitator.questflow.ai/discovery/resources`
//! - **AurraCloud**: `https://x402-facilitator.aurracloud.com/discovery/resources`
//! - **AnySpend**: `https://mainnet.anyspend.com/x402/discovery/resources`
//! - **OpenX402**: `https://open.x402.host/discovery/resources`
//! - **x402.rs**: `https://facilitator.x402.rs/discovery/resources`
//! - **Heurist**: `https://facilitator.heurist.xyz/discovery/resources`
//! - **Polymer**: `https://api.polymer.zone/x402/v1/discovery/resources`
//! - **Meridian**: `https://api.mrdn.finance/discovery/resources`
//! - **Virtuals**: `https://acpx.virtuals.io/discovery/resources`
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
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};
use url::Url;

use alloy::primitives::U256;
use std::str::FromStr;

// ============================================================================
// Timestamp Parsing (handles both u64 and ISO8601 string)
// ============================================================================

/// Deserialize a timestamp that can be either a u64 (Unix seconds) or an ISO8601 string.
fn deserialize_flexible_timestamp<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct FlexibleTimestampVisitor;

    impl<'de> Visitor<'de> for FlexibleTimestampVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u64 timestamp or an ISO8601 date string")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_any(FlexibleTimestampInnerVisitor)
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as u64))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_iso8601_to_unix(value)
                .map(Some)
                .map_err(de::Error::custom)
        }
    }

    struct FlexibleTimestampInnerVisitor;

    impl<'de> Visitor<'de> for FlexibleTimestampInnerVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u64 timestamp or an ISO8601 date string")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as u64))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_iso8601_to_unix(value)
                .map(Some)
                .map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_option(FlexibleTimestampVisitor)
}

/// Parse an ISO8601 date string to Unix timestamp.
/// Handles formats like "2026-01-06T20:22:59.724Z" or "2026-01-06T20:22:59Z"
fn parse_iso8601_to_unix(s: &str) -> Result<u64, String> {
    // Try to parse ISO8601 format manually
    // Format: YYYY-MM-DDTHH:MM:SS.sssZ or YYYY-MM-DDTHH:MM:SSZ
    let s = s.trim_end_matches('Z');
    let (date_time, _millis) = if let Some(dot_pos) = s.find('.') {
        (&s[..dot_pos], &s[dot_pos + 1..])
    } else {
        (s, "")
    };

    // Parse date and time parts
    let parts: Vec<&str> = date_time.split('T').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid ISO8601 format: {}", s));
    }

    let date_parts: Vec<u32> = parts[0]
        .split('-')
        .filter_map(|p| p.parse().ok())
        .collect();
    let time_parts: Vec<u32> = parts[1]
        .split(':')
        .filter_map(|p| p.parse().ok())
        .collect();

    if date_parts.len() != 3 || time_parts.len() != 3 {
        return Err(format!("Invalid date/time components: {}", s));
    }

    let year = date_parts[0];
    let month = date_parts[1];
    let day = date_parts[2];
    let hour = time_parts[0];
    let minute = time_parts[1];
    let second = time_parts[2];

    // Calculate Unix timestamp (simplified - doesn't handle leap seconds)
    // Days since Unix epoch (1970-01-01)
    let days = days_since_epoch(year, month, day);
    let seconds = days as u64 * 86400 + hour as u64 * 3600 + minute as u64 * 60 + second as u64;

    Ok(seconds)
}

/// Calculate days since Unix epoch (1970-01-01).
fn days_since_epoch(year: u32, month: u32, day: u32) -> i64 {
    // Algorithm from Howard Hinnant's date library
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

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

    /// Create PayAI facilitator config
    pub fn payai() -> Self {
        Self {
            id: "payai".to_string(),
            name: "PayAI".to_string(),
            discovery_url: "https://facilitator.payai.network/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create Thirdweb facilitator config
    pub fn thirdweb() -> Self {
        Self {
            id: "thirdweb".to_string(),
            name: "Thirdweb".to_string(),
            discovery_url: "https://api.thirdweb.com/v1/payments/x402/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create QuestFlow facilitator config
    pub fn questflow() -> Self {
        Self {
            id: "questflow".to_string(),
            name: "QuestFlow".to_string(),
            discovery_url: "https://facilitator.questflow.ai/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create AurraCloud facilitator config
    pub fn aurracloud() -> Self {
        Self {
            id: "aurracloud".to_string(),
            name: "AurraCloud".to_string(),
            discovery_url: "https://x402-facilitator.aurracloud.com/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create AnySpend facilitator config
    pub fn anyspend() -> Self {
        Self {
            id: "anyspend".to_string(),
            name: "AnySpend".to_string(),
            discovery_url: "https://mainnet.anyspend.com/x402/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create OpenX402 facilitator config
    pub fn openx402() -> Self {
        Self {
            id: "openx402".to_string(),
            name: "OpenX402".to_string(),
            discovery_url: "https://open.x402.host/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create x402.rs facilitator config (upstream)
    pub fn x402rs() -> Self {
        Self {
            id: "x402rs".to_string(),
            name: "x402.rs".to_string(),
            discovery_url: "https://facilitator.x402.rs/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create Heurist facilitator config
    pub fn heurist() -> Self {
        Self {
            id: "heurist".to_string(),
            name: "Heurist".to_string(),
            discovery_url: "https://facilitator.heurist.xyz/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create Polymer facilitator config
    pub fn polymer() -> Self {
        Self {
            id: "polymer".to_string(),
            name: "Polymer".to_string(),
            discovery_url: "https://api.polymer.zone/x402/v1/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create Meridian facilitator config
    pub fn meridian() -> Self {
        Self {
            id: "meridian".to_string(),
            name: "Meridian".to_string(),
            discovery_url: "https://api.mrdn.finance/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Create Virtuals facilitator config
    pub fn virtuals() -> Self {
        Self {
            id: "virtuals".to_string(),
            name: "Virtuals Protocol".to_string(),
            discovery_url: "https://acpx.virtuals.io/discovery/resources".to_string(),
            enabled: true,
            timeout_secs: 30,
        }
    }

    /// Get all known facilitator configs
    pub fn all() -> Vec<Self> {
        vec![
            Self::coinbase(),
            Self::payai(),
            Self::thirdweb(),
            Self::questflow(),
            Self::aurracloud(),
            Self::anyspend(),
            Self::openx402(),
            Self::x402rs(),
            Self::heurist(),
            Self::polymer(),
            Self::meridian(),
            Self::virtuals(),
        ]
    }
}

// ============================================================================
// Coinbase Response Types
// ============================================================================

/// Coinbase discovery resource format (v1-style).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoinbaseResource {
    /// Resource URL (accepts both "url" and "resource" field names)
    #[serde(alias = "resource")]
    pub url: String,
    /// Resource type (http, mcp, etc.)
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
    /// Description
    pub description: Option<String>,
    /// Payment requirements (v1 format)
    #[serde(default)]
    pub accepts: Vec<CoinbasePaymentRequirement>,
    /// Last updated timestamp (can be u64 or ISO8601 string)
    #[serde(default, deserialize_with = "deserialize_flexible_timestamp")]
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

/// Wrapped discovery response (some facilitators wrap in "data" object).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrappedDiscoveryResponse {
    pub data: CoinbaseDiscoveryResponse,
}

/// Alternative response format with "resources" instead of "items".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeDiscoveryResponse {
    #[serde(alias = "items")]
    pub resources: Vec<CoinbaseResource>,
    pub pagination: Option<CoinbasePagination>,
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
    /// Create a new aggregator with all known facilitators.
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("x402-rs-aggregator/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            facilitators: FacilitatorConfig::all(),
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

            // Try multiple response formats (facilitators use different schemas)
            let (items, pagination) = self.parse_discovery_response(&body, &config.id)?;

            let batch_count = items.len();

            // Convert to our format
            let resources = self.convert_coinbase_resources(items, &config.id);
            all_resources.extend(resources);

            // Check if we need to fetch more
            let total = pagination.as_ref().and_then(|p| p.total).unwrap_or(0);
            offset += batch_count as u32;

            if batch_count < limit as usize || offset >= total {
                break;
            }

            debug!(offset = offset, total = total, "Fetching next page");
        }

        Ok(all_resources)
    }

    /// Parse discovery response, trying multiple formats.
    ///
    /// Different facilitators use different response schemas:
    /// - Standard: `{ "items": [...], "pagination": {...} }`
    /// - Wrapped: `{ "data": { "items": [...] } }`
    /// - Alternative: `{ "resources": [...] }`
    fn parse_discovery_response(
        &self,
        body: &str,
        facilitator_id: &str,
    ) -> Result<(Vec<CoinbaseResource>, Option<CoinbasePagination>), AggregatorError> {
        // Try 1: Standard Coinbase format with "items"
        if let Ok(parsed) = serde_json::from_str::<CoinbaseDiscoveryResponse>(body) {
            debug!(facilitator = facilitator_id, format = "standard", items = parsed.items.len(), "Parsed response");
            return Ok((parsed.items, parsed.pagination));
        }

        // Try 2: Wrapped format with "data" object
        if let Ok(parsed) = serde_json::from_str::<WrappedDiscoveryResponse>(body) {
            debug!(facilitator = facilitator_id, format = "wrapped", items = parsed.data.items.len(), "Parsed response");
            return Ok((parsed.data.items, parsed.data.pagination));
        }

        // Try 3: Alternative format with "resources" instead of "items"
        if let Ok(parsed) = serde_json::from_str::<AlternativeDiscoveryResponse>(body) {
            debug!(facilitator = facilitator_id, format = "alternative", resources = parsed.resources.len(), "Parsed response");
            return Ok((parsed.resources, parsed.pagination));
        }

        // Try 4: Direct array of resources
        if let Ok(resources) = serde_json::from_str::<Vec<CoinbaseResource>>(body) {
            debug!(facilitator = facilitator_id, format = "array", resources = resources.len(), "Parsed response");
            return Ok((resources, None));
        }

        // All formats failed
        let preview = &body[..500.min(body.len())];
        Err(AggregatorError::ParseError(format!(
            "Unknown response format from {}: {}",
            facilitator_id, preview
        )))
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

    #[test]
    fn test_all_facilitators() {
        let all = FacilitatorConfig::all();
        assert_eq!(all.len(), 12);

        // Verify we have all expected facilitators
        let ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"coinbase"));
        assert!(ids.contains(&"payai"));
        assert!(ids.contains(&"thirdweb"));
        assert!(ids.contains(&"questflow"));
        assert!(ids.contains(&"aurracloud"));
        assert!(ids.contains(&"anyspend"));
        assert!(ids.contains(&"openx402"));
        assert!(ids.contains(&"x402rs"));
        assert!(ids.contains(&"heurist"));
        assert!(ids.contains(&"polymer"));
        assert!(ids.contains(&"meridian"));
        assert!(ids.contains(&"virtuals"));
    }

    #[test]
    fn test_parse_iso8601_to_unix() {
        // Test a known date (Unix epoch)
        let epoch = parse_iso8601_to_unix("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(epoch, 0);

        // Test Y2K
        let y2k = parse_iso8601_to_unix("2000-01-01T00:00:00Z").unwrap();
        assert_eq!(y2k, 946684800);

        // Test full ISO8601 with milliseconds
        let ts = parse_iso8601_to_unix("2026-01-06T20:22:59.724Z").unwrap();
        // Just verify it parses and returns a reasonable value (around 2026)
        assert!(ts > 1700000000); // After 2023
        assert!(ts < 1800000000); // Before 2027

        // Test ISO8601 without milliseconds should give same result
        let ts2 = parse_iso8601_to_unix("2026-01-06T20:22:59Z").unwrap();
        assert_eq!(ts, ts2);
    }

    #[test]
    fn test_deserialize_flexible_timestamp_u64() {
        let json = r#"{"last_updated": 1767730979}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_flexible_timestamp")]
            last_updated: Option<u64>,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.last_updated, Some(1767730979));
    }

    #[test]
    fn test_deserialize_flexible_timestamp_string() {
        let json = r#"{"last_updated": "2026-01-06T20:22:59.724Z"}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_flexible_timestamp")]
            last_updated: Option<u64>,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        // Just verify it parses successfully and returns a reasonable timestamp
        assert!(parsed.last_updated.is_some());
        let ts = parsed.last_updated.unwrap();
        assert!(ts > 1700000000); // After 2023
        assert!(ts < 1800000000); // Before 2027
    }

    #[test]
    fn test_deserialize_flexible_timestamp_null() {
        let json = r#"{"last_updated": null}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_flexible_timestamp")]
            last_updated: Option<u64>,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.last_updated, None);
    }

    #[test]
    fn test_deserialize_flexible_timestamp_missing() {
        let json = r#"{}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            #[serde(default, deserialize_with = "deserialize_flexible_timestamp")]
            last_updated: Option<u64>,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.last_updated, None);
    }
}
