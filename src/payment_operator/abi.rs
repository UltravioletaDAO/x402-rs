//! ABI bindings for PaymentOperator contracts using Alloy sol! macro
//!
//! These bindings are generated from the contract ABIs in the abi/ directory.
//!
//! Usage:
//! - `OperatorContract::authorizeCall` for building authorize calls
//! - Access PaymentInfo via: `PaymentInfo` (re-exported at module level)

use alloy::sol;

// PaymentOperator binding from ABI file
// This includes nested AuthCaptureEscrow types (PaymentInfo struct)
// The sol! macro generates AuthCaptureEscrow as a top-level module in this file
sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    OperatorContract,
    "abi/PaymentOperator.json"
);

// The sol! macro creates AuthCaptureEscrow at the top level of this module
// We can re-export its types directly
pub use AuthCaptureEscrow::PaymentInfo;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, U256, Uint};

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
