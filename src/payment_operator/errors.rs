//! Error types for PaymentOperator operations

use alloy::primitives::Address;
use thiserror::Error;

use crate::chain::FacilitatorLocalError;
use crate::network::Network;

/// Errors that can occur during PaymentOperator operations
#[derive(Debug, Error)]
pub enum OperatorError {
    #[error("PaymentOperator feature is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable.")]
    FeatureDisabled,

    #[error("PaymentOperator requires x402 v2 protocol")]
    V1NotSupported,

    #[error("Invalid scheme: expected 'escrow', got '{0}'")]
    InvalidScheme(String),

    #[error("Invalid escrow payload format: {0}")]
    InvalidExtensionFormat(String),

    #[error("Unknown operator action: {0}")]
    UnknownAction(String),

    #[error("Network {0} does not support PaymentOperator (escrow not deployed)")]
    UnsupportedNetwork(String),

    #[error("Only EVM networks support PaymentOperator settlement")]
    NonEvmNetwork,

    #[error("Invalid EVM address in payload")]
    InvalidEvmAddress,

    #[error("Invalid amount format: {0}")]
    InvalidAmount(String),

    #[error("Operator address mismatch: expected {expected}, got {actual}")]
    OperatorMismatch { expected: Address, actual: Address },

    #[error("Fee receiver address mismatch: expected operator ({expected}), got {actual}")]
    FeeReceiverMismatch { expected: Address, actual: Address },

    #[error("Fee bounds incompatible: calculated {calculated_bps} bps, allowed range [{min_bps}, {max_bps}]")]
    FeeBoundsIncompatible {
        calculated_bps: u16,
        min_bps: u16,
        max_bps: u16,
    },

    #[error("Condition check failed for action")]
    ConditionNotMet,

    #[error("Contract call failed: {0}")]
    ContractCall(String),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Provider not found for network: {0}")]
    ProviderNotFound(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("PaymentInfo validation failed: {0}")]
    PaymentInfoInvalid(String),

    #[error("Escrow state query failed: {0}")]
    EscrowStateQuery(String),

    #[error("Payment has already been collected")]
    PaymentAlreadyCollected,

    #[error("Insufficient authorization: authorized {authorized}, requested {requested}")]
    InsufficientAuthorization { authorized: u128, requested: u128 },

    #[error("Refund exceeds captured amount: refunding {refund}, captured {captured}")]
    RefundExceedsCapture { refund: u128, captured: u128 },

    #[error("Authorization expired")]
    AuthorizationExpired,

    #[error("Refund expired")]
    RefundExpired,

    #[error("Pre-approval expired")]
    PreApprovalExpired,
}

impl From<OperatorError> for FacilitatorLocalError {
    fn from(err: OperatorError) -> Self {
        FacilitatorLocalError::Other(err.to_string())
    }
}

/// Helper to convert network to OperatorError
impl OperatorError {
    pub fn unsupported_network(network: &Network) -> Self {
        OperatorError::UnsupportedNetwork(network.to_string())
    }
}
