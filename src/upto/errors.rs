//! Error types for the upto payment scheme.

/// Errors that can occur during upto payment verification and settlement.
#[derive(Debug, thiserror::Error)]
pub enum UptoError {
    #[error("Upto scheme is disabled (ENABLE_UPTO != true)")]
    FeatureDisabled,

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid scheme: expected 'upto', got '{0}'")]
    InvalidScheme(String),

    #[error("Unsupported network: {0}")]
    UnsupportedNetwork(String),

    #[error("Provider not found for network: {0}")]
    ProviderNotFound(String),

    #[error("Non-EVM network (Permit2 is EVM-only)")]
    NonEvmNetwork,

    #[error("Invalid payload: {0}")]
    InvalidPayload(String),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    #[error("Settlement failed: {0}")]
    SettlementFailed(String),

    #[error("Contract call failed: {0}")]
    ContractCall(String),

    #[error("Settlement amount {actual} exceeds authorized max {max}")]
    AmountExceedsMax { actual: String, max: String },

    #[error("Spender mismatch: expected {expected}, got {actual}")]
    SpenderMismatch { expected: String, actual: String },

    #[error("Recipient mismatch: expected {expected}, got {actual}")]
    RecipientMismatch { expected: String, actual: String },

    #[error("Insufficient Permit2 allowance: has {has}, needs {needs}")]
    InsufficientAllowance { has: String, needs: String },

    #[error("Insufficient balance: has {has}, needs {needs}")]
    InsufficientBalance { has: String, needs: String },

    #[error("Authorization expired: deadline {deadline} < now {now}")]
    Expired { deadline: u64, now: u64 },

    #[error("Authorization not yet valid: validAfter {valid_after} > now {now}")]
    NotYetValid { valid_after: u64, now: u64 },

    #[error("Deserialization error: {0}")]
    DeserializationError(#[from] serde_json::Error),
}
