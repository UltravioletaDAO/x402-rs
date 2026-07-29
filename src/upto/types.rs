//! Types for the upto payment scheme (Permit2-based variable amount settlement).
//!
//! These types represent the wire format for upto scheme requests and responses,
//! following the x402 v2 protocol specification.

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

use crate::network::Network;

// ============================================================================
// Constants
// ============================================================================

/// Canonical Uniswap Permit2 contract address (same on all EVM chains via CREATE2).
pub const PERMIT2_ADDRESS: Address =
    alloy::primitives::address!("0x000000000022D473030F116dDEE9F6B43aC78BA3");

/// x402 UptoPermit2Proxy contract address (vanity-mined, same on all EVM chains via CREATE2).
///
/// This is the canonical address pinned by the upstream x402 spec
/// (specs/schemes/upto/scheme_upto_evm.md) and exported by @x402/evm as
/// `x402UptoPermit2ProxyAddress`. Deployed via Arachnid's deterministic
/// deployment proxy (0x4e59b44847b379578588920cA78FbF26c0B4956C); bytecode
/// verified byte-identical on Base, Ethereum, Arbitrum, World Chain, Monad,
/// and Robinhood Chain (Sourcify match on Base, solc 0.8.28, Cancun).
///
/// NOTE: the previous value 0x4020633461b2895a48930Ff97eE8fCdE8E520002 had no
/// code on ANY chain (miscopied at implementation time), which made every
/// upto settlement target an empty address.
pub const UPTO_PERMIT2_PROXY_ADDRESS: Address =
    alloy::primitives::address!("0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002");

/// Networks where [`UPTO_PERMIT2_PROXY_ADDRESS`] actually has code.
///
/// The address is the same on every chain (deterministic CREATE2), but being
/// deterministic is not the same as being deployed: the deployment has to be
/// replayed per chain, and on five of ours it never was. `/supported` used to
/// advertise `upto` on every EVM network that supported `exact`, so it promised
/// a scheme that could only ever fail there — and failing at settle time means
/// the client has already signed a Permit2 authorization.
///
/// Verified by `eth_getCode` on 2026-07-29, every entry, 3142 bytes each.
/// Measured absent: avalanche, celo, scroll, unichain, optimism-sepolia.
///
/// # Why a list and not a probe
///
/// Probing at startup looks more honest and is less reliable. Measuring polygon
/// during this audit returned NO CODE from `polygon-rpc.com` and from Ankr, and
/// the correct 3142 bytes from PublicNode — same address, same block height,
/// three answers. A probe would have silently dropped a working network from
/// `/supported` depending on which node happened to answer. The settle path
/// keeps its own `assert_proxy_deployed` guard, so a stale entry here degrades
/// to a clear rejection rather than a transfer into an empty address.
///
/// # When adding a network
///
/// Check the proxy with `eth_getCode` against **two independent RPCs** before
/// adding it. One endpoint is not a measurement.
pub const UPTO_DEPLOYED_NETWORKS: &[Network] = &[
    Network::Base,
    Network::Optimism,
    Network::Arbitrum,
    Network::Polygon,
    Network::Bsc,
    Network::Ethereum,
    Network::HyperEvm,
    Network::Monad,
    Network::BaseSepolia,
    Network::AvalancheFuji,
    Network::ArbitrumSepolia,
];

/// Whether the `upto` scheme can actually settle on `network`.
///
/// See [`UPTO_DEPLOYED_NETWORKS`] for how the list is established and why it is
/// not a runtime probe.
pub fn is_proxy_deployed_on(network: Network) -> bool {
    UPTO_DEPLOYED_NETWORKS.contains(&network)
}

// ============================================================================
// Permit2 Wire Types (deserialized from JSON payload)
// ============================================================================

/// Token permissions in the Permit2 authorization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permit2TokenPermissions {
    /// ERC-20 token contract address.
    pub token: String,
    /// Maximum amount authorized (in atomic token units, as string).
    pub amount: String,
}

/// Witness data binding the payment recipient.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permit2Witness {
    /// Recipient address (cryptographically bound in signature).
    pub to: String,
    /// Address authorized to settle (must be the facilitator/proxy).
    #[serde(default)]
    pub facilitator: Option<String>,
    /// Earliest timestamp when payment can be settled.
    pub valid_after: String,
    /// Extra data (ABI-encoded, usually empty).
    #[serde(
        default,
        deserialize_with = "crate::json_depth::deserialize_bounded_extra"
    )]
    pub extra: Option<serde_json::Value>,
}

/// Permit2 authorization details (from the client's signed message).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permit2Authorization {
    /// Expiration timestamp for the authorization.
    pub deadline: String,
    /// Payer address (token owner).
    pub from: String,
    /// Unique nonce (hex-encoded 32 bytes) to prevent replay.
    pub nonce: String,
    /// Token and max amount.
    pub permitted: Permit2TokenPermissions,
    /// Address authorized to spend (must be UPTO_PERMIT2_PROXY_ADDRESS).
    pub spender: String,
    /// Witness data binding recipient.
    pub witness: Permit2Witness,
}

/// Complete Permit2 payload (inside the "payload" field of the payment).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permit2Payload {
    /// The Permit2 authorization parameters.
    pub permit_2_authorization: Permit2Authorization,
    /// The client's EIP-712 signature over the authorization.
    pub signature: String,
}

// ============================================================================
// Upto Request Types
// ============================================================================

/// Payment requirements for the upto scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoPaymentRequirements {
    /// Must be "upto".
    pub scheme: String,
    /// Blockchain network in CAIP-2 format (e.g., "eip155:8453").
    pub network: String,
    /// Maximum amount (for verify) or actual amount (for settle), as string.
    pub amount: String,
    /// Token contract address.
    pub asset: String,
    /// Recipient wallet address.
    pub pay_to: String,
    /// Maximum time allowed for payment completion.
    #[serde(default)]
    pub max_timeout_seconds: Option<u64>,
    /// Extra scheme-specific data.
    #[serde(
        default,
        deserialize_with = "crate::json_depth::deserialize_bounded_extra"
    )]
    pub extra: Option<serde_json::Value>,
}

/// The accepted requirements + payload from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoPaymentPayload {
    /// The requirements the client accepted.
    pub accepted: UptoPaymentRequirements,
    /// The Permit2 payment payload.
    pub payload: Permit2Payload,
    /// Resource being paid for.
    #[serde(default)]
    pub resource: Option<serde_json::Value>,
    /// Protocol version.
    #[serde(default)]
    pub x402_version: Option<u8>,
}

/// Full upto verify/settle request envelope (v2 format).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoRequest {
    /// Protocol version (must be 2).
    #[serde(default)]
    pub x402_version: Option<u8>,
    /// The payment payload from the client.
    pub payment_payload: UptoPaymentPayload,
    /// The server's payment requirements.
    pub payment_requirements: UptoPaymentRequirements,
}

// ============================================================================
// Upto Response Types
// ============================================================================

/// Settlement response for the upto scheme.
///
/// Extends the standard settle response with the `amount` field
/// indicating the actual amount charged (may be less than authorized max).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoSettleResponse {
    /// Whether settlement succeeded.
    pub success: bool,
    /// Error reason if settlement failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// Payer address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// Transaction hash (empty string for $0 settlements).
    pub transaction: String,
    /// Network identifier (CAIP-2 format).
    pub network: String,
    /// Actual amount charged (in atomic token units, as string).
    /// This is the key difference from the exact scheme response.
    pub amount: String,
}

/// Verify response for the upto scheme.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UptoVerifyResponse {
    /// Whether the payment authorization is valid.
    pub is_valid: bool,
    /// Reason for invalidity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    /// Payer address (if valid).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
}

// ============================================================================
// Helper: Parse U256 from string
// ============================================================================

/// Parse a string amount (decimal or hex) into alloy U256.
pub fn parse_amount(s: &str) -> Result<U256, super::UptoError> {
    if s.starts_with("0x") || s.starts_with("0X") {
        U256::from_str_radix(&s[2..], 16).map_err(|e| {
            super::UptoError::InvalidPayload(format!("invalid hex amount '{}': {}", s, e))
        })
    } else {
        U256::from_str_radix(s, 10).map_err(|e| {
            super::UptoError::InvalidPayload(format!("invalid decimal amount '{}': {}", s, e))
        })
    }
}

/// Parse an address string into alloy Address.
pub fn parse_address(s: &str) -> Result<Address, super::UptoError> {
    s.parse::<Address>()
        .map_err(|e| super::UptoError::InvalidPayload(format!("invalid address '{}': {}", s, e)))
}

#[cfg(test)]
mod proxy_deployment_tests {
    use super::*;

    /// The five networks `/supported` used to lie about.
    ///
    /// Measured absent with `eth_getCode` against two independent RPCs each on
    /// 2026-07-29. Advertising `upto` here promised a scheme that could only
    /// fail — and fail *after* the client signed a Permit2 authorization.
    #[test]
    fn networks_without_the_proxy_are_excluded() {
        for network in [
            Network::Avalanche,
            Network::Celo,
            Network::Scroll,
            Network::Unichain,
            Network::OptimismSepolia,
        ] {
            assert!(
                !is_proxy_deployed_on(network),
                "{network} has no proxy code but would be advertised as upto-capable"
            );
        }
    }

    /// The eleven where the proxy really is deployed, 3142 bytes each. Excluding
    /// one of these would silently remove working functionality, which is the
    /// opposite failure and just as quiet.
    #[test]
    fn networks_with_the_proxy_are_included() {
        for network in [
            Network::Base,
            Network::Optimism,
            Network::Arbitrum,
            Network::Polygon,
            Network::Bsc,
            Network::Ethereum,
            Network::HyperEvm,
            Network::Monad,
            Network::BaseSepolia,
            Network::AvalancheFuji,
            Network::ArbitrumSepolia,
        ] {
            assert!(
                is_proxy_deployed_on(network),
                "{network} has verified proxy code but would not be advertised"
            );
        }
    }

    /// Polygon earns its own test.
    ///
    /// During the audit it returned NO CODE from polygon-rpc.com and from Ankr,
    /// and the correct 3142 bytes from PublicNode. Whoever next sees a "no code"
    /// reading for polygon should get a second opinion before deleting this
    /// entry — that is exactly the trap this test exists to hold open.
    #[test]
    fn polygon_is_deployed_despite_rpcs_that_say_otherwise() {
        assert!(is_proxy_deployed_on(Network::Polygon));
    }

    /// A testnet being absent says nothing about its mainnet, and vice versa.
    /// Avalanche is the live example: Fuji has the proxy, mainnet does not.
    #[test]
    fn testnet_and_mainnet_are_independent() {
        assert!(is_proxy_deployed_on(Network::AvalancheFuji));
        assert!(!is_proxy_deployed_on(Network::Avalanche));
        assert!(is_proxy_deployed_on(Network::Optimism));
        assert!(!is_proxy_deployed_on(Network::OptimismSepolia));
    }
}
