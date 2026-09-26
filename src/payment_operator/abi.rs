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

/// PaymentOperator ABI of the operators the canonical v1.0.2 factory deploys
/// (Arc, Arc testnet): `capture` / `void` / `refund` instead of `release` /
/// `refundInEscrow`, and `FEE_RECEIVER()` instead of `FEE_RECIPIENT()` as the
/// owner getter. `authorize` keeps the selector every generation shares.
///
/// `abi/PaymentOperatorV3.json` is `forge inspect PaymentOperator abi` at
/// BackTrackCo/x402r-contracts@8345776e (`src/operator/payment/PaymentOperator.sol`),
/// filtered to `authorize`, `capture`, `void`, `refund`, `ESCROW`,
/// `FEE_RECEIVER` and the `*_PRE_ACTION_CONDITION` getters. The ABI above
/// is left as it is: SKALE and the legacy networks depend on it.
mod operator_v3_abi {
    use alloy::sol;

    sol!(
        #[allow(missing_docs)]
        #[derive(Debug)]
        OperatorV3Contract,
        "abi/PaymentOperatorV3.json"
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
pub use operator_v3_abi::OperatorV3Contract;

/// PaymentInfo type from the OperatorV3Contract ABI scope; same fields, a
/// third Rust type (see [`EscrowPaymentInfo`]).
pub use operator_v3_abi::AuthCaptureEscrow::PaymentInfo as OperatorV3PaymentInfo;

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

    /// The selectors the v3 binding encodes are the ones measured in the
    /// v1.0.2 factory bytecode and in PaymentOperator.sol, and `authorize` is
    /// the one every generation shares.
    #[test]
    fn v3_selectors_are_the_generation_d_ones() {
        use alloy::sol_types::SolCall;
        assert_eq!(
            OperatorV3Contract::captureCall::SELECTOR,
            [0xf1, 0x2b, 0x86, 0xf6]
        );
        assert_eq!(
            OperatorV3Contract::voidCall::SELECTOR,
            [0xc3, 0xc5, 0x09, 0x0e]
        );
        assert_eq!(
            OperatorV3Contract::FEE_RECEIVERCall::SELECTOR,
            [0xd3, 0xe7, 0x8e, 0x4d]
        );
        assert_eq!(
            OperatorV3Contract::authorizeCall::SELECTOR,
            OperatorContract::authorizeCall::SELECTOR
        );
        assert_eq!(
            OperatorV3Contract::ESCROWCall::SELECTOR,
            OperatorContract::ESCROWCall::SELECTOR
        );
        // The getter the legacy/CREATE3 operators expose does not exist on v3.
        assert_ne!(
            OperatorV3Contract::FEE_RECEIVERCall::SELECTOR,
            OperatorContract::FEE_RECIPIENTCall::SELECTOR
        );
    }
}
