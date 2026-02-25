//! Types for the upto payment scheme (Permit2-based variable amount settlement).
//!
//! These types represent the wire format for upto scheme requests and responses,
//! following the x402 v2 protocol specification.

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

// ============================================================================
// Constants
// ============================================================================

/// Canonical Uniswap Permit2 contract address (same on all EVM chains via CREATE2).
pub const PERMIT2_ADDRESS: Address =
    alloy::primitives::address!("0x000000000022D473030F116dDEE9F6B43aC78BA3");

/// x402 UptoPermit2Proxy contract address (vanity-mined, same on all EVM chains via CREATE2).
pub const UPTO_PERMIT2_PROXY_ADDRESS: Address =
    alloy::primitives::address!("0x4020633461b2895a48930Ff97eE8fCdE8E520002");

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
    #[serde(default)]
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
    #[serde(default)]
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
        U256::from_str_radix(&s[2..], 16)
            .map_err(|e| super::UptoError::InvalidPayload(format!("invalid hex amount '{}': {}", s, e)))
    } else {
        U256::from_str_radix(s, 10)
            .map_err(|e| super::UptoError::InvalidPayload(format!("invalid decimal amount '{}': {}", s, e)))
    }
}

/// Parse an address string into alloy Address.
pub fn parse_address(s: &str) -> Result<Address, super::UptoError> {
    s.parse::<Address>()
        .map_err(|e| super::UptoError::InvalidPayload(format!("invalid address '{}': {}", s, e)))
}
