//! Enhanced debug utilities for the facilitator.
//!
//! This module contains helper functions for debug logging that can be enabled/disabled
//! via the `FACILITATOR_ENHANCED_DEBUG` environment variable.

use alloy::primitives::U256;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::env;

/// Check if enhanced debug logging is enabled.
///
/// Returns `true` if the `FACILITATOR_ENHANCED_DEBUG` environment variable is set to "true" (case-insensitive).
/// Returns `false` otherwise.
pub fn is_enhanced_debug_enabled() -> bool {
    env::var("FACILITATOR_ENHANCED_DEBUG")
        .unwrap_or_else(|_| "true".to_string()) // Default to true for now
        .eq_ignore_ascii_case("true")
}

/// Format USDC amount from micro-units (6 decimals) to human-readable dollar amount.
///
/// # Examples
/// ```
/// use alloy::primitives::U256;
/// assert_eq!(format_usdc_amount(U256::from(10000u64)), "$0.010000");
/// assert_eq!(format_usdc_amount(U256::from(1000000u64)), "$1.000000");
/// ```
pub fn format_usdc_amount(micro_units: U256) -> String {
    match TryInto::<u64>::try_into(micro_units) {
        Ok(amount_u64) => {
            let usdc = amount_u64 as f64 / 1_000_000.0;
            format!("${:.6}", usdc)
        }
        Err(_) => {
            // Handle very large numbers
            format!("{} micro-units (too large to display)", micro_units)
        }
    }
}

/// Convert Unix timestamp to human-readable RFC3339 format.
///
/// # Examples
/// ```
/// assert_eq!(
///     timestamp_to_readable(1609459200),
///     "2021-01-01T00:00:00+00:00"
/// );
/// ```
pub fn timestamp_to_readable(unix_timestamp: u64) -> String {
    NaiveDateTime::from_timestamp_opt(unix_timestamp as i64, 0)
        .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc).to_rfc3339())
        .unwrap_or_else(|| "Invalid timestamp".to_string())
}

/// Try to extract a revert reason from an error message.
///
/// This is a best-effort function that looks for common patterns in error messages.
pub fn extract_revert_reason(error: &str) -> Option<String> {
    // Common patterns in contract revert errors
    if error.contains("revert") {
        // Try to extract the reason after "revert"
        if let Some(start) = error.find("revert") {
            let after_revert = &error[start + 6..];
            // Find the end of the reason (typically a quote, newline, or end of string)
            let end = after_revert
                .find(|c: char| c == '"' || c == '\n' || c == ',')
                .unwrap_or(after_revert.len().min(100));
            let reason = after_revert[..end].trim();
            if !reason.is_empty() {
                return Some(reason.to_string());
            }
        }
    }

    // Check for specific error patterns
    if error.contains("insufficient") {
        return Some("Insufficient balance or allowance".to_string());
    }
    if error.contains("nonce") && error.contains("used") {
        return Some("Nonce already used".to_string());
    }
    if error.contains("signature") && error.contains("invalid") {
        return Some("Invalid signature".to_string());
    }
    if error.contains("expired") {
        return Some("Authorization expired".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_usdc_amount() {
        assert_eq!(format_usdc_amount(U256::from(10000u64)), "$0.010000");
        assert_eq!(format_usdc_amount(U256::from(1000000u64)), "$1.000000");
        assert_eq!(format_usdc_amount(U256::from(0u64)), "$0.000000");
    }

    #[test]
    fn test_timestamp_to_readable() {
        // 2021-01-01 00:00:00 UTC
        let readable = timestamp_to_readable(1609459200);
        assert!(readable.starts_with("2021-01-01"));
    }

    #[test]
    fn test_extract_revert_reason() {
        assert_eq!(
            extract_revert_reason("Contract call reverted with insufficient balance"),
            Some("Insufficient balance or allowance".to_string())
        );
        assert_eq!(
            extract_revert_reason("Error: nonce already used"),
            Some("Nonce already used".to_string())
        );
        assert_eq!(extract_revert_reason("Unknown error"), None);
    }
}
