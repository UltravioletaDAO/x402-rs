//! x402r Escrow Scheme Support
//!
//! Implements the escrow payment scheme from x402r using Base Commerce Payments contracts.
//!
//! # How It Works
//!
//! The escrow scheme uses `scheme: "escrow"` in the payment payload (not extensions).
//! When a client sends a payment with this scheme, the facilitator:
//!
//! 1. Verifies the ERC-3009 signature
//! 2. Calls `PaymentOperator.authorize()` to place funds in escrow
//!
//! That's it - the facilitator ONLY handles authorize. Other actions like charge,
//! release, and refunds are handled by the resource server or other systems.
//!
//! # Request Format
//!
//! ```json
//! {
//!   "x402Version": 2,
//!   "scheme": "escrow",
//!   "payload": {
//!     "authorization": { "from": "0x...", "to": "0x...", "value": "...", ... },
//!     "signature": "0x...",
//!     "paymentInfo": { "operator": "0x...", "receiver": "0x...", ... }
//!   },
//!   "paymentRequirements": {
//!     "scheme": "escrow",
//!     "network": "eip155:8453",
//!     "extra": {
//!       "escrowAddress": "0x...",
//!       "operatorAddress": "0x...",
//!       "tokenCollector": "0x..."
//!     }
//!   }
//! }
//! ```
//!
//! # Feature Flag
//!
//! Set `ENABLE_PAYMENT_OPERATOR=true` to enable escrow scheme support.
//!
//! # Deployed Contracts
//!
//! ## Base Sepolia (eip155:84532)
//!
//! | Contract | Address |
//! |----------|---------|
//! | AuthCaptureEscrow | 0xb9488351E48b23D798f24e8174514F28B741Eb4f |
//! | PaymentOperatorFactory | 0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70 |
//! | ERC3009TokenCollector | 0x0E3dF9510de65469C4518D7843919c0b8C7A7757 |
//! | ProtocolFeeConfig | 0x1e52a74cE6b69F04a506eF815743E1052A1BD28F |
//!
//! ## Base Mainnet (eip155:8453)
//!
//! Coming soon - waiting for Ali to deploy.
//!
//! # Reference Implementation
//!
//! Based on: https://github.com/BackTrackCo/x402r-scheme

pub mod abi;
pub mod addresses;
pub mod errors;
pub mod operator;
pub mod types;

pub use errors::OperatorError;
pub use operator::{settle_escrow, ESCROW_SCHEME};
pub use types::{ContractPaymentInfo, EscrowAuthorization, EscrowExtra, EscrowPayload, EscrowPaymentInfo};

use std::env;

/// Check if escrow scheme (PaymentOperator) is enabled via environment variable
pub fn is_enabled() -> bool {
    env::var("ENABLE_PAYMENT_OPERATOR")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_enabled() {
        // Default should be disabled
        env::remove_var("ENABLE_PAYMENT_OPERATOR");
        assert!(!is_enabled());

        // Test enabling
        env::set_var("ENABLE_PAYMENT_OPERATOR", "true");
        assert!(is_enabled());

        env::set_var("ENABLE_PAYMENT_OPERATOR", "TRUE");
        assert!(is_enabled());

        env::set_var("ENABLE_PAYMENT_OPERATOR", "1");
        assert!(is_enabled());

        // Test disabling
        env::set_var("ENABLE_PAYMENT_OPERATOR", "false");
        assert!(!is_enabled());

        env::set_var("ENABLE_PAYMENT_OPERATOR", "0");
        assert!(!is_enabled());

        // Cleanup
        env::remove_var("ENABLE_PAYMENT_OPERATOR");
    }
}
