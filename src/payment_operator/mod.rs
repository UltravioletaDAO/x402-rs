//! PaymentOperator x402r Support
//!
//! Implements support for the advanced PaymentOperator escrow system from x402r.
//! This system provides:
//! - Authorize/Capture pattern (authorize -> charge/release)
//! - Pluggable conditions (payer, receiver, arbiter, time-based)
//! - Pluggable recorders (state tracking after actions)
//! - Fee system (protocol fees + operator fees)
//! - Refunds (in-escrow and post-escrow)
//!
//! # Architecture
//!
//! The PaymentOperator system works alongside (not replacing) the simpler DepositRelay system:
//! - `extensions["refund"]` -> DepositRelay (simple escrow)
//! - `extensions["operator"]` -> PaymentOperator (advanced escrow with conditions/fees)
//!
//! # Feature Flag
//!
//! Set `ENABLE_PAYMENT_OPERATOR=true` to enable PaymentOperator settlement support.
//!
//! # Deployed Contracts (Base Sepolia)
//!
//! | Contract | Address |
//! |----------|---------|
//! | AuthCaptureEscrow | 0xb9488351E48b23D798f24e8174514F28B741Eb4f |
//! | PaymentOperatorFactory | 0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70 |
//! | ProtocolFeeConfig | 0x1e52a74cE6b69F04a506eF815743E1052A1BD28F |
//! | RefundRequest | 0x6926c05193c714ED4bA3867Ee93d6816Fdc14128 |
//! | PayerCondition | 0xBAF68176FF94CAdD403EF7FbB776bbca548AC09D |
//! | ReceiverCondition | 0x12EDefd4549c53497689067f165c0f101796Eb6D |
//! | AlwaysTrueCondition | 0x785cC83DEa3d46D5509f3bf7496EAb26D42EE610 |

pub mod abi;
pub mod addresses;
pub mod errors;
pub mod operator;
pub mod types;

pub use errors::OperatorError;
pub use operator::settle_with_operator;
pub use types::{OperatorAction, OperatorExtension, PaymentInfo};

use std::env;

/// Check if PaymentOperator feature is enabled via environment variable
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
