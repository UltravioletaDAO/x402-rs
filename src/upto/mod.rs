//! x402 Upto Payment Scheme Support (Permit2-based variable amount settlement)
//!
//! The `upto` scheme allows clients to authorize a **maximum** payment amount via Permit2,
//! and servers settle for the **actual** amount consumed (<= max).
//!
//! This is ideal for usage-based pricing: LLM token generation, bandwidth metering, etc.
//!
//! # How It Works
//!
//! 1. Client signs a Permit2 `permitWitnessTransferFrom` for the max amount
//! 2. Facilitator verifies the signature, Permit2 allowance, and balance
//! 3. Server determines actual usage cost
//! 4. Facilitator settles via `x402UptoPermit2Proxy.settle(permit, actualAmount, ...)`
//!
//! Unlike EIP-3009 (used by `exact` scheme), Permit2 allows settling for less
//! than the signed amount because the `requestedAmount` is a parameter of the
//! settlement call, not part of the signed message.
//!
//! # Feature Flag
//!
//! Set `ENABLE_UPTO=true` to enable upto scheme support.

pub mod abi;
pub mod errors;
pub mod permit2;
pub mod types;

pub use errors::UptoError;
pub use permit2::{settle_upto, verify_upto};

use std::env;

/// Upto scheme identifier
pub const UPTO_SCHEME: &str = "upto";

/// Check if upto scheme is enabled via ENABLE_UPTO env var
pub fn is_enabled() -> bool {
    env::var("ENABLE_UPTO")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_enabled() {
        // Default should be disabled
        env::remove_var("ENABLE_UPTO");
        assert!(!is_enabled());

        // Test enabling
        unsafe { env::set_var("ENABLE_UPTO", "true") };
        assert!(is_enabled());

        // Test with "1"
        unsafe { env::set_var("ENABLE_UPTO", "1") };
        assert!(is_enabled());

        // Test case insensitive
        unsafe { env::set_var("ENABLE_UPTO", "TRUE") };
        assert!(is_enabled());

        // Test disabled
        unsafe { env::set_var("ENABLE_UPTO", "false") };
        assert!(!is_enabled());

        // Cleanup
        env::remove_var("ENABLE_UPTO");
    }
}
