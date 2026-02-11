//! ABI bindings for PaymentOperator contracts using Alloy sol! macro
//!
//! These bindings are generated from the contract ABIs in the abi/ directory.
//!
//! Each sol! invocation lives in its own sub-module to prevent AuthCaptureEscrow
//! module name collisions (both ABIs reference the same PaymentInfo struct type).
//!
//! Usage:
//! - `OperatorContract::authorizeCall` for building authorize/release/refund calls
//! - `EscrowContract::getHashCall` / `paymentStateCall` for state queries
//! - Access PaymentInfo via: `PaymentInfo` (re-exported at module level)

/// PaymentOperator ABI bindings (authorize, release, refundInEscrow, charge, etc.)
mod operator_abi {
    use alloy::sol;

    sol!(
        #[allow(missing_docs)]
        #[derive(Debug)]
        #[sol(rpc)]
        OperatorContract,
        "abi/PaymentOperator.json"
    );
}

/// AuthCaptureEscrow ABI bindings (getHash, paymentState, etc.)
mod escrow_abi {
    use alloy::sol;

    sol!(
        #[allow(missing_docs)]
        #[derive(Debug)]
        #[sol(rpc)]
        EscrowContract,
        "abi/AuthCaptureEscrow.json"
    );
}

// Re-export contract types at this module level
pub use escrow_abi::EscrowContract;
pub use operator_abi::AuthCaptureEscrow::PaymentInfo;
pub use operator_abi::OperatorContract;

/// PaymentInfo type from the EscrowContract ABI scope.
/// Structurally identical to `PaymentInfo` but a different Rust type because
/// it comes from a separate sol! invocation. Used for EscrowContract calls.
pub use escrow_abi::AuthCaptureEscrow::PaymentInfo as EscrowPaymentInfo;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, Uint, U256};

    #[test]
    fn test_payment_info_struct() {
        // Verify the generated struct has all expected fields
        // Note: maxAmount is Uint<120, 2>, expiry timestamps are Uint<48, 1>
        let _info = PaymentInfo {
            operator: address!("0000000000000000000000000000000000000001"),
            payer: address!("0000000000000000000000000000000000000002"),
            receiver: address!("0000000000000000000000000000000000000003"),
            token: address!("0000000000000000000000000000000000000004"),
            maxAmount: Uint::from(1000000u128),
            preApprovalExpiry: Uint::from(1738400000u64),
            authorizationExpiry: Uint::from(1738500000u64),
            refundExpiry: Uint::from(1738600000u64),
            minFeeBps: 0,
            maxFeeBps: 100,
            feeReceiver: address!("0000000000000000000000000000000000000005"),
            salt: U256::from(12345),
        };
    }
}
