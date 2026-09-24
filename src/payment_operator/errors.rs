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

    /// The call would go to one address and name another as
    /// `paymentInfo.operator`. The escrow only takes calls from
    /// `paymentInfo.operator` itself, so the two must be the same.
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

    /// The operator transaction was broadcast and no verdict came back. It may
    /// be mined, so the hash travels instead of being flattened into text.
    #[error("Settlement unconfirmed: {0} on {1}")]
    SettlementUnconfirmed(crate::types::TransactionHash, Network),

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

    /// A `release` / `refundInEscrow` order was refused under
    /// `ESCROW_LIFECYCLE_AUTH=enforce`. `category` is bounded (see
    /// `lifecycle_auth::Verdict::category`); `owner_unverifiable` is the one
    /// that is retryable.
    #[error("Escrow lifecycle order rejected ({category}): {detail}")]
    LifecycleAuthRejected {
        category: &'static str,
        detail: String,
    },

    /// A chain read the request depends on failed -- an RPC error, a rate
    /// limit, an answer that would not decode. Nothing was sent, so the same
    /// request can be retried. Never a 4xx: a caller that treats every 4xx as
    /// final would give up on a payment that only needed a second try.
    #[error("could not read the chain ({0}); nothing was sent, retry later")]
    ChainReadUnavailable(String),

    /// A PaymentOperator this facilitator declares for the network has not
    /// passed its self-check against the chain (see `autoverify`), so a NEW
    /// authorization is not placed against it. Nothing was sent. The expected
    /// case is an operator that is declared but not deployed yet: it turns
    /// verified on its own once it is, which is why this is retryable.
    #[error(
        "PaymentOperator {operator} is not verified on {network} ({reason}); nothing was sent"
    )]
    OperatorNotVerified {
        operator: Address,
        network: String,
        reason: String,
    },

    /// The escrow's signature over the authorization does not come from the
    /// payer as a plain EOA under this network's token domain.
    #[error("Escrow authorization signature rejected: {0}")]
    AuthorizationSignatureInvalid(String),

    /// `void` returns the whole capturable amount and takes no amount, so a
    /// refund of any other non-zero amount cannot be honoured on this
    /// operator generation. Nothing was sent.
    #[error("refundInEscrow of {requested} cannot be honoured: this operator voids the whole capturable amount ({capturable}); nothing was sent")]
    PartialRefundUnsupported { requested: u128, capturable: u128 },

    /// A `refundInEscrow` of 0 on an operator that voids everything. A missing
    /// or misparsed amount is never read as permission to void the whole
    /// authorization: the caller names the capturable amount. Nothing was sent.
    #[error("refundInEscrow needs the amount to void ({capturable} is capturable); 0 is not taken to mean all of it; nothing was sent")]
    AmountRequired { capturable: u128 },

    /// Nothing is capturable any more -- the authorization was already voided
    /// or captured. Typically the retry of a void that already went through.
    #[error("nothing to void: the capturable amount is 0; nothing was sent")]
    NothingToVoid,

    /// The PaymentOperator a write would go to has no code on this network. A
    /// call to such an address succeeds and does nothing, so it is refused
    /// before anything is signed.
    #[error("PaymentOperator {operator} has no code on {network}; nothing was sent")]
    OperatorHasNoCode { operator: Address, network: String },
}

/// How a typed escrow failure answers over HTTP: status, the bounded token
/// that goes in `errorReason`, and the `Retry-After` seconds when the same
/// request may be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpAnswer {
    pub status: u16,
    pub token: &'static str,
    pub retry_after_secs: Option<u32>,
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

    /// The fixed answer of the failures that carry their own status. `None`
    /// for every other variant, which keeps the classification it always had.
    ///
    /// No transaction exists behind any of these: each is decided before
    /// anything is signed.
    pub fn http_answer(&self) -> Option<HttpAnswer> {
        match self {
            OperatorError::ChainReadUnavailable(_) => Some(HttpAnswer {
                status: 502,
                token: "chain_read_unavailable",
                retry_after_secs: Some(30),
            }),
            // Retryable: the verdict is re-read at most a minute later, and an
            // operator that is declared but not yet deployed turns verified
            // on its own once it is.
            OperatorError::OperatorNotVerified { .. } => Some(HttpAnswer {
                status: 503,
                token: "operator_not_verified",
                retry_after_secs: Some(60),
            }),
            OperatorError::OperatorMismatch { .. } => Some(HttpAnswer {
                status: 400,
                token: "operator_mismatch",
                retry_after_secs: None,
            }),
            OperatorError::OperatorHasNoCode { .. } => Some(HttpAnswer {
                status: 422,
                token: "operator_has_no_code",
                retry_after_secs: None,
            }),
            OperatorError::AuthorizationSignatureInvalid(_) => Some(HttpAnswer {
                status: 400,
                token: "authorization_signature_invalid",
                retry_after_secs: None,
            }),
            OperatorError::PartialRefundUnsupported { .. } => Some(HttpAnswer {
                status: 422,
                token: "partial_refund_unsupported_on_generation",
                retry_after_secs: None,
            }),
            OperatorError::AmountRequired { .. } => Some(HttpAnswer {
                status: 422,
                token: "amount_required_on_generation",
                retry_after_secs: None,
            }),
            OperatorError::NothingToVoid => Some(HttpAnswer {
                status: 409,
                token: "nothing_to_void",
                retry_after_secs: None,
            }),
            _ => None,
        }
    }
}
