//! ABI bindings for the x402UptoPermit2Proxy contract.
//!
//! Generated from `abi/X402UptoPermit2Proxy.json` using Alloy's `sol!` macro.
//! Also defines inline Solidity types for the settle() call since the sol! macro
//! generates private sub-modules for imported interfaces.

use alloy::sol;

sol!(
    #[allow(missing_docs)]
    #[allow(clippy::too_many_arguments)]
    #[derive(Debug)]
    #[sol(rpc)]
    X402UptoPermit2Proxy,
    "abi/X402UptoPermit2Proxy.json"
);

/// Inline Solidity types for the settle() call.
/// We define these separately because the sol! macro generates private modules
/// for nested interface types (ISignatureTransfer, x402BasePermit2Proxy).
sol! {
    #[allow(missing_docs)]
    #[allow(clippy::too_many_arguments)]
    #[derive(Debug)]

    /// Permit2 token permissions.
    struct TokenPermissions {
        address token;
        uint256 amount;
    }

    /// Permit2 transfer authorization.
    struct PermitTransferFrom {
        TokenPermissions permitted;
        uint256 nonce;
        uint256 deadline;
    }

    /// Witness data binding the payment recipient and facilitator.
    struct Witness {
        address to;
        address facilitator;
        uint256 validAfter;
    }

    /// The settle() function signature on X402UptoPermit2Proxy.
    function settle(
        PermitTransferFrom permit,
        uint256 amount,
        address owner,
        Witness witness,
        bytes signature
    ) external;

    /// ERC-20 allowance and balance checks.
    function allowance(address owner, address spender) external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
}
