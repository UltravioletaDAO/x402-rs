//! Type definitions for x402 protocol version 2.
//!
//! This module provides v2 types that use CAIP-2 network identifiers and
//! support the new protocol structure with ResourceInfo separated from
//! PaymentRequirements.
//!
//! Key changes from v1:
//! - Network identifiers use CAIP-2 format (e.g., "eip155:8453" instead of "base")
//! - ResourceInfo is now a separate top-level field
//! - PaymentRequirements renamed fields (maxAmountRequired -> amount)
//! - Support for protocol extensions
//!
//! # Migration
//!
//! During the transition period, both v1 and v2 payloads are supported through
//! envelope types that auto-detect the version and route appropriately.

use alloy::hex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

use crate::caip2::{Caip2NetworkId, Namespace};
use crate::network::Network;
use crate::timestamp::UnixTimestamp;
use crate::types::{
    EvmAddress, EvmSignature, ExactEvmPayload, ExactEvmPayloadAuthorization, ExactPaymentPayload,
    FacilitatorErrorReason, HexEncodedNonce, MixedAddress, PaymentPayload, PaymentRequirements,
    Scheme, SupportedPaymentKindExtra, SupportedPaymentKindsResponse, TokenAmount, VerifyRequest,
    X402Version,
};

// ============================================================================
// ResourceInfo - New in v2
// ============================================================================

/// Information about the resource requiring payment.
///
/// Introduced in x402 v2 to separate resource metadata from payment requirements.
/// Previously these fields were embedded in PaymentRequirements.
///
/// # References
/// - x402 v2 spec: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    /// The URL of the protected resource
    pub url: Url,

    /// Human-readable description of the resource
    pub description: String,

    /// MIME type of the resource (e.g., "application/json", "text/html")
    pub mime_type: String,
}

impl ResourceInfo {
    /// Create a new ResourceInfo
    pub fn new(url: Url, description: String, mime_type: String) -> Self {
        Self {
            url,
            description,
            mime_type,
        }
    }
}

// ============================================================================
// PaymentRequirementsV2 - Simplified from v1
// ============================================================================

/// Payment requirements for x402 v2.
///
/// Simplified from v1 - resource metadata moved to ResourceInfo at top level.
///
/// # Breaking Changes from v1
/// - `resource`, `description`, `mime_type`, `output_schema` removed (moved to ResourceInfo)
/// - `max_amount_required` renamed to `amount`
/// - `network` now uses CAIP-2 format
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirementsV2 {
    /// Payment scheme (currently only "exact")
    pub scheme: Scheme,

    /// Network in CAIP-2 format (e.g., "eip155:8453", "solana:5eykt...")
    pub network: Caip2NetworkId,

    /// Token contract address or account
    pub asset: MixedAddress,

    /// Exact amount required (renamed from maxAmountRequired)
    pub amount: TokenAmount,

    /// Recipient address for payment
    pub pay_to: MixedAddress,

    /// Maximum seconds before payment expires
    pub max_timeout_seconds: u64,

    /// Optional chain-specific or application-specific data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

impl PaymentRequirementsV2 {
    /// Convert to v1 PaymentRequirements for backward compatibility
    pub fn to_v1(&self, resource_info: &ResourceInfo) -> Result<PaymentRequirements, NetworkParseError> {
        let network = Network::from_caip2(&self.network.to_string())
            .ok_or_else(|| NetworkParseError::InvalidCaip2(self.network.to_string()))?;

        Ok(PaymentRequirements {
            scheme: self.scheme,
            network,
            max_amount_required: self.amount,
            resource: resource_info.url.clone(),
            description: resource_info.description.clone(),
            mime_type: resource_info.mime_type.clone(),
            output_schema: None, // v2 doesn't have this
            pay_to: self.pay_to.clone(),
            max_timeout_seconds: self.max_timeout_seconds,
            asset: self.asset.clone(),
            extra: self.extra.clone(),
        })
    }
}

// ============================================================================
// PaymentRequirements v1 -> v2 conversion
// ============================================================================

/// Extension trait for converting v1 PaymentRequirements to v2 format
pub trait PaymentRequirementsV1ToV2 {
    /// Convert v1 PaymentRequirements to v2 format
    fn to_v2(&self) -> (ResourceInfo, PaymentRequirementsV2);
}

impl PaymentRequirementsV1ToV2 for PaymentRequirements {
    fn to_v2(&self) -> (ResourceInfo, PaymentRequirementsV2) {
        let resource_info = ResourceInfo {
            url: self.resource.clone(),
            description: self.description.clone(),
            mime_type: self.mime_type.clone(),
        };

        // Parse the CAIP-2 string from to_caip2() into a Caip2NetworkId
        let caip2_str = self.network.to_caip2();
        let network = Caip2NetworkId::parse(&caip2_str)
            .expect("Network::to_caip2() should always produce valid CAIP-2");

        let requirements_v2 = PaymentRequirementsV2 {
            scheme: self.scheme,
            network,
            asset: self.asset.clone(),
            amount: self.max_amount_required,
            pay_to: self.pay_to.clone(),
            max_timeout_seconds: self.max_timeout_seconds,
            extra: self.extra.clone(),
        };

        (resource_info, requirements_v2)
    }
}

// ============================================================================
// PaymentPayloadV2
// ============================================================================

/// x402 v2 payment payload structure.
///
/// # Major Changes from v1
/// - New `resource` field (ResourceInfo) at top level
/// - `accepted` field containing payment requirements
/// - New `extensions` field for protocol extensions
/// - `x402_version` is now a plain u8 (value: 2)
///
/// # References
/// - x402 v2 spec: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayloadV2 {
    /// Protocol version (always 2 for v2)
    pub x402_version: u8,

    /// Information about the protected resource
    pub resource: ResourceInfo,

    /// Accepted payment requirements
    pub accepted: PaymentRequirementsV2,

    /// Chain-specific payment authorization data
    pub payload: ExactPaymentPayload,

    /// Optional protocol extensions (e.g., "bazaar", "sign_in_with_x")
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extensions: HashMap<String, serde_json::Value>,
}

impl PaymentPayloadV2 {
    /// Create a new v2 payment payload
    pub fn new(
        resource: ResourceInfo,
        accepted: PaymentRequirementsV2,
        payload: ExactPaymentPayload,
    ) -> Self {
        Self {
            x402_version: 2,
            resource,
            accepted,
            payload,
            extensions: HashMap::new(),
        }
    }

    /// Add an extension to the payload
    pub fn with_extension(mut self, name: String, value: serde_json::Value) -> Self {
        self.extensions.insert(name, value);
        self
    }

    /// Convert to v1 PaymentPayload for backward compatibility
    pub fn to_v1(&self) -> Result<PaymentPayload, NetworkParseError> {
        let network = Network::from_caip2(&self.accepted.network.to_string())
            .ok_or_else(|| NetworkParseError::InvalidCaip2(self.accepted.network.to_string()))?;

        Ok(PaymentPayload {
            x402_version: X402Version::V1,
            scheme: self.accepted.scheme,
            network,
            payload: self.payload.clone(),
        })
    }
}

/// Extension trait for converting v1 PaymentPayload to v2 format
pub trait PaymentPayloadV1ToV2 {
    /// Convert v1 PaymentPayload to v2 format
    ///
    /// # Arguments
    /// - `resource_info`: Metadata about the protected resource (not present in v1)
    /// - `amount`: Payment amount
    /// - `asset`: Token address
    /// - `pay_to`: Recipient address
    fn to_v2(
        &self,
        resource_info: ResourceInfo,
        amount: TokenAmount,
        asset: MixedAddress,
        pay_to: MixedAddress,
    ) -> PaymentPayloadV2;
}

impl PaymentPayloadV1ToV2 for PaymentPayload {
    fn to_v2(
        &self,
        resource_info: ResourceInfo,
        amount: TokenAmount,
        asset: MixedAddress,
        pay_to: MixedAddress,
    ) -> PaymentPayloadV2 {
        // Parse the CAIP-2 string from to_caip2() into a Caip2NetworkId
        let caip2_str = self.network.to_caip2();
        let network = Caip2NetworkId::parse(&caip2_str)
            .expect("Network::to_caip2() should always produce valid CAIP-2");

        let accepted = PaymentRequirementsV2 {
            scheme: self.scheme,
            network,
            asset,
            amount,
            pay_to,
            max_timeout_seconds: 300, // Default 5 minutes
            extra: None,
        };

        PaymentPayloadV2 {
            x402_version: 2,
            resource: resource_info,
            accepted,
            payload: self.payload.clone(),
            extensions: HashMap::new(),
        }
    }
}

// ============================================================================
// Envelope Types for Dual v1/v2 Support
// ============================================================================

/// Envelope type for handling both v1 and v2 payment payloads.
///
/// This type enables the facilitator to accept and route both protocol versions
/// during the migration period. Deserialization automatically detects the version
/// from the `x402_version` field.
///
/// # Lifecycle
/// - **Phase 1** (0-6 months): Dual support for v1 and v2
/// - **Phase 2** (6-12 months): v2 preferred, v1 deprecated warnings
/// - **Phase 3** (12+ months): v1 removed, v2 only
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaymentPayloadEnvelope {
    V2(PaymentPayloadV2),
    V1(PaymentPayload),
}

impl PaymentPayloadEnvelope {
    /// Extract the protocol version
    pub fn version(&self) -> X402Version {
        match self {
            PaymentPayloadEnvelope::V1(_) => X402Version::V1,
            PaymentPayloadEnvelope::V2(_) => X402Version::V2,
        }
    }

    /// Extract the network (v1 enum or v2 CAIP-2)
    pub fn network_v1(&self) -> Result<Network, NetworkParseError> {
        match self {
            PaymentPayloadEnvelope::V1(payload) => Ok(payload.network),
            PaymentPayloadEnvelope::V2(payload) => {
                Network::from_caip2(&payload.accepted.network.to_string())
                    .ok_or_else(|| NetworkParseError::InvalidCaip2(payload.accepted.network.to_string()))
            }
        }
    }

    /// Extract the network as CAIP-2 (for v2 compatibility)
    pub fn network_v2(&self) -> Caip2NetworkId {
        match self {
            PaymentPayloadEnvelope::V1(payload) => {
                let caip2_str = payload.network.to_caip2();
                Caip2NetworkId::parse(&caip2_str)
                    .expect("Network::to_caip2() should always produce valid CAIP-2")
            }
            PaymentPayloadEnvelope::V2(payload) => payload.accepted.network.clone(),
        }
    }

    /// Extract the payment payload (chain-specific authorization)
    pub fn payload(&self) -> &ExactPaymentPayload {
        match self {
            PaymentPayloadEnvelope::V1(p) => &p.payload,
            PaymentPayloadEnvelope::V2(p) => &p.payload,
        }
    }
}

// ============================================================================
// Request/Response Versioning
// ============================================================================

/// x402 v2 verify request (standard format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRequestV2 {
    pub x402_version: u8, // Always 2
    pub payment_payload: PaymentPayloadV2,
    pub resource: ResourceInfo,
    pub accepted: PaymentRequirementsV2,
}

// ============================================================================
// x402r Format Support (from x402r SDK)
// ============================================================================

/// x402r authorization structure (inner payload)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402rAuthorization {
    pub from: String,
    pub to: String,
    pub value: String,
    pub valid_after: String,
    pub valid_before: String,
    pub nonce: String,
}

/// x402r payload structure (authorization + signature)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402rPayload {
    pub authorization: X402rAuthorization,
    pub signature: String,
}

/// x402r verify request format (from x402r SDK) - top-level payload variant
///
/// This is an alternative format used by the x402r SDK for refundable payments.
/// Key differences from standard v2:
/// - Uses `payload` instead of `paymentPayload`
/// - `payload` contains `authorization` + `signature` directly
/// - No nested duplication of `resource`/`accepted` inside payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRequestX402r {
    pub x402_version: u8, // Always 2
    pub payload: X402rPayload,
    pub resource: ResourceInfo,
    pub accepted: PaymentRequirementsV2,
}

// ============================================================================
// x402r Nested Format (Ali's SDK format)
// ============================================================================

/// Inner payment payload for x402r nested format
///
/// Ali's SDK sends: paymentPayload.payload.authorization
/// This struct represents the inner paymentPayload object.
/// Note: resource may be missing in some SDK versions, so it's optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402rPaymentPayloadNested {
    pub x402_version: u8,
    pub payload: X402rPayload,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    /// Resource info - optional since some SDK versions don't include it
    #[serde(default)]
    pub resource: Option<ResourceInfo>,
    pub accepted: PaymentRequirementsV2,
}

/// x402r verify request format with nested paymentPayload (Ali's SDK actual format)
///
/// Ali's SDK sends this structure:
/// ```json
/// {
///   "x402Version": 2,
///   "paymentPayload": {
///     "x402Version": 2,
///     "payload": { "authorization": {...}, "signature": "..." },
///     "extensions": { "refund": {...} },
///     "accepted": {...}
///   },
///   "resource": {...},  // May be at top level or inside paymentPayload
///   "paymentRequirements": {...}
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRequestX402rNested {
    pub x402_version: u8,
    pub payment_payload: X402rPaymentPayloadNested,
    pub payment_requirements: PaymentRequirementsV2,
    /// Resource info - may be at top level (checked first) or inside paymentPayload
    #[serde(default)]
    pub resource: Option<ResourceInfo>,
}

impl VerifyRequestX402rNested {
    pub fn network(&self) -> &Caip2NetworkId {
        &self.payment_payload.accepted.network
    }

    /// Convert nested x402r format to v1 VerifyRequest for processing
    pub fn to_v1(&self) -> Result<VerifyRequest, NetworkParseError> {
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;

        let inner = &self.payment_payload;

        // Parse the authorization values
        let from = Address::from_str(&inner.payload.authorization.from)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.from.clone()))?;
        let to = Address::from_str(&inner.payload.authorization.to)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.to.clone()))?;
        let value = U256::from_str(&inner.payload.authorization.value)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.value.clone()))?;
        let valid_after = U256::from_str(&inner.payload.authorization.valid_after)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.valid_after.clone()))?;
        let valid_before = U256::from_str(&inner.payload.authorization.valid_before)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.valid_before.clone()))?;

        // Parse nonce (32 bytes hex)
        let nonce_str = inner.payload.authorization.nonce.trim_start_matches("0x");
        let nonce_bytes = hex::decode(nonce_str)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.authorization.nonce.clone()))?;

        // Parse signature
        let sig_str = inner.payload.signature.trim_start_matches("0x");
        let sig_bytes = hex::decode(sig_str)
            .map_err(|_| NetworkParseError::InvalidCaip2(inner.payload.signature.clone()))?;

        // Get network from accepted (inside paymentPayload)
        let network = Network::from_caip2(&inner.accepted.network.to_string())
            .ok_or_else(|| NetworkParseError::InvalidCaip2(inner.accepted.network.to_string()))?;

        // Convert U256 timestamps to u64 for UnixTimestamp
        let valid_after_u64: u64 = valid_after.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("valid_after too large".to_string()))?;
        let valid_before_u64: u64 = valid_before.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("valid_before too large".to_string()))?;

        // Convert nonce bytes to [u8; 32]
        let nonce_array: [u8; 32] = nonce_bytes.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("nonce must be 32 bytes".to_string()))?;

        let evm_authorization = ExactEvmPayloadAuthorization {
            from: EvmAddress(from),
            to: EvmAddress(to),
            value: TokenAmount(value),
            valid_after: UnixTimestamp(valid_after_u64),
            valid_before: UnixTimestamp(valid_before_u64),
            nonce: HexEncodedNonce(nonce_array),
        };

        let evm_payload = ExactEvmPayload {
            authorization: evm_authorization,
            signature: EvmSignature(sig_bytes),
        };

        let payment_payload = PaymentPayload {
            x402_version: X402Version::V2,
            network,
            scheme: inner.accepted.scheme,
            payload: ExactPaymentPayload::Evm(evm_payload),
        };

        // Get resource from: top-level, inner paymentPayload, or create default
        let resource = self.resource.clone()
            .or_else(|| inner.resource.clone())
            .unwrap_or_else(|| {
                // Create a minimal ResourceInfo if none provided
                ResourceInfo {
                    url: Url::parse("https://x402r.escrow/resource").unwrap(),
                    description: "x402r escrow payment".to_string(),
                    mime_type: "application/json".to_string(),
                }
            });

        // Build v1 PaymentRequirements from inner.accepted
        let payment_requirements = inner.accepted.to_v1(&resource)?;

        Ok(VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload,
            payment_requirements,
        })
    }
}

impl VerifyRequestX402r {
    pub fn network(&self) -> &Caip2NetworkId {
        &self.accepted.network
    }

    /// Convert x402r format to v1 VerifyRequest for processing
    pub fn to_v1(&self) -> Result<VerifyRequest, NetworkParseError> {
        use alloy::primitives::{Address, U256};
        use std::str::FromStr;

        // Parse the authorization values
        let from = Address::from_str(&self.payload.authorization.from)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.from.clone()))?;
        let to = Address::from_str(&self.payload.authorization.to)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.to.clone()))?;
        let value = U256::from_str(&self.payload.authorization.value)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.value.clone()))?;
        let valid_after = U256::from_str(&self.payload.authorization.valid_after)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.valid_after.clone()))?;
        let valid_before = U256::from_str(&self.payload.authorization.valid_before)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.valid_before.clone()))?;

        // Parse nonce (32 bytes hex)
        let nonce_str = self.payload.authorization.nonce.trim_start_matches("0x");
        let nonce_bytes = hex::decode(nonce_str)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.authorization.nonce.clone()))?;

        // Parse signature
        let sig_str = self.payload.signature.trim_start_matches("0x");
        let sig_bytes = hex::decode(sig_str)
            .map_err(|_| NetworkParseError::InvalidCaip2(self.payload.signature.clone()))?;

        // Get network
        let network = Network::from_caip2(&self.accepted.network.to_string())
            .ok_or_else(|| NetworkParseError::InvalidCaip2(self.accepted.network.to_string()))?;

        // Build v1 PaymentPayload
        // Convert U256 timestamps to u64 for UnixTimestamp
        let valid_after_u64: u64 = valid_after.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("valid_after too large".to_string()))?;
        let valid_before_u64: u64 = valid_before.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("valid_before too large".to_string()))?;

        // Convert nonce bytes to [u8; 32]
        let nonce_array: [u8; 32] = nonce_bytes.try_into()
            .map_err(|_| NetworkParseError::InvalidCaip2("nonce must be 32 bytes".to_string()))?;

        let evm_authorization = ExactEvmPayloadAuthorization {
            from: EvmAddress(from),
            to: EvmAddress(to),
            value: TokenAmount(value),
            valid_after: UnixTimestamp(valid_after_u64),
            valid_before: UnixTimestamp(valid_before_u64),
            nonce: HexEncodedNonce(nonce_array),
        };

        let evm_payload = ExactEvmPayload {
            authorization: evm_authorization,
            signature: EvmSignature(sig_bytes),
        };

        let payment_payload = PaymentPayload {
            x402_version: X402Version::V2,
            network,
            scheme: self.accepted.scheme,
            payload: ExactPaymentPayload::Evm(evm_payload),
        };

        // Build v1 PaymentRequirements
        let payment_requirements = self.accepted.to_v1(&self.resource)?;

        Ok(VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload,
            payment_requirements,
        })
    }
}

impl VerifyRequestV2 {
    pub fn network(&self) -> &Caip2NetworkId {
        &self.accepted.network
    }

    /// Convert to v1 VerifyRequest for backward compatibility
    pub fn to_v1(&self) -> Result<VerifyRequest, NetworkParseError> {
        let payment_payload = self.payment_payload.to_v1()?;
        let payment_requirements = self.accepted.to_v1(&self.resource)?;

        Ok(VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload,
            payment_requirements,
        })
    }
}

/// Unified verify request supporting v1, v2, x402r, and x402r-nested formats
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerifyRequestEnvelope {
    /// x402r nested format (Ali's SDK) - paymentPayload.payload.authorization
    /// MUST be first to try parsing nested format before standard v2
    X402rNested(VerifyRequestX402rNested),
    /// Standard v2 format with paymentPayload
    V2(VerifyRequestV2),
    /// x402r format with top-level payload (authorization + signature)
    X402r(VerifyRequestX402r),
    /// Legacy v1 format
    V1(VerifyRequest),
}

impl VerifyRequestEnvelope {
    /// Extract the protocol version
    pub fn version(&self) -> X402Version {
        match self {
            VerifyRequestEnvelope::V1(_) => X402Version::V1,
            VerifyRequestEnvelope::V2(_) => X402Version::V2,
            VerifyRequestEnvelope::X402r(_) => X402Version::V2,
            VerifyRequestEnvelope::X402rNested(_) => X402Version::V2,
        }
    }

    /// Get the network as a v1 Network enum
    pub fn network_v1(&self) -> Result<Network, NetworkParseError> {
        match self {
            VerifyRequestEnvelope::V1(req) => Ok(req.network()),
            VerifyRequestEnvelope::V2(req) => {
                Network::from_caip2(&req.network().to_string())
                    .ok_or_else(|| NetworkParseError::InvalidCaip2(req.network().to_string()))
            }
            VerifyRequestEnvelope::X402r(req) => {
                Network::from_caip2(&req.network().to_string())
                    .ok_or_else(|| NetworkParseError::InvalidCaip2(req.network().to_string()))
            }
            VerifyRequestEnvelope::X402rNested(req) => {
                Network::from_caip2(&req.network().to_string())
                    .ok_or_else(|| NetworkParseError::InvalidCaip2(req.network().to_string()))
            }
        }
    }

    /// Convert to v1 VerifyRequest for processing
    pub fn to_v1(&self) -> Result<VerifyRequest, NetworkParseError> {
        match self {
            VerifyRequestEnvelope::V1(req) => Ok(req.clone()),
            VerifyRequestEnvelope::V2(req) => req.to_v1(),
            VerifyRequestEnvelope::X402r(req) => req.to_v1(),
            VerifyRequestEnvelope::X402rNested(req) => req.to_v1(),
        }
    }
}

/// Unified settle request (same structure as verify)
pub type SettleRequestEnvelope = VerifyRequestEnvelope;
pub type SettleRequestV2 = VerifyRequestV2;

// ============================================================================
// Extended SupportedPaymentKindsResponse
// ============================================================================

/// Single supported payment kind in v2 format
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedPaymentKindV2 {
    pub x402_version: u8, // Can be 1 or 2
    pub scheme: Scheme,

    /// Network in CAIP-2 format for v2, v1 string for v1
    pub network: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<SupportedPaymentKindExtra>,
}

/// x402 v2 extended response for /supported endpoint.
///
/// # New in v2
/// - `extensions`: List of supported protocol extensions
/// - `signers`: Map of network patterns to facilitator signer addresses
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedPaymentKindsResponseV2 {
    /// List of supported payment methods
    pub kinds: Vec<SupportedPaymentKindV2>,

    /// List of supported extensions (e.g., ["bazaar", "sign_in_with_x"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,

    /// Facilitator signer addresses per network pattern
    /// Key format: "namespace:*" (e.g., "eip155:*", "solana:*")
    /// Value: List of signer addresses for that namespace
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub signers: HashMap<String, Vec<String>>,
}

impl SupportedPaymentKindsResponseV2 {
    /// Create a new v2 response with both v1 and v2 network formats
    pub fn new(networks: &[Network], facilitator_addresses: &HashMap<Namespace, Vec<MixedAddress>>) -> Self {
        let mut kinds = Vec::new();
        let mut signers = HashMap::new();

        // Generate v1 entries (for backward compatibility)
        for network in networks {
            kinds.push(SupportedPaymentKindV2 {
                x402_version: 1,
                scheme: Scheme::Exact,
                network: network.to_string(),
                extra: None,
            });
        }

        // Generate v2 entries (CAIP-2 format)
        for network in networks {
            kinds.push(SupportedPaymentKindV2 {
                x402_version: 2,
                scheme: Scheme::Exact,
                network: network.to_caip2().to_string(),
                extra: None,
            });
        }

        // Populate signers map
        for (namespace, addresses) in facilitator_addresses {
            let key = format!("{}:*", namespace);
            let address_strings = addresses.iter().map(|a| a.to_string()).collect();
            signers.insert(key, address_strings);
        }

        Self {
            kinds,
            extensions: vec![], // No extensions initially
            signers,
        }
    }
}

/// Extension trait for converting v1 SupportedPaymentKindsResponse to v2
pub trait SupportedPaymentKindsResponseV1ToV2 {
    /// Convert v1 response to v2 format
    fn to_v2(
        &self,
        extensions: Vec<String>,
        signers: HashMap<String, Vec<String>>,
    ) -> SupportedPaymentKindsResponseV2;
}

impl SupportedPaymentKindsResponseV1ToV2 for SupportedPaymentKindsResponse {
    fn to_v2(
        &self,
        extensions: Vec<String>,
        signers: HashMap<String, Vec<String>>,
    ) -> SupportedPaymentKindsResponseV2 {
        let kinds = self.kinds.iter().map(|kind| {
            SupportedPaymentKindV2 {
                x402_version: match kind.x402_version {
                    X402Version::V1 => 1,
                    X402Version::V2 => 2,
                },
                scheme: kind.scheme,
                network: kind.network.clone(),
                extra: kind.extra.clone(),
            }
        }).collect();

        SupportedPaymentKindsResponseV2 {
            kinds,
            extensions,
            signers,
        }
    }
}

// ============================================================================
// Error Types for V2
// ============================================================================

/// Error reasons specific to x402 v2
#[derive(Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum FacilitatorErrorReasonV2 {
    /// Payer doesn't have sufficient funds
    #[error("insufficient_funds")]
    InsufficientFunds,

    /// Invalid payment scheme
    #[error("invalid_scheme")]
    InvalidScheme,

    /// Network not supported or invalid CAIP-2 format
    #[error("invalid_network")]
    InvalidNetwork,

    /// Unexpected settlement error
    #[error("unexpected_settle_error")]
    UnexpectedSettleError,

    /// Invalid CAIP-2 network identifier
    #[error("invalid_caip2_network")]
    InvalidCaip2Network,

    /// Unsupported protocol extension
    #[error("unsupported_extension")]
    UnsupportedExtension,

    /// Resource metadata missing or invalid
    #[error("invalid_resource_info")]
    InvalidResourceInfo,

    /// Free-form error message
    #[error("{0}")]
    FreeForm(String),
}

impl From<FacilitatorErrorReason> for FacilitatorErrorReasonV2 {
    fn from(v1: FacilitatorErrorReason) -> Self {
        match v1 {
            FacilitatorErrorReason::InsufficientFunds => FacilitatorErrorReasonV2::InsufficientFunds,
            FacilitatorErrorReason::InvalidScheme => FacilitatorErrorReasonV2::InvalidScheme,
            FacilitatorErrorReason::InvalidNetwork => FacilitatorErrorReasonV2::InvalidNetwork,
            FacilitatorErrorReason::UnexpectedSettleError => FacilitatorErrorReasonV2::UnexpectedSettleError,
            FacilitatorErrorReason::FreeForm(msg) => FacilitatorErrorReasonV2::FreeForm(msg),
        }
    }
}

// ============================================================================
// Network Parse Error
// ============================================================================

/// Error returned when parsing a CAIP-2 network identifier to a Network enum
#[derive(Debug, thiserror::Error)]
pub enum NetworkParseError {
    #[error("Invalid CAIP-2 format: {0}")]
    InvalidCaip2(String),
    #[error("Invalid chain ID: {0}")]
    InvalidChainId(String),
    #[error("Unknown EVM chain ID: {0}")]
    UnknownChainId(u64),
    #[error("Unknown Solana genesis hash: {0}")]
    UnknownSolanaGenesisHash(String),
    #[error("Unknown NEAR network: {0}")]
    UnknownNearNetwork(String),
    #[error("Unknown Stellar network: {0}")]
    UnknownStellarNetwork(String),
    #[error("Unknown Fogo network: {0}")]
    UnknownFogoNetwork(String),
}

// ============================================================================
// Discovery API Types (Bazaar)
// ============================================================================

// ============================================================================
// Discovery Source Tracking
// ============================================================================

/// How a resource was discovered and added to the Bazaar registry.
///
/// This enables the "Meta-Bazaar" architecture where resources can come from
/// multiple sources: self-registration, settlement tracking, crawling, or
/// aggregation from other facilitators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    /// Resource was explicitly registered via POST /discovery/register
    #[default]
    SelfRegistered,

    /// Resource was auto-registered during /settle when discoverable=true
    Settlement,

    /// Resource was discovered by crawling /.well-known/x402 endpoints
    Crawled,

    /// Resource was aggregated from another facilitator's Bazaar
    Aggregated,
}

impl std::fmt::Display for DiscoverySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoverySource::SelfRegistered => write!(f, "self_registered"),
            DiscoverySource::Settlement => write!(f, "settlement"),
            DiscoverySource::Crawled => write!(f, "crawled"),
            DiscoverySource::Aggregated => write!(f, "aggregated"),
        }
    }
}

/// Metadata for a discoverable resource in the Bazaar registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryMetadata {
    /// Category for filtering (e.g., "finance", "ai", "data")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// Provider name or organization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Tags for search and discovery
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Default for DiscoveryMetadata {
    fn default() -> Self {
        Self {
            category: None,
            provider: None,
            tags: Vec::new(),
        }
    }
}

/// A discoverable paid resource in the Bazaar registry.
///
/// Represents an API endpoint or service that accepts x402 payments.
///
/// # Source Tracking (Meta-Bazaar)
///
/// Resources can come from multiple sources:
/// - `SelfRegistered`: Explicit POST /discovery/register
/// - `Settlement`: Auto-registered via /settle with discoverable=true
/// - `Crawled`: Discovered from /.well-known/x402 endpoints
/// - `Aggregated`: Pulled from another facilitator's Bazaar
///
/// The `source` and `source_facilitator` fields enable filtering and attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResource {
    /// The URL of the paid resource
    pub url: Url,

    /// Type of resource ("http", "mcp", "a2a")
    #[serde(rename = "type")]
    pub resource_type: String,

    /// x402 protocol version this resource supports
    pub x402_version: u8,

    /// Human-readable description of the resource
    pub description: String,

    /// Accepted payment methods
    pub accepts: Vec<PaymentRequirementsV2>,

    /// Unix timestamp of last registration/update
    pub last_updated: u64,

    /// Optional metadata for categorization and search
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<DiscoveryMetadata>,

    // ========== Meta-Bazaar Source Tracking ==========

    /// How this resource was discovered/registered
    #[serde(default)]
    pub source: DiscoverySource,

    /// Origin facilitator for aggregated resources (e.g., "coinbase", "ultravioleta")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_facilitator: Option<String>,

    /// Unix timestamp when we first discovered this resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<u64>,

    /// Number of settlements observed (for Settlement source)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_count: Option<u32>,
}

impl DiscoveryResource {
    /// Create a new discovery resource (defaults to SelfRegistered source)
    pub fn new(
        url: Url,
        resource_type: String,
        description: String,
        accepts: Vec<PaymentRequirementsV2>,
    ) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            url,
            resource_type,
            x402_version: 2,
            description,
            accepts,
            last_updated: now,
            metadata: None,
            source: DiscoverySource::SelfRegistered,
            source_facilitator: None,
            first_seen: Some(now),
            settlement_count: None,
        }
    }

    /// Create a resource from aggregation (another facilitator's Bazaar)
    pub fn from_aggregation(
        url: Url,
        resource_type: String,
        description: String,
        accepts: Vec<PaymentRequirementsV2>,
        source_facilitator: String,
        original_last_updated: u64,
    ) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            url,
            resource_type,
            x402_version: 2,
            description,
            accepts,
            last_updated: original_last_updated,
            metadata: None,
            source: DiscoverySource::Aggregated,
            source_facilitator: Some(source_facilitator),
            first_seen: Some(now),
            settlement_count: None,
        }
    }

    /// Create a resource from settlement tracking
    pub fn from_settlement(
        url: Url,
        resource_type: String,
        description: String,
        accepts: Vec<PaymentRequirementsV2>,
    ) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            url,
            resource_type,
            x402_version: 2,
            description,
            accepts,
            last_updated: now,
            metadata: None,
            source: DiscoverySource::Settlement,
            source_facilitator: None,
            first_seen: Some(now),
            settlement_count: Some(1),
        }
    }

    /// Set metadata for the resource
    pub fn with_metadata(mut self, metadata: DiscoveryMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set the source facilitator (for aggregated resources)
    pub fn with_source_facilitator(mut self, facilitator: String) -> Self {
        self.source_facilitator = Some(facilitator);
        self
    }

    /// Increment settlement count (for Settlement source)
    pub fn increment_settlement_count(&mut self) {
        self.settlement_count = Some(self.settlement_count.unwrap_or(0) + 1);
        // Update last_updated timestamp
        use std::time::{SystemTime, UNIX_EPOCH};
        self.last_updated = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }
}

/// Pagination information for discovery responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    /// Maximum number of items per page
    pub limit: u32,

    /// Number of items to skip
    pub offset: u32,

    /// Total number of matching items
    pub total: u32,
}

impl Pagination {
    pub fn new(limit: u32, offset: u32, total: u32) -> Self {
        Self { limit, offset, total }
    }
}

/// Response from GET /discovery/resources endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryResponse {
    /// x402 protocol version
    pub x402_version: u8,

    /// List of discoverable resources
    pub items: Vec<DiscoveryResource>,

    /// Pagination information
    pub pagination: Pagination,
}

impl DiscoveryResponse {
    pub fn new(items: Vec<DiscoveryResource>, pagination: Pagination) -> Self {
        Self {
            x402_version: 2,
            items,
            pagination,
        }
    }

    pub fn empty() -> Self {
        Self {
            x402_version: 2,
            items: Vec::new(),
            pagination: Pagination::new(10, 0, 0),
        }
    }
}

/// Request to register a resource with the discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResourceRequest {
    /// The URL of the paid resource
    pub url: Url,

    /// Type of resource ("http", "mcp", "a2a")
    #[serde(rename = "type")]
    pub resource_type: String,

    /// Human-readable description
    pub description: String,

    /// Accepted payment methods
    pub accepts: Vec<PaymentRequirementsV2>,

    /// Optional metadata for categorization
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<DiscoveryMetadata>,
}

impl RegisterResourceRequest {
    /// Convert to a DiscoveryResource
    pub fn into_resource(self) -> DiscoveryResource {
        let resource = DiscoveryResource::new(
            self.url,
            self.resource_type,
            self.description,
            self.accepts,
        );
        match self.metadata {
            Some(meta) => resource.with_metadata(meta),
            None => resource,
        }
    }
}

/// Query parameters for filtering discovery results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFilters {
    /// Filter by category
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// Filter by network (CAIP-2 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,

    /// Filter by provider name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Filter by tag
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    // ========== Meta-Bazaar Filters ==========

    /// Filter by discovery source (self_registered, settlement, crawled, aggregated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Filter by source facilitator (e.g., "coinbase", "ultravioleta")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_facilitator: Option<String>,
}

impl DiscoveryFilters {
    pub fn is_empty(&self) -> bool {
        self.category.is_none()
            && self.network.is_none()
            && self.provider.is_none()
            && self.tag.is_none()
            && self.source.is_none()
            && self.source_facilitator.is_none()
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;

    #[test]
    fn test_resource_info_creation() {
        let resource = ResourceInfo::new(
            Url::parse("https://api.example.com/data").unwrap(),
            "Premium data".to_string(),
            "application/json".to_string(),
        );

        assert_eq!(resource.url.as_str(), "https://api.example.com/data");
        assert_eq!(resource.description, "Premium data");
        assert_eq!(resource.mime_type, "application/json");
    }

    #[test]
    fn test_resource_info_serde() {
        let resource = ResourceInfo::new(
            Url::parse("https://api.example.com/data").unwrap(),
            "Premium data".to_string(),
            "application/json".to_string(),
        );

        let json = serde_json::to_string(&resource).unwrap();
        let parsed: ResourceInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(resource, parsed);
    }

    #[test]
    fn test_payment_requirements_v2_serde() {
        let json = r#"{
            "scheme": "exact",
            "network": "eip155:8453",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount": "1000000",
            "payTo": "0x1234567890123456789012345678901234567890",
            "maxTimeoutSeconds": 300
        }"#;

        let reqs: PaymentRequirementsV2 = serde_json::from_str(json).unwrap();
        assert_eq!(reqs.scheme, Scheme::Exact);
        assert_eq!(reqs.network.to_string(), "eip155:8453");
        assert_eq!(reqs.max_timeout_seconds, 300);
    }

    #[test]
    fn test_payment_payload_envelope_version_detection() {
        // Test that envelope correctly wraps v1 and v2 payloads
        // and can determine their version

        // V1: Direct PaymentPayload
        let v1_payload = PaymentPayload {
            x402_version: X402Version::V1,
            scheme: Scheme::Exact,
            network: Network::Base,
            payload: ExactPaymentPayload::Evm(crate::types::ExactEvmPayload {
                signature: crate::types::EvmSignature(vec![0u8; 65]),
                authorization: crate::types::ExactEvmPayloadAuthorization {
                    from: "0x1234567890123456789012345678901234567890".parse().unwrap(),
                    to: "0x1234567890123456789012345678901234567890".parse().unwrap(),
                    value: TokenAmount::from(1000000u64),
                    valid_after: crate::timestamp::UnixTimestamp(0),
                    valid_before: crate::timestamp::UnixTimestamp(2000000000),
                    nonce: crate::types::HexEncodedNonce([0u8; 32]),
                },
            }),
        };

        let envelope_v1 = PaymentPayloadEnvelope::V1(v1_payload);
        assert!(matches!(envelope_v1.version(), X402Version::V1));

        // V2: PaymentPayloadV2
        let resource = ResourceInfo::new(
            Url::parse("https://example.com").unwrap(),
            "test".to_string(),
            "application/json".to_string(),
        );
        let requirements = PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network: Caip2NetworkId::eip155(8453),
            asset: MixedAddress::Evm("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse().unwrap()),
            amount: TokenAmount::from(1000000u64),
            pay_to: MixedAddress::Evm("0x1234567890123456789012345678901234567890".parse().unwrap()),
            max_timeout_seconds: 300,
            extra: None,
        };
        let v2_payload = PaymentPayloadV2::new(
            resource,
            requirements,
            ExactPaymentPayload::Evm(crate::types::ExactEvmPayload {
                signature: crate::types::EvmSignature(vec![0u8; 65]),
                authorization: crate::types::ExactEvmPayloadAuthorization {
                    from: "0x1234567890123456789012345678901234567890".parse().unwrap(),
                    to: "0x1234567890123456789012345678901234567890".parse().unwrap(),
                    value: TokenAmount::from(1000000u64),
                    valid_after: crate::timestamp::UnixTimestamp(0),
                    valid_before: crate::timestamp::UnixTimestamp(2000000000),
                    nonce: crate::types::HexEncodedNonce([0u8; 32]),
                },
            }),
        );

        let envelope_v2 = PaymentPayloadEnvelope::V2(v2_payload);
        assert!(matches!(envelope_v2.version(), X402Version::V2));
    }

    #[test]
    fn test_supported_payment_kinds_response_v2_new() {
        let networks = vec![Network::Base, Network::Solana];
        let mut facilitator_addresses = HashMap::new();
        facilitator_addresses.insert(
            Namespace::Eip155,
            vec![MixedAddress::Evm("0x1234567890123456789012345678901234567890".parse().unwrap())],
        );

        let response = SupportedPaymentKindsResponseV2::new(&networks, &facilitator_addresses);

        // Should have 4 entries: 2 v1 + 2 v2
        assert_eq!(response.kinds.len(), 4);

        // Check v1 entries exist
        let v1_networks: Vec<_> = response.kinds.iter()
            .filter(|k| k.x402_version == 1)
            .map(|k| k.network.as_str())
            .collect();
        assert!(v1_networks.contains(&"base"));
        assert!(v1_networks.contains(&"solana"));

        // Check v2 entries exist (CAIP-2 format)
        let v2_networks: Vec<_> = response.kinds.iter()
            .filter(|k| k.x402_version == 2)
            .map(|k| k.network.as_str())
            .collect();
        assert!(v2_networks.contains(&"eip155:8453"));
        assert!(v2_networks.iter().any(|n| n.starts_with("solana:")));

        // Check signers
        assert!(response.signers.contains_key("eip155:*"));
    }

    #[test]
    fn test_facilitator_error_reason_v2_from_v1() {
        let v1_error = FacilitatorErrorReason::InsufficientFunds;
        let v2_error: FacilitatorErrorReasonV2 = v1_error.into();
        assert!(matches!(v2_error, FacilitatorErrorReasonV2::InsufficientFunds));

        let v1_error = FacilitatorErrorReason::FreeForm("test error".to_string());
        let v2_error: FacilitatorErrorReasonV2 = v1_error.into();
        assert!(matches!(v2_error, FacilitatorErrorReasonV2::FreeForm(msg) if msg == "test error"));
    }
}
