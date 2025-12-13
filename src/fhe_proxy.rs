//! FHE (Fully Homomorphic Encryption) Proxy Module
//!
//! This module handles proxying x402 payment requests with `fhe-transfer` scheme
//! to the Zama FHE facilitator Lambda endpoint. The Lambda handles ERC7984
//! confidential token verification using Zama FHEVM.
//!
//! Architecture:
//! - Requests with scheme `fhe-transfer` are detected in handlers
//! - This module forwards them to the Lambda endpoint
//! - Lambda processes FHE-specific verification/settlement
//! - Response is returned to the original caller

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Configuration for the FHE proxy
#[derive(Clone, Debug)]
pub struct FheProxyConfig {
    /// Base URL of the Zama FHE facilitator Lambda
    /// Default: https://zama-facilitator.ultravioletadao.xyz
    pub endpoint: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl Default for FheProxyConfig {
    fn default() -> Self {
        Self {
            endpoint: std::env::var("FHE_FACILITATOR_URL")
                .unwrap_or_else(|_| "https://zama-facilitator.ultravioletadao.xyz".to_string()),
            timeout_secs: 30,
        }
    }
}

/// FHE Proxy client for forwarding requests to Zama Lambda
#[derive(Clone)]
pub struct FheProxy {
    client: Client,
    config: FheProxyConfig,
}

/// Response from FHE verify endpoint
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FheVerifyResponse {
    pub is_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decrypted_amount: Option<String>,
}

/// Error type for FHE proxy operations
#[derive(Debug, thiserror::Error)]
pub enum FheProxyError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("FHE facilitator returned error: {0}")]
    FacilitatorError(String),

    #[error("Invalid response from FHE facilitator: {0}")]
    InvalidResponse(String),

    #[error("FHE facilitator unavailable")]
    Unavailable,
}

impl FheProxy {
    /// Create a new FHE proxy with default configuration
    pub fn new() -> Self {
        Self::with_config(FheProxyConfig::default())
    }

    /// Create a new FHE proxy with custom configuration
    pub fn with_config(config: FheProxyConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        info!(
            endpoint = %config.endpoint,
            "FHE proxy initialized"
        );

        Self { client, config }
    }

    /// Check if the FHE facilitator is healthy
    pub async fn health_check(&self) -> Result<bool, FheProxyError> {
        let url = format!("{}/health", self.config.endpoint);
        debug!(url = %url, "Checking FHE facilitator health");

        let response = self.client.get(&url).send().await?;

        if response.status().is_success() {
            Ok(true)
        } else {
            warn!(
                status = %response.status(),
                "FHE facilitator health check failed"
            );
            Ok(false)
        }
    }

    /// Forward a verify request to the FHE facilitator
    pub async fn verify(&self, body: &serde_json::Value) -> Result<FheVerifyResponse, FheProxyError> {
        let url = format!("{}/verify", self.config.endpoint);
        info!(url = %url, "Forwarding verify request to FHE facilitator");

        let response = self.client
            .post(&url)
            .json(body)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!(
            status = %status,
            body_len = response_text.len(),
            "Received response from FHE facilitator"
        );

        if status.is_success() {
            serde_json::from_str(&response_text)
                .map_err(|e| FheProxyError::InvalidResponse(format!(
                    "Failed to parse verify response: {} - body: {}",
                    e, response_text
                )))
        } else {
            error!(
                status = %status,
                body = %response_text,
                "FHE facilitator verify failed"
            );
            Err(FheProxyError::FacilitatorError(response_text))
        }
    }

    /// Forward a settle request to the FHE facilitator
    pub async fn settle(&self, body: &serde_json::Value) -> Result<serde_json::Value, FheProxyError> {
        let url = format!("{}/settle", self.config.endpoint);
        info!(url = %url, "Forwarding settle request to FHE facilitator");

        let response = self.client
            .post(&url)
            .json(body)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!(
            status = %status,
            body_len = response_text.len(),
            "Received response from FHE facilitator"
        );

        if status.is_success() {
            serde_json::from_str(&response_text)
                .map_err(|e| FheProxyError::InvalidResponse(format!(
                    "Failed to parse settle response: {} - body: {}",
                    e, response_text
                )))
        } else {
            error!(
                status = %status,
                body = %response_text,
                "FHE facilitator settle failed"
            );
            Err(FheProxyError::FacilitatorError(response_text))
        }
    }
}

impl Default for FheProxy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FheProxyConfig::default();
        assert!(config.endpoint.contains("zama-facilitator"));
        assert_eq!(config.timeout_secs, 30);
    }
}
