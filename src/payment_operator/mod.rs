//! x402r Escrow / Commerce Scheme Support
//!
//! Implements the escrow payment scheme from x402r using Base Commerce Payments contracts.
//! Both `"escrow"` and `"commerce"` scheme identifiers are accepted (functionally identical).
//! The `"commerce"` alias was introduced by x402r for marketplace integrations.
//!
//! # How It Works
//!
//! The escrow scheme uses `scheme: "escrow"` or `scheme: "commerce"` in the payment payload.
//! When a client sends a payment with either scheme, the facilitator:
//!
//! 1. Verifies the ERC-3009 signature
//! 2. Calls `PaymentOperator.authorize()` to place funds in escrow
//!
//! `release` and `refundInEscrow` are served here too (`operator.rs`); they take no
//! ERC-3009 signature because the funds are already escrowed, and are instead gated
//! by a signed order under `ESCROW_LIFECYCLE_AUTH` (see `lifecycle_auth`). `charge`
//! and `refundPostEscrow` are not implemented.
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
//! `addresses` is the source; these tables repeat two of its entries.
//!
//! ## Base Sepolia (eip155:84532)
//!
//! | Contract | Address |
//! |----------|---------|
//! | AuthCaptureEscrow | 0x29025c0E9D4239d438e169570818dB9FE0A80873 |
//! | PaymentOperatorFactory | 0x97d53e63A9CB97556c00BeFd325AF810c9b267B2 |
//! | TokenCollector | 0x5cA789000070DF15b4663DB64a50AeF5D49c5Ee0 |
//! | ProtocolFeeConfig | 0x8F96C493bAC365E41f0315cf45830069EBbDCaCe |
//!
//! ## Base Mainnet (eip155:8453)
//!
//! | Contract | Address |
//! |----------|---------|
//! | AuthCaptureEscrow | 0xb9488351E48b23D798f24e8174514F28B741Eb4f |
//! | PaymentOperatorFactory | 0x3D0837fF8Ea36F417261577b9BA568400A840260 |
//! | TokenCollector | 0x48ADf6E37F9b31dC2AAD0462C5862B5422C736B8 |
//! | ProtocolFeeConfig | 0x59314674BAbb1a24Eb2704468a9cCdD50668a1C6 |
//!
//! ## Arc (eip155:5042) and Arc testnet (eip155:5042002)
//!
//! The canonical commerce-payments v1.0.0 set (`addresses::canonical_v1`),
//! whose operators `capture` and `void` instead of `release` and
//! `refundInEscrow` (`operator::OperatorAbi::V3`).
//!
//! | Contract | Address |
//! |----------|---------|
//! | AuthCaptureEscrow | 0xBdEA0D1bcC5966192B070Fdf62aB4EF5b4420cff |
//! | PaymentOperatorFactory v1.0.2 | 0xc24153B7ED8DC03e551F29DDEeA5CadFe57e2716 |
//! | ERC3009TokenCollector | 0x0E3dF9510de65469C4518D7843919c0b8C7A7757 |
//! | ProtocolFeeConfig | 0xBe2d24614F339a1eB103A399F93AA2a39Ca815Bc |
//!
//! # Reference Implementation
//!
//! Based on: https://github.com/BackTrackCo/x402r-scheme

pub mod abi;
pub mod addresses;
#[cfg(test)]
mod arc_chain_tests;
pub mod autoverify;
pub mod errors;
pub mod lifecycle_auth;
pub mod operator;
#[cfg(test)]
pub(crate) mod test_rpc;
pub mod types;

pub use errors::OperatorError;
pub use operator::{
    is_escrow_scheme, query_escrow_state, settle_escrow, verify_escrow, COMMERCE_SCHEME,
    ESCROW_SCHEME,
};
pub use types::{
    ContractPaymentInfo, EscrowAuthorization, EscrowExtra, EscrowLifecyclePayload, EscrowPayload,
    EscrowPaymentInfo, EscrowStateQuery, EscrowStateResponse,
};

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
