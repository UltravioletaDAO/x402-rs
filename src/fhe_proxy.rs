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

/// Default FHE request timeout, in seconds.
///
/// This mirrors `fhe_request_timeout_secs` in
/// `terraform/environments/zama-testnet/variables.tf`, which is the single
/// source of truth for the whole chain (Lambda, API Gateway, the duration
/// alarm, and this proxy). The two states are separate, so production sets
/// [`ENV_TIMEOUT_SECS`] from its own mirror of that variable and this constant
/// is only the fallback when the environment says nothing.
///
/// Do not change it here alone: three surfaces used to disagree (30s in
/// Terraform, 60s in a comment, 90s in the client) and the proxy sat waiting
/// on a Lambda AWS had already killed.
const DEFAULT_TIMEOUT_SECS: u64 = 90;

/// Environment variable carrying the effective timeout, set by Terraform.
const ENV_TIMEOUT_SECS: &str = "FHE_PROXY_TIMEOUT_SECS";

/// Accepted range. The upper bound is the AWS Lambda maximum; the lower bound
/// keeps a typo like `1` from turning every FHE call into an instant timeout.
const MIN_TIMEOUT_SECS: u64 = 3;
const MAX_TIMEOUT_SECS: u64 = 900;

/// Read the timeout from the environment, falling back to the default.
///
/// A bad value warns and falls back; it never panics. This is built during
/// router construction, and an unparseable environment variable must not take
/// the whole facilitator down over a feature that is off for most callers.
/// The range is checked as well as the type: `9000` parses cleanly and would
/// silently outlive every upstream hop.
fn timeout_secs_from_env() -> u64 {
    let raw = match std::env::var(ENV_TIMEOUT_SECS) {
        Ok(v) => v,
        Err(_) => return DEFAULT_TIMEOUT_SECS,
    };

    match raw.trim().parse::<u64>() {
        Ok(secs) if (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs) => secs,
        Ok(secs) => {
            warn!(
                value = secs,
                min = MIN_TIMEOUT_SECS,
                max = MAX_TIMEOUT_SECS,
                default = DEFAULT_TIMEOUT_SECS,
                "{ENV_TIMEOUT_SECS} out of range, using default"
            );
            DEFAULT_TIMEOUT_SECS
        }
        Err(_) => {
            warn!(
                value = %raw,
                default = DEFAULT_TIMEOUT_SECS,
                "{ENV_TIMEOUT_SECS} is not a number, using default"
            );
            DEFAULT_TIMEOUT_SECS
        }
    }
}

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
            // FHE decryption via the Zama relayer is slower than an ordinary RPC
            // call, so this is generous on purpose. Sourced from Terraform.
            timeout_secs: timeout_secs_from_env(),
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

        // Log the EFFECTIVE timeout, not just the endpoint: an override that
        // cannot be read back in a running task is invisible drift.
        info!(
            endpoint = %config.endpoint,
            timeout_secs = config.timeout_secs,
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
    pub async fn verify(
        &self,
        body: &serde_json::Value,
    ) -> Result<FheVerifyResponse, FheProxyError> {
        let url = format!("{}/verify", self.config.endpoint);
        info!(url = %url, "Forwarding verify request to FHE facilitator");

        let response = self.client.post(&url).json(body).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!(
            status = %status,
            body_len = response_text.len(),
            "Received response from FHE facilitator"
        );

        if status.is_success() {
            serde_json::from_str(&response_text).map_err(|e| {
                FheProxyError::InvalidResponse(format!(
                    "Failed to parse verify response: {} - body: {}",
                    e, response_text
                ))
            })
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
    pub async fn settle(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, FheProxyError> {
        let url = format!("{}/settle", self.config.endpoint);
        info!(url = %url, "Forwarding settle request to FHE facilitator");

        let response = self.client.post(&url).json(body).send().await?;

        let status = response.status();
        let response_text = response.text().await?;

        debug!(
            status = %status,
            body_len = response_text.len(),
            "Received response from FHE facilitator"
        );

        if status.is_success() {
            serde_json::from_str(&response_text).map_err(|e| {
                FheProxyError::InvalidResponse(format!(
                    "Failed to parse settle response: {} - body: {}",
                    e, response_text
                ))
            })
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

    /// Run `f` with `ENV_TIMEOUT_SECS` set to `value` (or unset for `None`),
    /// then restore whatever was there. CI runs this suite with
    /// `--test-threads=1`, which is what makes touching process env safe here.
    fn with_timeout_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var(ENV_TIMEOUT_SECS).ok();
        match value {
            Some(v) => std::env::set_var(ENV_TIMEOUT_SECS, v),
            None => std::env::remove_var(ENV_TIMEOUT_SECS),
        }
        let out = f();
        match previous {
            Some(v) => std::env::set_var(ENV_TIMEOUT_SECS, v),
            None => std::env::remove_var(ENV_TIMEOUT_SECS),
        }
        out
    }

    #[test]
    fn test_default_config() {
        let config = with_timeout_env(None, FheProxyConfig::default);
        assert!(config.endpoint.contains("zama-facilitator"));
        // FHE decryption takes longer; 90s accounts for relayer + cold starts
        assert_eq!(config.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(DEFAULT_TIMEOUT_SECS, 90);
    }

    #[test]
    fn test_timeout_read_from_env() {
        assert_eq!(with_timeout_env(Some("120"), timeout_secs_from_env), 120);
        // Surrounding whitespace is a Terraform/shell artifact, not a typo.
        assert_eq!(with_timeout_env(Some(" 45 "), timeout_secs_from_env), 45);
    }

    /// A junk override must not take the service down, and must not be
    /// silently honoured either: both paths fall back to the default.
    #[test]
    fn test_timeout_rejects_garbage_and_out_of_range() {
        for bad in ["not-a-number", "", "-5", "90s"] {
            assert_eq!(
                with_timeout_env(Some(bad), timeout_secs_from_env),
                DEFAULT_TIMEOUT_SECS,
                "unparseable value {bad:?} should fall back to the default"
            );
        }

        // Parses cleanly, but 0 would make every FHE call fail instantly and
        // 9000 would outlive every hop upstream. Range is checked, not just type.
        for bad in ["0", "1", "9000"] {
            assert_eq!(
                with_timeout_env(Some(bad), timeout_secs_from_env),
                DEFAULT_TIMEOUT_SECS,
                "out-of-range value {bad:?} should fall back to the default"
            );
        }

        // The bounds themselves are accepted.
        assert_eq!(with_timeout_env(Some("3"), timeout_secs_from_env), 3);
        assert_eq!(with_timeout_env(Some("900"), timeout_secs_from_env), 900);
    }
}
