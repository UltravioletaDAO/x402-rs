//! ERC-8004 type definitions for x402 integration.
//!
//! These types represent the data structures used in the `8004-reputation` extension
//! and match the official ERC-8004 specification.

use alloy::primitives::{FixedBytes, U256};
use serde::{Deserialize, Serialize};

use crate::network::Network;
use crate::types::{MixedAddress, TokenAmount, TransactionHash};

// ============================================================================
// Identity Registry Types
// ============================================================================

/// Agent identity information from the Identity Registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdentity {
    /// The agent's ID (ERC-721 tokenId)
    pub agent_id: u64,
    /// Owner address of the agent NFT
    pub owner: MixedAddress,
    /// URI pointing to agent registration file
    pub agent_uri: String,
    /// Payment wallet address (if set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_wallet: Option<MixedAddress>,
    /// Network where the agent is registered
    pub network: Network,
}

/// Agent registration file structure (resolved from agentURI)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRegistrationFile {
    /// Type identifier
    #[serde(rename = "type")]
    pub type_: String,
    /// Agent name
    pub name: String,
    /// Agent description
    pub description: String,
    /// Image URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// List of services the agent provides
    #[serde(default)]
    pub services: Vec<AgentService>,
    /// Whether x402 payments are supported
    #[serde(default)]
    pub x402_support: bool,
    /// Whether the agent is active
    #[serde(default = "default_true")]
    pub active: bool,
    /// List of registrations across chains
    #[serde(default)]
    pub registrations: Vec<AgentRegistration>,
    /// Supported trust models
    #[serde(default)]
    pub supported_trust: Vec<String>,
}

/// Agent service entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentService {
    pub name: String,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Agent registration reference
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRegistration {
    pub agent_id: u64,
    pub agent_registry: String, // Format: {namespace}:{chainId}:{address}
}

// ============================================================================
// Reputation Registry Types
// ============================================================================

/// Parameters for submitting reputation feedback (matches official spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackParams {
    /// The agent's ID (tokenId in Identity Registry)
    pub agent_id: u64,

    /// Feedback value (fixed-point)
    /// Examples: 87 with decimals=0 means 87/100, 9977 with decimals=2 means 99.77%
    pub value: i128,

    /// Decimal places for value interpretation (0-18)
    #[serde(default)]
    pub value_decimals: u8,

    /// Primary categorization tag (e.g., "starred", "uptime", "responseTime")
    #[serde(default)]
    pub tag1: String,

    /// Secondary categorization tag
    #[serde(default)]
    pub tag2: String,

    /// Service endpoint that was used (optional)
    #[serde(default)]
    pub endpoint: String,

    /// URI to off-chain feedback file (IPFS, HTTPS)
    #[serde(default)]
    pub feedback_uri: String,

    /// Keccak256 hash of feedback content (for integrity)
    #[serde(default)]
    pub feedback_hash: Option<FixedBytes<32>>,

    /// Proof of payment (required for authorized feedback)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof: Option<ProofOfPayment>,
}

/// Request body for POST /feedback endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRequest {
    /// x402 protocol version
    pub x402_version: crate::types::X402Version,
    /// Network where feedback will be submitted
    pub network: Network,
    /// Feedback parameters
    pub feedback: FeedbackParams,
}

/// Response from POST /feedback endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackResponse {
    /// Whether the feedback was successfully submitted
    pub success: bool,
    /// Transaction hash of the feedback submission
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    /// Feedback index assigned (1-indexed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_index: Option<u64>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Network where feedback was submitted
    pub network: Network,
}

/// Request to revoke feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeFeedbackRequest {
    pub x402_version: crate::types::X402Version,
    pub network: Network,
    pub agent_id: u64,
    pub feedback_index: u64,
}

/// Request to append response to feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendResponseRequest {
    pub x402_version: crate::types::X402Version,
    pub network: Network,
    pub agent_id: u64,
    pub client_address: MixedAddress,
    pub feedback_index: u64,
    pub response_uri: String,
    #[serde(default)]
    pub response_hash: Option<FixedBytes<32>>,
}

/// Reputation summary for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReputationSummary {
    /// Agent ID
    pub agent_id: u64,
    /// Number of feedback entries
    pub count: u64,
    /// Aggregated value
    pub summary_value: i128,
    /// Decimal places for summary_value
    pub summary_value_decimals: u8,
    /// Network
    pub network: Network,
}

/// Individual feedback entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEntry {
    /// Client who submitted the feedback
    pub client: MixedAddress,
    /// Feedback index (1-indexed)
    pub feedback_index: u64,
    /// Feedback value
    pub value: i128,
    /// Value decimals
    pub value_decimals: u8,
    /// Primary tag
    pub tag1: String,
    /// Secondary tag
    pub tag2: String,
    /// Whether this feedback was revoked
    pub is_revoked: bool,
}

/// Request to get reputation summary
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetReputationRequest {
    /// Filter by specific clients (empty = all)
    #[serde(default)]
    pub client_addresses: Vec<MixedAddress>,
    /// Filter by tag1
    #[serde(default)]
    pub tag1: String,
    /// Filter by tag2
    #[serde(default)]
    pub tag2: String,
}

/// Response for reputation query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReputationResponse {
    pub agent_id: u64,
    pub summary: ReputationSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Vec<FeedbackEntry>>,
    pub network: Network,
}

// ============================================================================
// Proof of Payment
// ============================================================================

/// Cryptographic proof of a settled payment for reputation submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofOfPayment {
    /// Transaction hash of the settled payment
    pub transaction_hash: TransactionHash,
    /// Block number where the transaction was included
    pub block_number: u64,
    /// Network where the payment was settled
    pub network: Network,
    /// The payer (consumer/client) address
    pub payer: MixedAddress,
    /// The payee (agent/resource owner) address
    pub payee: MixedAddress,
    /// Amount paid in token base units
    pub amount: TokenAmount,
    /// Token contract address
    pub token: MixedAddress,
    /// Unix timestamp of the block
    pub timestamp: u64,
    /// Keccak256 hash of the payment data for verification
    pub payment_hash: FixedBytes<32>,
}

impl ProofOfPayment {
    /// Create a new ProofOfPayment from settlement data.
    pub fn new(
        transaction_hash: TransactionHash,
        block_number: u64,
        network: Network,
        payer: MixedAddress,
        payee: MixedAddress,
        amount: TokenAmount,
        token: MixedAddress,
        timestamp: u64,
    ) -> Self {
        let payment_hash = Self::compute_payment_hash(
            &transaction_hash,
            block_number,
            &payer,
            &payee,
            &amount,
        );

        Self {
            transaction_hash,
            block_number,
            network,
            payer,
            payee,
            amount,
            token,
            timestamp,
            payment_hash,
        }
    }

    /// Compute the payment hash from core fields.
    fn compute_payment_hash(
        transaction_hash: &TransactionHash,
        block_number: u64,
        payer: &MixedAddress,
        payee: &MixedAddress,
        amount: &TokenAmount,
    ) -> FixedBytes<32> {
        use alloy::primitives::keccak256;

        let mut data = Vec::new();

        match transaction_hash {
            TransactionHash::Evm(bytes) => data.extend_from_slice(bytes),
            _ => data.extend_from_slice(&[0u8; 32]),
        }

        data.extend_from_slice(&block_number.to_be_bytes());
        let payer_bytes = format!("{}", payer);
        data.extend_from_slice(payer_bytes.as_bytes());
        let payee_bytes = format!("{}", payee);
        data.extend_from_slice(payee_bytes.as_bytes());
        let amount_u256: U256 = (*amount).into();
        data.extend_from_slice(&amount_u256.to_be_bytes::<32>());

        keccak256(&data)
    }
}

// ============================================================================
// Extension Types
// ============================================================================

/// Extension data parsed from PaymentRequirements.extra
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Erc8004Extension {
    /// Whether proof of payment should be included in settlement response
    #[serde(default = "default_true")]
    pub include_proof: bool,
}

fn default_true() -> bool {
    true
}

impl Erc8004Extension {
    /// Try to parse ERC-8004 extension from PaymentRequirements.extra
    pub fn from_extra(extra: &Option<serde_json::Value>) -> Option<Self> {
        let extra = extra.as_ref()?;
        let obj = extra.as_object()?;
        let extension_data = obj.get(super::EXTENSION_ID)?;
        serde_json::from_value(extension_data.clone()).ok()
    }
}

// ============================================================================
// Off-Chain Feedback File Structure
// ============================================================================

/// Off-chain feedback file structure (for feedbackURI)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackFile {
    /// Type identifier
    #[serde(rename = "type", default = "default_feedback_type")]
    pub type_: String,
    /// Feedback entries
    pub feedback: Vec<FeedbackFileEntry>,
}

fn default_feedback_type() -> String {
    "https://eips.ethereum.org/EIPS/eip-8004#feedback-v1".to_string()
}

/// Individual entry in a feedback file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackFileEntry {
    pub timestamp: u64,
    #[serde(default)]
    pub result: String,
    pub value: i128,
    #[serde(default)]
    pub value_decimals: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_info: Option<PaymentInfo>,
    #[serde(default)]
    pub interactions: Vec<String>,
}

/// Payment info in feedback file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentInfo {
    pub token: String,
    pub amount: String,
    pub reason: String,
}

// ============================================================================
// Legacy Types (for backward compatibility)
// ============================================================================

/// Extended SettleResponse that includes ProofOfPayment when ERC-8004 is active.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponseWithProof {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<crate::types::FacilitatorErrorReason>,
    pub payer: MixedAddress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    pub network: Network,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_of_payment: Option<ProofOfPayment>,
}

// ============================================================================
// Validation Registry Types
// ============================================================================

/// Validation request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationRequestParams {
    pub x402_version: crate::types::X402Version,
    pub network: Network,
    pub validator_address: MixedAddress,
    pub agent_id: u64,
    pub request_uri: String,
    #[serde(default)]
    pub request_hash: Option<FixedBytes<32>>,
}

/// Validation response submission
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResponseParams {
    pub x402_version: crate::types::X402Version,
    pub network: Network,
    pub request_hash: FixedBytes<32>,
    pub response: u8, // 0-100
    pub response_uri: String,
    #[serde(default)]
    pub response_hash: Option<FixedBytes<32>>,
    #[serde(default)]
    pub tag: String,
}

/// Validation status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationStatus {
    pub validator_address: MixedAddress,
    pub agent_id: u64,
    pub response: u8,
    pub response_hash: FixedBytes<32>,
    pub tag: String,
    pub last_update: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::X402Version;

    #[test]
    fn test_erc8004_extension_parsing() {
        let extra = serde_json::json!({
            "8004-reputation": {
                "includeProof": true
            }
        });

        let extension = Erc8004Extension::from_extra(&Some(extra)).unwrap();
        assert!(extension.include_proof);
    }

    #[test]
    fn test_feedback_params_full() {
        let params = FeedbackParams {
            agent_id: 42,
            value: 87,
            value_decimals: 0,
            tag1: "starred".to_string(),
            tag2: "quality".to_string(),
            endpoint: "https://agent.example/api".to_string(),
            feedback_uri: "ipfs://QmFeedback".to_string(),
            feedback_hash: None,
            proof: None,
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"agentId\":42"));
        assert!(json.contains("\"tag1\":\"starred\""));
    }

    #[test]
    fn test_feedback_request_serialization() {
        let request = FeedbackRequest {
            x402_version: X402Version::V1,
            network: Network::EthereumSepolia,
            feedback: FeedbackParams {
                agent_id: 1,
                value: 100,
                value_decimals: 0,
                tag1: "test".to_string(),
                tag2: "".to_string(),
                endpoint: "".to_string(),
                feedback_uri: "".to_string(),
                feedback_hash: None,
                proof: None,
            },
        };

        let json = serde_json::to_string_pretty(&request).unwrap();
        assert!(json.contains("ethereum-sepolia"));
    }
}
