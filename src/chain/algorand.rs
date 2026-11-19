//! Algorand payment provider implementation.
//!
//! This module implements Algorand payments using atomic transaction groups.
//! Users sign standard ASA (Algorand Standard Asset) transfers, and the
//! facilitator co-signs a fee-paying transaction to enable gasless payments
//! via Algorand's fee pooling mechanism.
//!
//! Flow (based on Coinbase/Algorand Foundation x402 specification):
//! 1. Client creates an atomic transaction group: [fee_tx, asa_transfer]
//! 2. Client signs only the ASA transfer (standard wallet signature)
//! 3. Client sends the partially-signed group to facilitator
//! 4. Facilitator verifies the ASA transfer is valid and authorized
//! 5. Facilitator signs the fee transaction and submits the entire group
//! 6. Algorand network executes both atomically (or neither)
//!
//! Key Insight: Fee pooling allows transaction 0 to pay fees for transaction 1,
//! enabling completely gasless payments for users.

#![cfg(feature = "algorand")]

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use once_cell::sync::OnceCell;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use algonaut::algod::v2::Algod;
use algonaut::core::Address as AlgoAddress;
use algonaut::core::{LogicSignature, MultisigAddress, MultisigSignature};
use algonaut::transaction::account::Account;
use algonaut::transaction::transaction::TransactionSignature;
use algonaut::transaction::{SignedTransaction, Transaction as AlgoTransaction, TransactionType};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::Network;
use crate::nonce_store::{algorand_nonce_key, algorand_ttl_seconds, NonceStore, NonceStoreError};
use crate::types::{
    ExactAlgorandPayload, ExactPaymentPayload, FacilitatorErrorReason, MixedAddress,
    PaymentRequirements, Scheme, SettleRequest, SettleResponse, SupportedPaymentKind,
    SupportedPaymentKindExtra, SupportedPaymentKindsResponse, TransactionHash, VerifyRequest,
    VerifyResponse, X402Version,
};

// =============================================================================
// Constants
// =============================================================================

/// USDC ASA ID on Algorand mainnet
pub const USDC_ASA_ID_MAINNET: u64 = 31566704;

/// USDC ASA ID on Algorand testnet
pub const USDC_ASA_ID_TESTNET: u64 = 10458941;

/// Default Algorand mainnet algod endpoint
pub const ALGORAND_MAINNET_ALGOD: &str = "https://mainnet-api.algonode.cloud";

/// Default Algorand testnet algod endpoint
pub const ALGORAND_TESTNET_ALGOD: &str = "https://testnet-api.algonode.cloud";

/// Most transactions an Algorand atomic group may hold (the consensus
/// parameter `MaxTxGroupSize`): algod refuses a larger group.
pub const MAX_ALGORAND_GROUP_SIZE: usize = 16;

/// Most the facilitator's fee transaction may pay per transaction in its
/// group, in microAlgos.
///
/// Through fee pooling the fee transaction pays for the whole group, so its
/// cap is this times the group's size. The standard minimum fee is 1_000
/// microAlgos per transaction; five times that is the cap the official
/// facilitator (`@x402/avm`, `MAX_REASONABLE_FEE_PER_TXN`) applies, room for
/// congestion. The two-transaction group every known client sends sets
/// 2_000 against a cap of 10_000.
pub const MAX_ALGORAND_FEE_PER_GROUP_TXN_MICROALGOS: u64 = 5_000;

/// The note clients put on the fee transaction: `@x402/avm` writes it
/// followed by `-` and `Date.now()`, other public x402 clients write it
/// alone, and the uvd-x402-sdk clients write none. The facilitator signs the
/// fee transaction, so its note is published from the facilitator's account.
const FEE_NOTE: &[u8] = b"x402-fee-payer";

/// How many digits the fee note carries after `FEE_NOTE` and `-`: those of
/// `Date.now()`, 13 from September 2001 until the year 2286.
const FEE_NOTE_DIGITS: usize = 13;

/// The only multisignature version Algorand defines (algonaut keeps its own
/// copy private).
const MULTISIG_VERSION: u8 = 1;

/// Most keys algod takes in a multisignature.
const MAX_MULTISIG_KEYS: usize = 255;

// =============================================================================
// Error Types
// =============================================================================

/// Whether a failed broadcast may still have put the group in algod's pool.
///
/// An HTTP 4xx is algod refusing the group, except `already in ledger`, which
/// says it was committed. A 5xx, a timeout or a client error (a dropped
/// connection, an answer that does not decode) leaves it possibly accepted. A
/// bad URL, token or header, or a group that failed to encode, never left.
fn broadcast_may_have_landed(error: &algonaut::error::ServiceError) -> bool {
    use algonaut::error::{RequestErrorDetails, ServiceError};
    match error {
        ServiceError::Request(request) => match &request.details {
            RequestErrorDetails::Http { status, message } => {
                *status >= 500 || message.to_ascii_lowercase().contains("already in ledger")
            }
            RequestErrorDetails::Timeout | RequestErrorDetails::Client { .. } => true,
        },
        _ => false,
    }
}

/// The settle answer for a submitted group.
///
/// Broadcast-but-unconfirmed is not a failed settlement. Reporting it as
/// `success: false` with no transaction tells the caller the payment did not
/// happen; it travels as an error so `IntoResponse` can answer `502
/// settlement_unconfirmed` with the id.
fn settle_outcome(
    submitted: Result<String, AlgorandError>,
    payer: MixedAddress,
    network: Network,
) -> Result<SettleResponse, FacilitatorLocalError> {
    match submitted {
        Ok(tx_id) => {
            tracing::info!(tx_id = %tx_id, "Algorand settle: Transaction submitted successfully");
            Ok(SettleResponse {
                success: true,
                error_reason: None,
                payer,
                transaction: Some(TransactionHash::Algorand(tx_id)),
                network,
                proof_of_payment: None, // ERC-8004 not supported on Algorand
                extensions: None,
            })
        }
        Err(AlgorandError::TransactionNotConfirmed { tx_id, attempts }) => {
            tracing::error!(
                tx_id = %tx_id,
                attempts = attempts,
                "Algorand settle: transaction submitted, never confirmed"
            );
            Err(FacilitatorLocalError::SettlementUnconfirmed(
                TransactionHash::Algorand(tx_id),
                network,
            ))
        }
        Err(AlgorandError::SubmissionUnconfirmed { tx_id, detail }) => {
            tracing::error!(
                tx_id = %tx_id,
                error = %crate::redact::scrub_urls(&detail),
                "Algorand settle: broadcast answer lost, the group may be committed"
            );
            Err(FacilitatorLocalError::SettlementUnconfirmed(
                TransactionHash::Algorand(tx_id),
                network,
            ))
        }
        Err(e) => {
            tracing::error!(error = %e, "Algorand settle: Failed to submit transaction");
            Ok(SettleResponse {
                success: false,
                error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                payer,
                transaction: None,
                network,
                proof_of_payment: None,
                extensions: None,
            })
        }
    }
}

/// A failed broadcast of the group whose first transaction is `first_id`.
fn broadcast_error(error: algonaut::error::ServiceError, first_id: String) -> AlgorandError {
    if broadcast_may_have_landed(&error) {
        AlgorandError::SubmissionUnconfirmed {
            tx_id: first_id,
            detail: error.to_string(),
        }
    } else {
        AlgorandError::SubmissionFailed(error.to_string())
    }
}

/// Algorand-specific errors
#[derive(Debug, thiserror::Error)]
pub enum AlgorandError {
    #[error("Invalid transaction encoding: {0}")]
    InvalidEncoding(String),

    #[error("Invalid atomic group: {0}")]
    InvalidAtomicGroup(String),

    #[error("Transaction expired at round {expiry_round} (current: {current_round})")]
    TransactionExpired {
        expiry_round: u64,
        current_round: u64,
    },

    #[error("Invalid signature for address {address}")]
    InvalidSignature { address: String },

    #[error("Fee transaction has forbidden fields: {field}")]
    ForbiddenFeeField { field: String },

    #[error("Insufficient fee amount: provided {provided}, required {required}")]
    InsufficientFee { provided: u64, required: u64 },

    #[error("Transaction submission failed: {0}")]
    SubmissionFailed(String),

    /// The group was broadcast and never seen confirmed.
    ///
    /// Carries `tx_id` because this is not a failed submission: the group may
    /// yet be committed, and the caller needs the id to check rather than a
    /// bare attempt count.
    #[error("Transaction {tx_id} not confirmed after {attempts} attempts")]
    TransactionNotConfirmed { tx_id: String, attempts: u32 },

    /// The broadcast itself failed without algod refusing the group: its
    /// answer was lost, so the group may have entered the pool.
    #[error("Transaction {tx_id} submitted with no answer: {detail}")]
    SubmissionUnconfirmed { tx_id: String, detail: String },

    #[error("ASA ID mismatch: expected {expected}, got {actual}")]
    AsaIdMismatch { expected: u64, actual: u64 },

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Invalid group ID")]
    InvalidGroupId,

    #[error("Payment index out of bounds: {index} >= {len}")]
    PaymentIndexOutOfBounds { index: usize, len: usize },

    #[error("Transaction type mismatch: expected asset transfer")]
    TransactionTypeMismatch,

    #[error("Transaction simulation failed: {0}")]
    SimulationFailed(String),

    #[error("Payment transaction missing lease field for replay protection")]
    MissingLease,

    #[error("Lease mismatch: expected {expected}, got {actual}")]
    LeaseMismatch { expected: String, actual: String },

    #[error("Fee transaction sender mismatch: expected facilitator {expected}, got {actual}")]
    FeeSenderMismatch { expected: String, actual: String },

    #[error("Fee transaction receiver mismatch: expected facilitator {expected}, got {actual}")]
    FeeReceiverMismatch { expected: String, actual: String },

    #[error("Fee transaction amount {amount} exceeds maximum allowed {max}")]
    FeeTooHigh { amount: u64, max: u64 },

    #[error("Transaction group {group_id} already used")]
    GroupAlreadyUsed { group_id: String },

    // Same fail-closed rule as Stellar's nonce read on verify: a store that
    // cannot answer rejects the payment instead of vouching for it.
    #[error("Nonce store unavailable: {0}")]
    NonceStoreUnavailable(String),
}

impl From<AlgorandError> for FacilitatorLocalError {
    fn from(e: AlgorandError) -> Self {
        match e {
            // The payer's authorization does not verify: the protocol's
            // `invalid_signature`, as on the other chains.
            AlgorandError::InvalidSignature { ref address } => {
                FacilitatorLocalError::InvalidSignature(
                    MixedAddress::Algorand(address.clone()),
                    e.to_string(),
                )
            }
            other => FacilitatorLocalError::Other(other.to_string()),
        }
    }
}

// =============================================================================
// Chain Configuration
// =============================================================================

/// Algorand network chain configuration
#[derive(Clone, Debug)]
pub struct AlgorandChain {
    pub network: Network,
    pub usdc_asa_id: u64,
}

impl AlgorandChain {
    /// Get the default algod API URL for this network
    pub fn default_algod_url(&self) -> &'static str {
        match self.network {
            Network::Algorand => ALGORAND_MAINNET_ALGOD,
            Network::AlgorandTestnet => ALGORAND_TESTNET_ALGOD,
            _ => unreachable!("AlgorandChain only supports Algorand networks"),
        }
    }
}

impl TryFrom<Network> for AlgorandChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Algorand => Ok(Self {
                network: value,
                usdc_asa_id: USDC_ASA_ID_MAINNET,
            }),
            Network::AlgorandTestnet => Ok(Self {
                network: value,
                usdc_asa_id: USDC_ASA_ID_TESTNET,
            }),
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

// =============================================================================
// Address Types
// =============================================================================

/// Algorand address wrapper
#[derive(Clone, Debug)]
pub struct AlgorandAddress {
    /// The address in standard Algorand format (58 characters, base32)
    pub address: String,
}

impl AlgorandAddress {
    /// Create a new AlgorandAddress
    pub fn new(address: String) -> Self {
        Self { address }
    }

    /// Check if this is a valid Algorand address
    pub fn is_valid(&self) -> bool {
        // Algorand addresses are 58 characters, base32 encoded
        if self.address.len() != 58 {
            return false;
        }
        // Try to parse as Algorand address
        AlgoAddress::from_str(&self.address).is_ok()
    }

    /// Convert to algonaut Address type
    pub fn to_algo_address(&self) -> Result<AlgoAddress, AlgorandError> {
        AlgoAddress::from_str(&self.address)
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Invalid address: {}", e)))
    }
}

impl TryFrom<String> for AlgorandAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let addr = AlgorandAddress::new(value);
        if addr.is_valid() {
            Ok(addr)
        } else {
            Err(FacilitatorLocalError::InvalidAddress(format!(
                "Invalid Algorand address: {}",
                addr.address
            )))
        }
    }
}

impl TryFrom<MixedAddress> for AlgorandAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Algorand(address) => Self::try_from(address),
            _ => Err(FacilitatorLocalError::InvalidAddress(
                "expected Algorand address".to_string(),
            )),
        }
    }
}

impl From<AlgorandAddress> for MixedAddress {
    fn from(value: AlgorandAddress) -> Self {
        MixedAddress::Algorand(value.address)
    }
}

// =============================================================================
// Provider Implementation
// =============================================================================

/// Algorand payment provider
///
/// Implements USDC payments on Algorand using atomic transaction groups.
/// The facilitator receives partially-signed atomic groups, verifies them,
/// Global nonce store for replay protection across all Algorand providers.
/// Initialized lazily on first use.
static GLOBAL_NONCE_STORE: OnceCell<Arc<dyn NonceStore>> = OnceCell::new();

/// Get or initialize the global nonce store.
/// Uses DynamoDB if NONCE_STORE_TABLE_NAME is configured, otherwise falls back to memory.
async fn get_global_nonce_store() -> Arc<dyn NonceStore> {
    // If already initialized, return it
    if let Some(store) = GLOBAL_NONCE_STORE.get() {
        return store.clone();
    }

    // Initialize and store
    let store = crate::nonce_store::create_nonce_store().await;
    let _ = GLOBAL_NONCE_STORE.set(store.clone());
    store
}

/// signs the fee transaction, and submits the complete group.
#[derive(Clone)]
pub struct AlgorandProvider {
    /// The facilitator's Algorand account (for signing fee transactions)
    account: Arc<Account>,
    /// The facilitator's public address
    public_address: String,
    /// Algod client for RPC calls
    algod: Arc<Algod>,
    /// Algod URL for simulation API calls (not wrapped by algonaut)
    algod_url: String,
    /// HTTP client for simulation requests
    http_client: reqwest::Client,
    /// Network configuration
    chain: AlgorandChain,
}

impl Debug for AlgorandProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlgorandProvider")
            .field("public_address", &self.public_address)
            .field("chain", &self.chain)
            .finish()
    }
}

impl AlgorandProvider {
    /// Get the chain name for nonce store keys.
    fn chain_name(&self) -> &'static str {
        match self.chain.network {
            Network::Algorand => "algorand",
            Network::AlgorandTestnet => "algorand-testnet",
            _ => "algorand-unknown",
        }
    }

    /// Atomically check if group_id is unused and mark it as used.
    /// Must be called BEFORE submitting transaction to blockchain.
    async fn check_and_mark_group_used(
        &self,
        group_id: &[u8; 32],
        current_round: u64,
        last_valid_round: u64,
    ) -> Result<(), AlgorandError> {
        let store = get_global_nonce_store().await;
        let key = algorand_nonce_key(self.chain_name(), group_id);
        let ttl = algorand_ttl_seconds(current_round, last_valid_round);

        store
            .check_and_mark_used(&key, ttl)
            .await
            .map_err(|e| match e {
                NonceStoreError::NonceAlreadyUsed(_) => AlgorandError::InvalidAtomicGroup(
                    "Transaction group already processed (replay attempt)".to_string(),
                ),
                other => AlgorandError::RpcError(format!("Nonce store error: {}", other)),
            })
    }

    /// Check that group_id has not been used (read-only, for verification).
    ///
    /// The fee transaction can only be signed by this facilitator, and
    /// submit_group() claims the group_id in the store before broadcasting it,
    /// so every group that reached the chain through us is recorded here. The
    /// claim outlives the group's validity window (algorand_ttl_seconds), and
    /// past that window verify_payment_group() already rejects it as expired.
    async fn check_group_unused(&self, group_id: &[u8; 32]) -> Result<(), AlgorandError> {
        let store = get_global_nonce_store().await;
        let key = algorand_nonce_key(self.chain_name(), group_id);

        match store.is_used(&key).await {
            Ok(true) => Err(AlgorandError::GroupAlreadyUsed {
                group_id: hex::encode(group_id),
            }),
            Ok(false) => Ok(()),
            Err(e) => {
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(
                    %correlation_id,
                    error = %e,
                    group_id = %hex::encode(group_id),
                    "Nonce store read failed during verify, failing closed"
                );
                Err(AlgorandError::NonceStoreUnavailable(format!(
                    "verification_unavailable (ref: {correlation_id})"
                )))
            }
        }
    }

    /// Create a new Algorand provider
    pub fn try_new(
        mnemonic: String,
        algod_url: Option<String>,
        network: Network,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = AlgorandChain::try_from(network)?;

        // Create account from mnemonic
        let account = Account::from_mnemonic(&mnemonic).map_err(|e| {
            FacilitatorLocalError::InvalidAddress(format!("Invalid Algorand mnemonic: {}", e))
        })?;

        let public_address = account.address().to_string();
        let effective_url = algod_url.as_deref().unwrap_or(chain.default_algod_url());

        // Create algod client
        // Use placeholder token - algonode.cloud doesn't require auth but algonaut needs valid format
        let placeholder_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let algod = Algod::new(effective_url, placeholder_token).map_err(|e| {
            FacilitatorLocalError::Other(format!("Failed to create Algod client: {}", e))
        })?;

        tracing::info!(
            network = %network,
            public_address = %public_address,
            algod_url = %effective_url,
            usdc_asa_id = chain.usdc_asa_id,
            "Initialized Algorand provider"
        );

        // Create HTTP client for simulation API calls. Explicit timeouts: reqwest
        // defaults to none, so an RPC that accepts the TCP connection and never
        // responds would otherwise hang until the ALB's 600s idle timeout.
        let http_client = reqwest::Client::builder()
            .timeout(crate::chain::rpc_http_timeout())
            .connect_timeout(crate::chain::rpc_http_connect_timeout())
            .build()
            .expect("static reqwest client config is always valid");

        Ok(Self {
            account: Arc::new(account),
            public_address,
            algod: Arc::new(algod),
            algod_url: effective_url.to_string(),
            http_client,
            chain,
        })
    }

    /// Get the facilitator's public address as MixedAddress
    pub fn facilitator_address(&self) -> MixedAddress {
        MixedAddress::Algorand(self.public_address.clone())
    }

    /// Decode a base64 msgpack transaction
    fn decode_transaction(&self, base64_tx: &str) -> Result<AlgoTransaction, AlgorandError> {
        let bytes = BASE64
            .decode(base64_tx)
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Base64 decode failed: {}", e)))?;

        // Decode msgpack transaction
        rmp_serde::from_slice(&bytes)
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Msgpack decode failed: {}", e)))
    }

    /// Decode a base64 msgpack signed transaction
    fn decode_signed_transaction(
        &self,
        base64_tx: &str,
    ) -> Result<SignedTransaction, AlgorandError> {
        let bytes = BASE64
            .decode(base64_tx)
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Base64 decode failed: {}", e)))?;

        rmp_serde::from_slice(&bytes)
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Msgpack decode failed: {}", e)))
    }

    /// Validate the fee transaction is safe to sign
    ///
    /// CRITICAL SECURITY: This prevents malicious transactions from:
    /// - Draining the facilitator's funds (close_remainder_to)
    /// - Taking over the facilitator's account (rekey_to)
    /// - Signing as a sender that is NOT the facilitator (sender check -- B2)
    /// - Paying more fee than the group needs from the facilitator wallet (fee
    ///   cap per transaction in the group of `group_size`)
    /// - Moving funds or acting for the facilitator's account: only a payment
    ///   of zero microAlgos from the facilitator to itself, with no lease, is
    ///   ever signed
    /// - Publishing the client's text from the facilitator's account: the only
    ///   notes allowed are the fixed ones clients write
    fn validate_fee_transaction(
        &self,
        tx: &AlgoTransaction,
        group_size: usize,
    ) -> Result<(), AlgorandError> {
        // B2 invariant 1: The fee transaction MUST be sent FROM the facilitator address.
        // An attacker could otherwise embed Payment(facilitator -> attacker, amount) in the
        // atomic group and get the facilitator to co-sign it, draining the facilitator wallet.
        let tx_sender = tx.sender().to_string();
        if tx_sender != self.public_address {
            tracing::warn!(
                expected = %self.public_address,
                actual = %tx_sender,
                "Fee transaction sender is not the facilitator address -- rejecting"
            );
            return Err(AlgorandError::FeeSenderMismatch {
                expected: self.public_address.clone(),
                actual: tx_sender,
            });
        }

        // B2 invariant 2: the fee is bounded by the group it pays for, at
        // MAX_ALGORAND_FEE_PER_GROUP_TXN_MICROALGOS per transaction, so a settle
        // never spends from the facilitator wallet more than its group can need.
        let fee_amount = tx.fee.0;
        let max_fee = MAX_ALGORAND_FEE_PER_GROUP_TXN_MICROALGOS.saturating_mul(group_size as u64);
        if fee_amount > max_fee {
            tracing::warn!(
                fee_amount = fee_amount,
                max = max_fee,
                group_size = group_size,
                "Fee transaction amount exceeds cap -- rejecting"
            );
            return Err(AlgorandError::FeeTooHigh {
                amount: fee_amount,
                max: max_fee,
            });
        }

        // Check for rekey_to which would give control of facilitator account to attacker
        if tx.rekey_to.is_some() {
            return Err(AlgorandError::ForbiddenFeeField {
                field: "rekey_to".to_string(),
            });
        }

        // A lease would take a (sender, lease) lock on the facilitator's account
        // chosen by the client; the fee transaction never needs one.
        if tx.lease.is_some() {
            return Err(AlgorandError::ForbiddenFeeField {
                field: "lease".to_string(),
            });
        }

        // The fee transaction may only be a payment that moves nothing, from the
        // facilitator to itself. Any amount, or any other type, would spend the
        // facilitator's funds or act for its account under its signature; any
        // other receiver would be a payment the facilitator signs to an address
        // the client chose. Every known client sends a self-payment, and the
        // official facilitator refuses anything else.
        match &tx.txn_type {
            TransactionType::Payment(payment) => {
                // close_remainder_to would send remaining funds to attacker
                if payment.close_remainder_to.is_some() {
                    return Err(AlgorandError::ForbiddenFeeField {
                        field: "close_remainder_to".to_string(),
                    });
                }
                if payment.amount.0 != 0 {
                    return Err(AlgorandError::ForbiddenFeeField {
                        field: "amount".to_string(),
                    });
                }
                let receiver = payment.receiver.to_string();
                if receiver != self.public_address {
                    tracing::warn!(
                        expected = %self.public_address,
                        actual = %receiver,
                        "Fee transaction receiver is not the facilitator address -- rejecting"
                    );
                    return Err(AlgorandError::FeeReceiverMismatch {
                        expected: self.public_address.clone(),
                        actual: receiver,
                    });
                }
            }
            _ => {
                return Err(AlgorandError::InvalidAtomicGroup(
                    "Fee transaction must be a payment transaction".to_string(),
                ));
            }
        }

        if !fee_note_is_allowed(tx.note.as_deref()) {
            return Err(AlgorandError::ForbiddenFeeField {
                field: "note".to_string(),
            });
        }

        Ok(())
    }

    /// Verify the atomic group structure and signatures
    async fn verify_payment_group(
        &self,
        payload: &ExactAlgorandPayload,
        requirements: &PaymentRequirements,
        payment_scheme: Scheme,
    ) -> Result<VerifyGroupResult, AlgorandError> {
        if payload.payment_group.len() < 2 {
            return Err(AlgorandError::InvalidAtomicGroup(
                "Group must have at least 2 transactions".to_string(),
            ));
        }

        // The fee cap grows with the group, so the group is held to the size
        // algod accepts.
        if payload.payment_group.len() > MAX_ALGORAND_GROUP_SIZE {
            return Err(AlgorandError::InvalidAtomicGroup(format!(
                "Group has {} transactions, at most {MAX_ALGORAND_GROUP_SIZE} allowed",
                payload.payment_group.len()
            )));
        }

        if payload.payment_index >= payload.payment_group.len() {
            return Err(AlgorandError::PaymentIndexOutOfBounds {
                index: payload.payment_index,
                len: payload.payment_group.len(),
            });
        }

        // Index 0 is the fee transaction: submit_group() puts the facilitator's
        // signed copy in its place, so a payment checked there is never broadcast.
        // From index 1 on, what is checked and what is broadcast are the same bytes
        // read by the same decoder.
        if payload.payment_index == 0 {
            return Err(AlgorandError::InvalidAtomicGroup(
                "payment index 0 is the fee transaction; the payment must be at index 1 or above"
                    .to_string(),
            ));
        }

        // Decode the fee transaction (index 0, unsigned)
        let fee_tx = self.decode_transaction(&payload.payment_group[0])?;

        // Validate fee transaction security
        self.validate_fee_transaction(&fee_tx, payload.payment_group.len())?;

        // Decode the payment transaction (signed by client)
        let payment_signed =
            self.decode_signed_transaction(&payload.payment_group[payload.payment_index])?;

        // Verify group IDs match
        let fee_group_id = fee_tx.group.ok_or(AlgorandError::InvalidGroupId)?;
        let payment_group_id = payment_signed
            .transaction
            .group
            .ok_or(AlgorandError::InvalidGroupId)?;

        if fee_group_id.0 != payment_group_id.0 {
            return Err(AlgorandError::InvalidAtomicGroup(
                "Group IDs do not match".to_string(),
            ));
        }

        // Verify lease field is present for replay protection
        // The lease should be SHA-256(paymentRequirements) as per GoPlausible x402-avm spec
        // This provides additional replay protection beyond group_id tracking
        match &payment_signed.transaction.lease {
            Some(lease) => {
                tracing::debug!(
                    lease = BASE64.encode(&lease.0),
                    "Payment transaction has lease field set"
                );
            }
            None => {
                // Warn but don't fail - client may not support lease yet
                // TODO: Make this an error once all clients implement lease support
                tracing::warn!(
                    "Payment transaction missing lease field - \
                     replay protection relies only on group_id tracking. \
                     Clients should set lease = SHA256(paymentRequirements) for security."
                );
            }
        }

        // Verify the payment is an asset transfer and get details
        let (asset_id, amount, receiver, sender) = match &payment_signed.transaction.txn_type {
            TransactionType::AssetTransferTransaction(xfer) => (
                xfer.xfer,
                xfer.amount,
                xfer.receiver.clone(),
                xfer.sender.clone(),
            ),
            _ => {
                return Err(AlgorandError::InvalidAtomicGroup(
                    "Payment must be an asset transfer".to_string(),
                ));
            }
        };

        // Verify it's USDC
        if asset_id != self.chain.usdc_asa_id {
            return Err(AlgorandError::AsaIdMismatch {
                expected: self.chain.usdc_asa_id,
                actual: asset_id,
            });
        }

        // ---- BIND TO PAYMENT REQUIREMENTS (SECURITY audit 05; mirrors near.rs / stellar B4) ----
        // Without these, the signed ASA transfer's receiver/amount were never compared to
        // requirements.pay_to / max_amount_required, so a 1-microUSDC self-transfer could
        // confirm an arbitrary-priced resource. Checks run BEFORE the algod round-trip below.

        // (a) network must match the network this provider serves
        if requirements.network != self.chain.network {
            return Err(AlgorandError::InvalidAtomicGroup(format!(
                "requirements.network {} does not match provider network {}",
                requirements.network, self.chain.network
            )));
        }

        // (b) scheme must be exact and match requirements
        if payment_scheme != Scheme::Exact || requirements.scheme != Scheme::Exact {
            return Err(AlgorandError::InvalidAtomicGroup(format!(
                "unsupported scheme: payload={payment_scheme:?}, requirements={:?}",
                requirements.scheme
            )));
        }

        // (c) recipient: signed transfer receiver MUST equal requirements.pay_to
        let expected_pay_to = match &requirements.pay_to {
            MixedAddress::Algorand(addr) => addr.clone(),
            other => {
                return Err(AlgorandError::InvalidAtomicGroup(format!(
                    "pay_to must be an Algorand address, got {other:?}"
                )));
            }
        };
        let actual_receiver = receiver.to_string();
        if actual_receiver != expected_pay_to {
            tracing::warn!(
                expected = %expected_pay_to,
                actual = %actual_receiver,
                "Algorand payment receiver does not match pay_to -- rejecting"
            );
            return Err(AlgorandError::InvalidAtomicGroup(format!(
                "payment receiver {actual_receiver} does not match required pay_to {expected_pay_to}"
            )));
        }

        // (d) amount: signed transfer amount MUST equal requirements.max_amount_required.
        // On-chain `amount` is u64; max_amount_required is TokenAmount(U256). Compare in U256.
        let expected_amount = requirements.max_amount_required.0; // alloy U256
        let actual_amount = alloy::primitives::U256::from(amount); // u64 -> U256, no overflow
        if actual_amount != expected_amount {
            tracing::warn!(
                expected = %expected_amount,
                actual = %actual_amount,
                "Algorand payment amount does not match max_amount_required -- rejecting"
            );
            return Err(AlgorandError::InvalidAtomicGroup(format!(
                "payment amount {actual_amount} does not match required {expected_amount}"
            )));
        }

        // (e) defense-in-depth: if requirements.asset carries a numeric ASA id, it must match
        if let MixedAddress::Offchain(asset_str) = &requirements.asset {
            if let Ok(req_asa) = asset_str.parse::<u64>() {
                if req_asa != asset_id {
                    return Err(AlgorandError::AsaIdMismatch {
                        expected: req_asa,
                        actual: asset_id,
                    });
                }
            }
        }

        // (f) the transfer is authorized by its sender, who is the payer verify
        // reports, so verify does not vouch for a payment its sender never
        // signed. The rest of the chain's verdict is still algod's.
        if !payment_authorized_by_sender(&payment_signed)? {
            tracing::warn!(
                sender = %sender,
                "Algorand payment authorization does not verify -- rejecting"
            );
            return Err(AlgorandError::InvalidSignature {
                address: sender.to_string(),
            });
        }

        // Get current round for validity checks
        let status = self
            .algod
            .status()
            .await
            .map_err(|e| AlgorandError::RpcError(e.to_string()))?;
        let current_round = status.last_round;

        // Check transaction validity window
        // last_valid is a Round (not Option), check if it's expired
        let last_valid_round = payment_signed.transaction.last_valid.0;
        if last_valid_round < current_round {
            return Err(AlgorandError::TransactionExpired {
                expiry_round: last_valid_round,
                current_round,
            });
        }

        // Extract payer address
        let payer_address = sender.to_string();

        Ok(VerifyGroupResult {
            payer: AlgorandAddress::new(payer_address),
            fee_tx,
            payment_signed,
            group_id: fee_group_id.0,
            amount,
            recipient: receiver.to_string(),
            current_round,
            last_valid_round,
        })
    }

    /// Sign the fee transaction and submit the group
    async fn submit_group(
        &self,
        verification: &VerifyGroupResult,
        payload: &ExactAlgorandPayload,
    ) -> Result<String, AlgorandError> {
        // CRITICAL: Atomically check and mark group_id as used BEFORE submitting to blockchain
        // This prevents replay attacks even if the facilitator crashes after submission
        self.check_and_mark_group_used(
            &verification.group_id,
            verification.current_round,
            verification.last_valid_round,
        )
        .await?;

        // Sign the fee transaction
        let signed_fee = self
            .account
            .sign_transaction(verification.fee_tx.clone())
            .map_err(|e| AlgorandError::InvalidEncoding(format!("Failed to sign fee tx: {}", e)))?;

        // Build the complete signed group
        let mut signed_group: Vec<SignedTransaction> =
            Vec::with_capacity(payload.payment_group.len());

        for (i, tx_base64) in payload.payment_group.iter().enumerate() {
            if i == 0 {
                // Fee transaction - use our signature
                signed_group.push(signed_fee.clone());
            } else {
                // Other transactions - already signed by client
                let signed = self.decode_signed_transaction(tx_base64)?;
                signed_group.push(signed);
            }
        }

        // NOTE: Simulation is temporarily disabled because rmp_serde::to_vec_named
        // produces msgpack with algonaut's field names (e.g., "transaction") instead of
        // Algorand's canonical field names (e.g., "txn"). The simulate API rejects this.
        // broadcast_signed_transactions uses algonaut's internal encoding which works.
        // TODO: Implement proper canonical msgpack encoding for simulation.
        tracing::info!(
            group_size = signed_group.len(),
            "Skipping simulation - algonaut broadcast handles encoding correctly"
        );

        let tx_id = self.broadcast(&signed_group).await?;

        tracing::info!(
            tx_id = %tx_id,
            group_size = signed_group.len(),
            "Submitted Algorand atomic group"
        );

        // Wait for confirmation
        self.wait_for_confirmation(&tx_id).await?;

        // Note: group_id was already marked as used at the start of submit_group()
        // via check_and_mark_group_used(), stored in persistent DynamoDB

        Ok(tx_id)
    }

    /// Broadcast an atomic group. algod answers with the id of the group's
    /// first transaction, our fee transaction, which is known before the
    /// broadcast and is what an unanswered one is reported under.
    async fn broadcast(&self, signed_group: &[SignedTransaction]) -> Result<String, AlgorandError> {
        let first_id = signed_group
            .first()
            .map(|tx| tx.transaction_id.clone())
            .ok_or_else(|| AlgorandError::InvalidAtomicGroup("empty group".to_string()))?;
        let pending_tx = self
            .algod
            .broadcast_signed_transactions(signed_group)
            .await
            .map_err(|e| broadcast_error(e, first_id))?;
        Ok(pending_tx.tx_id)
    }

    /// Wait for transaction confirmation
    async fn wait_for_confirmation(&self, tx_id: &str) -> Result<(), AlgorandError> {
        const MAX_ATTEMPTS: u32 = 20;
        const POLL_INTERVAL_MS: u64 = 500;

        for attempt in 1..=MAX_ATTEMPTS {
            tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;

            match self.algod.pending_transaction_with_id(tx_id).await {
                Ok(info) => {
                    if info.confirmed_round.is_some() {
                        tracing::info!(
                            tx_id = %tx_id,
                            confirmed_round = ?info.confirmed_round,
                            "Algorand transaction confirmed"
                        );
                        return Ok(());
                    }
                    tracing::debug!(
                        tx_id = %tx_id,
                        attempt = attempt,
                        "Transaction pending..."
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        tx_id = %tx_id,
                        error = %e,
                        attempt = attempt,
                        "Error checking transaction status"
                    );
                }
            }
        }

        // Algorand blocks finalise in ~3s and this polls for 10, so an
        // unconfirmed group here is probably dead -- but the last read may
        // equally have been an RPC error (the `Err` arm above only warns and
        // keeps polling), in which case we never learned anything. Either way
        // the id has to survive the error.
        tracing::warn!(
            tx_id = %tx_id,
            attempts = MAX_ATTEMPTS,
            "Algorand transaction never confirmed within the polling window"
        );
        Err(AlgorandError::TransactionNotConfirmed {
            tx_id: tx_id.to_string(),
            attempts: MAX_ATTEMPTS,
        })
    }

    /// Simulate the transaction group before submission
    ///
    /// This calls the algod /v2/transactions/simulate endpoint to verify the
    /// transaction group would succeed before actually broadcasting it.
    /// This is step 6 in the GoPlausible x402-avm verification flow.
    async fn simulate_group(
        &self,
        signed_group: &[SignedTransaction],
    ) -> Result<(), AlgorandError> {
        // Encode each signed transaction to msgpack then base64
        // The simulate API expects an array of base64 strings, not objects with "txn" keys
        let txn_strings: Vec<String> = signed_group
            .iter()
            .map(|stxn| {
                let encoded = rmp_serde::to_vec_named(stxn).map_err(|e| {
                    AlgorandError::InvalidEncoding(format!("Msgpack encode: {}", e))
                })?;
                Ok(BASE64.encode(&encoded))
            })
            .collect::<Result<Vec<_>, AlgorandError>>()?;

        let request_body = serde_json::json!({
            "txn-groups": [
                {
                    "txns": txn_strings
                }
            ],
            "allow-empty-signatures": false,
            "allow-more-logging": true
        });

        let simulate_url = format!("{}/v2/transactions/simulate", self.algod_url);

        tracing::debug!(
            url = %simulate_url,
            group_size = signed_group.len(),
            "Simulating Algorand transaction group"
        );

        let response = self
            .http_client
            .post(&simulate_url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AlgorandError::SimulationFailed(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlgorandError::SimulationFailed(format!(
                "Simulate API returned {}: {}",
                status, body
            )));
        }

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AlgorandError::SimulationFailed(format!("JSON parse failed: {}", e)))?;

        // Check if simulation passed
        // The response contains txn-groups[0].txn-results[*].txn-result.failure-message
        if let Some(txn_groups) = result.get("txn-groups").and_then(|g| g.as_array()) {
            for group in txn_groups {
                // Check group-level failure
                if let Some(failure) = group.get("failure-message").and_then(|f| f.as_str()) {
                    if !failure.is_empty() {
                        return Err(AlgorandError::SimulationFailed(format!(
                            "Group simulation failed: {}",
                            failure
                        )));
                    }
                }

                // Check individual transaction results
                if let Some(txn_results) = group.get("txn-results").and_then(|r| r.as_array()) {
                    for (i, txn_result) in txn_results.iter().enumerate() {
                        if let Some(failure) = txn_result
                            .get("txn-result")
                            .and_then(|r| r.get("failure-message"))
                            .and_then(|f| f.as_str())
                        {
                            if !failure.is_empty() {
                                return Err(AlgorandError::SimulationFailed(format!(
                                    "Transaction {} simulation failed: {}",
                                    i, failure
                                )));
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(
            group_size = signed_group.len(),
            "Algorand transaction group simulation passed"
        );

        Ok(())
    }
}

/// Whether `note`, the fee transaction's, is one the clients write: none,
/// `FEE_NOTE`, or `FEE_NOTE` and `-` followed by `FEE_NOTE_DIGITS` ASCII
/// digits.
fn fee_note_is_allowed(note: Option<&[u8]>) -> bool {
    let Some(note) = note.filter(|note| !note.is_empty()) else {
        return true;
    };
    let Some(rest) = note.strip_prefix(FEE_NOTE) else {
        return false;
    };
    match rest.strip_prefix(b"-") {
        None => rest.is_empty(),
        Some(digits) => digits.len() == FEE_NOTE_DIGITS && digits.iter().all(u8::is_ascii_digit),
    }
}

/// Whether the authorization `signed` carries is the sender's own, judged
/// from the bytes alone:
/// - a single signature by the sender's key;
/// - a multisignature of the multisig account the sender is;
/// - a logic signature of the sender: the account of its own program, or a
///   program the sender delegated with its signature. The program is not run
///   here. A program delegated with a multisignature (the logic signature's
///   `msig` field, the only one algonaut reads) is refused: algod has refused
///   that field since consensus v41.
///
/// algonaut drops `sgnr` when it decodes, and the facilitator broadcasts the
/// payment re-encoded without it, so only the sender's own key can authorize
/// it. What else the chain decides (a rekeyed sender, the program's verdict,
/// the balance) is left to algod.
fn payment_authorized_by_sender(signed: &SignedTransaction) -> Result<bool, AlgorandError> {
    let sender = signed.transaction.sender();
    let message = signed.transaction.bytes_to_sign().map_err(|e| {
        AlgorandError::InvalidEncoding(format!("Payment transaction encoding failed: {e}"))
    })?;
    Ok(match &signed.sig {
        TransactionSignature::Single(signature) => ed25519_signs(&sender.0, &message, &signature.0),
        TransactionSignature::Multi(msig) => multisig_authorizes(msig, &sender, &message),
        TransactionSignature::Logic(lsig) => match &lsig.sig {
            LogicSignature::ContractAccount => lsig.as_address() == sender,
            LogicSignature::DelegatedSig(signature) => {
                ed25519_signs(&sender.0, &lsig.logic.bytes_to_sign(), &signature.0)
            }
            LogicSignature::DelegatedMultiSig(_) => false,
        },
    })
}

/// Whether `msig` is a multisignature of `account` over `message`, checked
/// as algod checks one: version 1, a threshold of at least one, at most
/// `MAX_MULTISIG_KEYS` keys, keys that hash to `account`, every signature
/// present valid, and at least `threshold` of them (so no fewer keys than
/// the threshold).
fn multisig_authorizes(msig: &MultisigSignature, account: &AlgoAddress, message: &[u8]) -> bool {
    let threshold = usize::from(msig.threshold);
    if msig.version != MULTISIG_VERSION || threshold == 0 {
        return false;
    }
    if msig.subsigs.len() > MAX_MULTISIG_KEYS {
        return false;
    }
    let keys = MultisigAddress {
        version: msig.version,
        threshold: msig.threshold,
        public_keys: msig.subsigs.iter().map(|subsig| subsig.key).collect(),
    };
    if keys.address() != *account {
        return false;
    }
    let mut signatures = 0;
    for subsig in &msig.subsigs {
        if let Some(signature) = &subsig.sig {
            if !ed25519_signs(&subsig.key.0, message, &signature.0) {
                return false;
            }
            signatures += 1;
        }
    }
    signatures >= threshold
}

/// Strict Ed25519 verification: it also refuses small-order keys and nonce
/// points, which a wallet's signature never uses.
fn ed25519_signs(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    ed25519_dalek::VerifyingKey::from_bytes(public_key).is_ok_and(|key| {
        key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(signature))
            .is_ok()
    })
}

/// Result of verifying an Algorand payment group
pub struct VerifyGroupResult {
    pub payer: AlgorandAddress,
    pub fee_tx: AlgoTransaction,
    /// The signed payment transaction (stored for potential future use)
    #[allow(dead_code)]
    pub payment_signed: SignedTransaction,
    pub group_id: [u8; 32],
    pub amount: u64,
    pub recipient: String,
    pub current_round: u64,
    pub last_valid_round: u64,
}

// =============================================================================
// Trait Implementations
// =============================================================================

impl FromEnvByNetworkBuild for AlgorandProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let algod_url = std::env::var(from_env::rpc_env_name_from_network(network)).ok();

        // Get mnemonic from environment
        let mnemonic = match from_env::SignerType::from_env()?.get_algorand_mnemonic(network) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(network=%network, error=%e, "no Algorand mnemonic configured, skipping");
                return Ok(None);
            }
        };

        let provider = AlgorandProvider::try_new(mnemonic, algod_url, network)?;
        Ok(Some(provider))
    }
}

impl NetworkProviderOps for AlgorandProvider {
    fn signer_address(&self) -> MixedAddress {
        self.facilitator_address()
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for AlgorandProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let payload = &request.payment_payload;

        // Verify network matches
        if payload.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                payload.network,
            ));
        }

        // Handle Algorand atomic group payload (GoPlausible x402-avm spec)
        match &payload.payload {
            ExactPaymentPayload::Algorand(p) => {
                let verification = self
                    .verify_payment_group(
                        p,
                        &request.payment_requirements,
                        request.payment_payload.scheme,
                    )
                    .await
                    .map_err(FacilitatorLocalError::from)?;
                // Settle keeps its own atomic claim in submit_group(); this
                // read gives verify the same answer settle would.
                self.check_group_unused(&verification.group_id)
                    .await
                    .map_err(FacilitatorLocalError::from)?;
                Ok(VerifyResponse::valid(verification.payer.into()))
            }
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let payload = &request.payment_payload;

        // Verify network matches
        if payload.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                payload.network,
            ));
        }

        // Handle Algorand atomic group payload (GoPlausible x402-avm spec)
        match &payload.payload {
            ExactPaymentPayload::Algorand(algorand_payload) => {
                tracing::info!("Algorand settle: Verifying payment group");
                let verification = self
                    .verify_payment_group(
                        algorand_payload,
                        &request.payment_requirements,
                        request.payment_payload.scheme,
                    )
                    .await
                    .map_err(FacilitatorLocalError::from)?;

                tracing::info!(
                    payer = %verification.payer.address,
                    amount = verification.amount,
                    recipient = %verification.recipient,
                    "Algorand settle: Verification successful, submitting group"
                );

                // Submit the transaction group
                let submitted = self.submit_group(&verification, algorand_payload).await;
                settle_outcome(submitted, verification.payer.into(), self.network())
            }
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            network_aliases: None,
            extra: Some(SupportedPaymentKindExtra {
                near_escrow: None,
                fee_payer: Some(self.signer_address()),
                tokens: Some(crate::network::exact_payment_tokens(self.network())),
                escrow: None,
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_algorand_address_validation() {
        // Valid Algorand address (58 chars, base32)
        let valid = AlgorandAddress::new(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAY5HFKQ".to_string(),
        );
        assert!(valid.is_valid());

        // Invalid address - too short
        let invalid_short = AlgorandAddress::new("AAAA".to_string());
        assert!(!invalid_short.is_valid());

        // Invalid address - wrong characters
        let invalid_chars = AlgorandAddress::new(
            "0000000000000000000000000000000000000000000000000000000000".to_string(),
        );
        assert!(!invalid_chars.is_valid());
    }

    #[test]
    fn test_chain_config() {
        let mainnet = AlgorandChain::try_from(Network::Algorand).unwrap();
        assert_eq!(mainnet.usdc_asa_id, USDC_ASA_ID_MAINNET);

        let testnet = AlgorandChain::try_from(Network::AlgorandTestnet).unwrap();
        assert_eq!(testnet.usdc_asa_id, USDC_ASA_ID_TESTNET);
    }

    // validate_fee_transaction is exercised through verify and settle in
    // replay_verify_tests below, on groups built with algonaut's builder (no
    // node needed) and on the groups the known clients build.

    #[test]
    fn test_fee_cap_constant_is_sane() {
        // Standard Algorand minimum fee is 1_000 microAlgos per transaction.
        // The cap per transaction sits above it, and the largest group's cap
        // stays below 1 ALGO (1_000_000).
        assert!(MAX_ALGORAND_FEE_PER_GROUP_TXN_MICROALGOS > 1_000);
        assert!(
            MAX_ALGORAND_FEE_PER_GROUP_TXN_MICROALGOS * (MAX_ALGORAND_GROUP_SIZE as u64)
                < 1_000_000
        );
    }

    /// The fee note is absent, `x402-fee-payer` (other public clients), or
    /// the one `@x402/avm` writes: `x402-fee-payer-` and the 13 digits of
    /// `Date.now()`.
    #[test]
    fn only_the_notes_a_known_client_writes_are_allowed_on_the_fee_transaction() {
        for allowed in [
            None,
            Some(&b""[..]),
            Some(&b"x402-fee-payer"[..]),
            Some(&b"x402-fee-payer-1794934694448"[..]),
            Some(&b"x402-fee-payer-1000000000000"[..]),
            Some(&b"x402-fee-payer-9999999999999"[..]),
        ] {
            assert!(fee_note_is_allowed(allowed), "refused {allowed:?}");
        }
        for refused in [
            &b"x402-fee-payer-"[..],
            b"x402-fee-payer-0",
            b"x402-fee-payer-179493469444",
            b"x402-fee-payer-17949346944480",
            b"x402-fee-payer-9007199254740991",
            b"x402-fee-payer-12345678901234567",
            b"x402-fee-payer-1794934694448 ",
            b"x402-fee-payer-1794934694448\n",
            b" x402-fee-payer-1794934694448",
            b"X402-FEE-PAYER-1794934694448",
            b"x402-fee-payer--1794934694448",
            b"x402-fee-payer-17949346944a8",
            "x402-fee-payer-\u{0661}\u{0662}".as_bytes(),
            b"x402-fee-payer ",
            b"x402-fee-payerX",
            b"x402-fee-payer_1794934694448",
            b"x402-fee-paye",
            b"any other text",
            b"\0",
        ] {
            assert!(
                !fee_note_is_allowed(Some(refused)),
                "allowed {:?}",
                String::from_utf8_lossy(refused)
            );
        }
    }
}

/// `verify` against the replay state `settle` writes.
///
/// Uses nothing newer than `verify`, `settle`, the global nonce store and the
/// provider constructor, so the module also runs against the code before the
/// check. algod is a local stub answering `GET /v2/status` and recording,
/// then refusing, any other request; transactions are built with algonaut's
/// builder, which needs no node.
#[cfg(test)]
mod replay_verify_tests {
    use super::*;
    use crate::types::{PaymentPayload, TokenAmount};
    use algonaut::core::{
        CompiledTeal, LogicSignature, MicroAlgos, MultisigAddress, MultisigSignature,
        MultisigSubsig, Round, SuggestedTransactionParams,
    };
    use algonaut::crypto::HashDigest;
    use algonaut::transaction::builder::TxnFee;
    use algonaut::transaction::contract_account::ContractAccount;
    use algonaut::transaction::transaction::{SignedLogic, TransactionSignature};
    use algonaut::transaction::tx_group::TxGroup;
    use algonaut::transaction::{AcceptAsset, Pay, RegisterKey, TransferAsset, TxnBuilder};
    use std::collections::HashSet;
    use std::sync::Mutex;

    const AMOUNT: u64 = 1_000_000;
    const CURRENT_ROUND: u64 = 1_000;

    /// A nonce store whose reads can be made to fail, key by key.
    #[derive(Debug, Default)]
    struct ScriptedStore {
        used: Mutex<HashSet<String>>,
        unreadable: Mutex<HashSet<String>>,
    }

    #[async_trait::async_trait]
    impl NonceStore for ScriptedStore {
        async fn check_and_mark_used(
            &self,
            key: &str,
            _ttl_seconds: u64,
        ) -> Result<(), NonceStoreError> {
            if self.used.lock().unwrap().insert(key.to_string()) {
                Ok(())
            } else {
                Err(NonceStoreError::NonceAlreadyUsed(key.to_string()))
            }
        }

        async fn is_used(&self, key: &str) -> Result<bool, NonceStoreError> {
            if self.unreadable.lock().unwrap().contains(key) {
                return Err(NonceStoreError::ReadError("scripted".to_string()));
            }
            Ok(self.used.lock().unwrap().contains(key))
        }

        async fn health_check(&self) -> Result<(), NonceStoreError> {
            Ok(())
        }

        fn store_type(&self) -> &'static str {
            "scripted"
        }
    }

    static SCRIPTED_STORE: once_cell::sync::Lazy<Arc<ScriptedStore>> =
        once_cell::sync::Lazy::new(Default::default);

    /// The scripted store, installed as the providers' global store.
    fn scripted_store() -> Arc<ScriptedStore> {
        let scripted = SCRIPTED_STORE.clone();
        let installed = GLOBAL_NONCE_STORE.get_or_init(|| scripted.clone());
        assert!(
            std::ptr::addr_eq(Arc::as_ptr(installed), Arc::as_ptr(&scripted)),
            "the Algorand nonce store was initialized before the scripted one"
        );
        scripted
    }

    /// The requests an algod stub received besides `GET /v2/status`, as
    /// "METHOD /path": a broadcast is `POST /v2/transactions`.
    type AlgodCalls = Arc<Mutex<Vec<String>>>;

    /// An algod node at CURRENT_ROUND. Any other request is recorded and
    /// refused with a 400, which a broadcast reads as algod refusing the group.
    async fn algod_stub() -> (String, AlgodCalls) {
        let calls = AlgodCalls::default();
        let recorded = calls.clone();
        let status = serde_json::json!({
            "catchup-time": 0,
            "last-round": CURRENT_ROUND,
            "last-version": "future",
            "next-version": "future",
            "next-version-round": CURRENT_ROUND + 1,
            "next-version-supported": true,
            "stopped-at-unsupported-round": false,
            "time-since-last-round": 0,
        });
        let app = axum::Router::new()
            .route(
                "/v2/status",
                axum::routing::get(move || async move { axum::Json(status) }),
            )
            .fallback(
                move |method: axum::http::Method, uri: axum::http::Uri| async move {
                    recorded
                        .lock()
                        .unwrap()
                        .push(format!("{method} {}", uri.path()));
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"message": "algod stub: not served"})),
                    )
                },
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/"), calls)
    }

    struct Fixture {
        provider: AlgorandProvider,
        request: VerifyRequest,
        group_id: [u8; 32],
        payer: String,
        algod_calls: AlgodCalls,
    }

    /// The payer: the address the transfer is sent from, and how that
    /// account signs it.
    struct Payer {
        address: AlgoAddress,
        sign: Box<dyn FnOnce(AlgoTransaction) -> SignedTransaction>,
    }

    impl Payer {
        /// An account that signs with its own key, as a wallet does.
        fn single() -> Self {
            let account = Account::generate();
            Payer {
                address: account.address(),
                sign: Box::new(move |txn| account.sign_transaction(txn).unwrap()),
            }
        }
    }

    /// A test's group: the fee transaction's body (built from the
    /// facilitator's address), fee, lease and note; the payer; and how many
    /// transactions follow the payment.
    struct Group {
        fee_txn: FeeTxn,
        fee: u64,
        fee_lease: Option<HashDigest>,
        fee_note: Option<Vec<u8>>,
        payer: Payer,
        trailing: u8,
    }

    impl Default for Group {
        /// The group the uvd-x402-sdk clients send: a zero-amount payment
        /// from the facilitator to itself paying 2_000 for both
        /// transactions, without lease or note.
        fn default() -> Self {
            Group {
                fee_txn: Box::new(|f| Pay::new(f, f, MicroAlgos(0)).build()),
                fee: 2_000,
                fee_lease: None,
                fee_note: None,
                payer: Payer::single(),
                trailing: 0,
            }
        }
    }

    /// A group as a client sends it: the facilitator's unsigned fee
    /// transaction, then the payer's signed USDC transfer. `note` gives each
    /// test its own group_id.
    async fn fixture(note: &[u8]) -> Fixture {
        fixture_of(note, Group::default()).await
    }

    /// `fixture` with the fee transaction's body, built from the
    /// facilitator's address, and its lease chosen by the test.
    async fn fixture_with(
        note: &[u8],
        fee_txn: impl FnOnce(AlgoAddress) -> TransactionType + 'static,
        fee_lease: Option<HashDigest>,
    ) -> Fixture {
        let group = Group {
            fee_txn: Box::new(fee_txn),
            fee_lease,
            ..Group::default()
        };
        fixture_of(note, group).await
    }

    /// `fixture` with every part of the group chosen by the test.
    async fn fixture_of(note: &[u8], group: Group) -> Fixture {
        let (algod_url, algod_calls) = algod_stub().await;
        let provider = AlgorandProvider::try_new(
            Account::generate().mnemonic(),
            Some(algod_url),
            Network::AlgorandTestnet,
        )
        .unwrap();
        let facilitator = AlgoAddress::from_str(&provider.public_address).unwrap();
        let pay_to = Account::generate().address();

        let params = SuggestedTransactionParams {
            genesis_id: "testnet-v1.0".to_string(),
            genesis_hash: HashDigest([7u8; 32]),
            consensus_version: "future".to_string(),
            fee_per_byte: MicroAlgos(0),
            min_fee: MicroAlgos(1_000),
            first_valid: Round(CURRENT_ROUND - 10),
            last_valid: Round(CURRENT_ROUND + 500),
        };
        let mut fee_builder = TxnBuilder::with_fee(
            &params,
            TxnFee::Fixed(MicroAlgos(group.fee)),
            (group.fee_txn)(facilitator),
        );
        if let Some(lease) = group.fee_lease {
            fee_builder = fee_builder.lease(lease);
        }
        if let Some(fee_note) = group.fee_note {
            fee_builder = fee_builder.note(fee_note);
        }
        let mut fee_tx = fee_builder.build().unwrap();
        let mut payment_tx = TxnBuilder::with_fee(
            &params,
            TxnFee::zero(),
            TransferAsset::new(group.payer.address, USDC_ASA_ID_TESTNET, AMOUNT, pay_to).build(),
        )
        .note(note.to_vec())
        .build()
        .unwrap();
        // Transactions after the payment, from an account of their own.
        let other = Account::generate();
        let mut trailing: Vec<AlgoTransaction> = (0..group.trailing)
            .map(|i| {
                TxnBuilder::with_fee(
                    &params,
                    TxnFee::zero(),
                    Pay::new(other.address(), other.address(), MicroAlgos(0)).build(),
                )
                .note(vec![i])
                .build()
                .unwrap()
            })
            .collect();
        let mut members: Vec<&mut AlgoTransaction> = vec![&mut fee_tx, &mut payment_tx];
        members.extend(trailing.iter_mut());
        TxGroup::assign_group_id(&mut members).unwrap();
        let group_id = fee_tx.group.unwrap().0;
        let payment_signed = (group.payer.sign)(payment_tx);

        let mut payment_group = vec![
            BASE64.encode(rmp_serde::to_vec_named(&fee_tx).unwrap()),
            BASE64.encode(rmp_serde::to_vec_named(&payment_signed).unwrap()),
        ];
        payment_group.extend(trailing.into_iter().map(|txn| {
            BASE64.encode(rmp_serde::to_vec_named(&other.sign_transaction(txn).unwrap()).unwrap())
        }));
        let payload = ExactAlgorandPayload {
            payment_index: 1,
            payment_group,
        };

        Fixture {
            provider,
            request: verify_request(payload, pay_to.to_string(), AMOUNT, USDC_ASA_ID_TESTNET),
            group_id,
            payer: group.payer.address.to_string(),
            algod_calls,
        }
    }

    /// A testnet request for `payload` paying `amount` of ASA `asset` to
    /// `pay_to`.
    fn verify_request(
        payload: ExactAlgorandPayload,
        pay_to: String,
        amount: u64,
        asset: u64,
    ) -> VerifyRequest {
        VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload: PaymentPayload {
                x402_version: X402Version::V1,
                scheme: Scheme::Exact,
                network: Network::AlgorandTestnet,
                payload: ExactPaymentPayload::Algorand(payload),
            },
            payment_requirements: PaymentRequirements {
                scheme: Scheme::Exact,
                network: Network::AlgorandTestnet,
                max_amount_required: TokenAmount(alloy::primitives::U256::from(amount)),
                resource: url::Url::parse("https://example.com/paid").unwrap(),
                description: String::new(),
                mime_type: "application/json".to_string(),
                output_schema: None,
                pay_to: MixedAddress::Algorand(pay_to),
                max_timeout_seconds: 60,
                asset: MixedAddress::Offchain(asset.to_string()),
                extra: None,
            },
        }
    }

    /// The group a known client built, from `tests/fixtures/algorand/`
    /// (README there), before a provider that answers for the fixture's
    /// facilitator address. verify signs nothing; settle signs the fee
    /// transaction with the provider's own key, and the stub records the
    /// broadcast and refuses it.
    async fn client_fixture(json: &str) -> Fixture {
        let fixture: serde_json::Value = serde_json::from_str(json).unwrap();
        let text = |key: &str| fixture[key].as_str().unwrap().to_string();
        let (algod_url, algod_calls) = algod_stub().await;
        let mut provider = AlgorandProvider::try_new(
            Account::generate().mnemonic(),
            Some(algod_url),
            Network::AlgorandTestnet,
        )
        .unwrap();
        provider.public_address = text("facilitator");
        let payment_group: Vec<String> = fixture["paymentGroup"]
            .as_array()
            .unwrap()
            .iter()
            .map(|txn| txn.as_str().unwrap().to_string())
            .collect();
        let fee_tx: AlgoTransaction =
            rmp_serde::from_slice(&BASE64.decode(&payment_group[0]).unwrap()).unwrap();
        let payload = ExactAlgorandPayload {
            payment_index: usize::try_from(fixture["paymentIndex"].as_u64().unwrap()).unwrap(),
            payment_group,
        };
        Fixture {
            provider,
            request: verify_request(
                payload,
                text("payTo"),
                text("amount").parse().unwrap(),
                text("asset").parse().unwrap(),
            ),
            group_id: fee_tx.group.unwrap().0,
            payer: text("payer"),
            algod_calls,
        }
    }

    /// Settles `f`'s group, which settle must refuse with `reason`; the error
    /// names what happened instead: a request to algod besides its status, a
    /// claim on the group in the nonce store, or another answer. submit_group()
    /// claims the group before it signs the fee transaction, so an unclaimed
    /// group was never signed.
    async fn settle_refused(
        f: &Fixture,
        store: &ScriptedStore,
        reason: &str,
    ) -> Result<(), String> {
        let outcome = f.provider.settle(&f.request).await;
        let sent = f.algod_calls.lock().unwrap().clone();
        let key = algorand_nonce_key(f.provider.chain_name(), &f.group_id);
        let claimed = store.used.lock().unwrap().contains(&key);
        let refused = matches!(
            &outcome,
            Err(FacilitatorLocalError::Other(msg) | FacilitatorLocalError::InvalidSignature(_, msg))
                if msg.contains(reason)
        );
        if sent.is_empty() && !claimed && refused {
            Ok(())
        } else {
            Err(format!(
                "algod got {sent:?}, group claimed: {claimed}, settle answered {outcome:?}"
            ))
        }
    }

    #[tokio::test]
    async fn verify_rejects_a_group_settle_already_claimed() {
        let store = scripted_store();
        let f = fixture(b"claimed").await;
        let key = algorand_nonce_key(f.provider.chain_name(), &f.group_id);
        store.check_and_mark_used(&key, 3_600).await.unwrap();

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("a group settle already claimed must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("already used")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_accepts_a_group_not_yet_settled() {
        scripted_store();
        let f = fixture(b"unclaimed").await;

        let response = f
            .provider
            .verify(&f.request)
            .await
            .expect("an unclaimed group verifies");
        assert!(
            matches!(&response, VerifyResponse::Valid { payer } if *payer == MixedAddress::Algorand(f.payer.clone())),
            "got {response:?}"
        );
    }

    #[tokio::test]
    async fn verify_fails_closed_when_the_nonce_store_cannot_be_read() {
        let store = scripted_store();
        let f = fixture(b"unreadable").await;
        let key = algorand_nonce_key(f.provider.chain_name(), &f.group_id);
        store.unreadable.lock().unwrap().insert(key);

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("an unreadable nonce store must not vouch for a group");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("unavailable")),
            "got {err:?}"
        );
    }

    /// A fee transaction's body, built from the facilitator's address.
    type FeeTxn = Box<dyn FnOnce(AlgoAddress) -> TransactionType>;

    /// Fee transactions that are not a zero-amount payment, each with the
    /// reason verify and settle give for refusing it.
    fn fee_transactions_that_are_not_a_zero_amount_payment(
    ) -> Vec<(&'static str, FeeTxn, &'static str)> {
        let other = Account::generate().address();
        vec![
            (
                "pay with an amount",
                Box::new(move |f| Pay::new(f, other, MicroAlgos(1_000_000)).build()),
                "forbidden fields: amount",
            ),
            (
                "asset transfer",
                Box::new(move |f| TransferAsset::new(f, USDC_ASA_ID_TESTNET, 1, other).build()),
                "must be a payment",
            ),
            (
                "asset opt-in",
                Box::new(|f| AcceptAsset::new(f, USDC_ASA_ID_TESTNET).build()),
                "must be a payment",
            ),
            (
                "key registration",
                Box::new(|f| RegisterKey::offline(f).build()),
                "must be a payment",
            ),
        ]
    }

    /// The facilitator signs the fee transaction the client built, so it may
    /// only be a payment that moves nothing: any amount, or any other type,
    /// would spend the facilitator's funds or act for its account.
    #[tokio::test]
    async fn verify_rejects_a_fee_transaction_that_is_not_a_zero_amount_payment() {
        scripted_store();
        let mut verified = Vec::new();
        for (case, fee_txn, reason) in fee_transactions_that_are_not_a_zero_amount_payment() {
            let f = fixture_with(case.as_bytes(), fee_txn, None).await;
            match f.provider.verify(&f.request).await {
                Ok(response) => verified.push(format!("{case}: {response:?}")),
                Err(err) => assert!(
                    matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains(reason)),
                    "{case}: got {err:?}"
                ),
            }
        }
        assert!(
            verified.is_empty(),
            "fee transactions that verified: {verified:#?}"
        );
    }

    #[tokio::test]
    async fn verify_rejects_a_fee_transaction_with_a_lease() {
        scripted_store();
        let f = fixture_with(
            b"fee lease",
            |f| Pay::new(f, f, MicroAlgos(0)).build(),
            Some(HashDigest([9u8; 32])),
        )
        .await;

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("a fee transaction with a lease must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("forbidden fields: lease")),
            "got {err:?}"
        );
    }

    /// The facilitator signs the fee transaction, so it may only pay the
    /// facilitator itself: every known client sends a self-payment, and the
    /// official facilitator refuses any other receiver.
    #[tokio::test]
    async fn verify_rejects_a_fee_payment_to_another_receiver() {
        scripted_store();
        let other = Account::generate().address();
        let f = fixture_with(
            b"fee to another receiver",
            move |f| Pay::new(f, other, MicroAlgos(0)).build(),
            None,
        )
        .await;

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("a fee payment to another receiver must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("receiver mismatch")),
            "got {err:?}"
        );
    }

    /// Fee notes no known client writes, each named.
    fn fee_notes_no_client_writes() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("text", b"any other text".to_vec()),
            ("the prefix alone", b"x402-fee-payer-".to_vec()),
            ("twelve digits", b"x402-fee-payer-179493469444".to_vec()),
            ("fourteen digits", b"x402-fee-payer-17949346944480".to_vec()),
            (
                "digits and text",
                b"x402-fee-payer-1794934694448 more".to_vec(),
            ),
        ]
    }

    /// The fee transaction's note is published from the facilitator's
    /// account: only the one a known client writes is signed.
    #[tokio::test]
    async fn verify_rejects_a_fee_note_no_client_writes() {
        scripted_store();
        let mut verified = Vec::new();
        for (case, note) in fee_notes_no_client_writes() {
            let group = Group {
                fee_note: Some(note),
                ..Group::default()
            };
            let f = fixture_of(format!("fee note: {case}").as_bytes(), group).await;
            match f.provider.verify(&f.request).await {
                Ok(response) => verified.push(format!("{case}: {response:?}")),
                Err(err) => assert!(
                    matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("forbidden fields: note")),
                    "{case}: got {err:?}"
                ),
            }
        }
        assert!(
            verified.is_empty(),
            "fee notes that verified: {verified:#?}"
        );
    }

    /// `@x402/avm` writes `x402-fee-payer-` and `Date.now()` on its fee
    /// transaction, other public clients `x402-fee-payer` alone.
    #[tokio::test]
    async fn verify_accepts_the_fee_notes_clients_write() {
        scripted_store();
        let mut refused = Vec::new();
        for note in [&b"x402-fee-payer-1794934694448"[..], b"x402-fee-payer"] {
            let group = Group {
                fee_note: Some(note.to_vec()),
                ..Group::default()
            };
            let case = String::from_utf8_lossy(note).into_owned();
            let f = fixture_of(format!("client fee note: {case}").as_bytes(), group).await;
            match f.provider.verify(&f.request).await {
                Ok(VerifyResponse::Valid { payer })
                    if payer == MixedAddress::Algorand(f.payer.clone()) => {}
                other => refused.push(format!("{case}: {other:?}")),
            }
        }
        assert!(refused.is_empty(), "client fee notes refused: {refused:#?}");
    }

    /// The fee transaction pays for its whole group: up to 5_000 microAlgos
    /// for each transaction in it.
    #[tokio::test]
    async fn verify_holds_the_fee_to_the_size_of_its_group() {
        scripted_store();
        let mut wrong = Vec::new();
        for (case, fee, trailing, refusal) in [
            ("2 transactions, fee 10_000", 10_000, 0, None),
            (
                "2 transactions, fee 10_001",
                10_001,
                0,
                Some("exceeds maximum allowed 10000"),
            ),
            ("3 transactions, fee 15_000", 15_000, 1, None),
            (
                "3 transactions, fee 15_001",
                15_001,
                1,
                Some("exceeds maximum allowed 15000"),
            ),
            ("16 transactions, fee 80_000", 80_000, 14, None),
        ] {
            let group = Group {
                fee,
                trailing,
                ..Group::default()
            };
            let f = fixture_of(case.as_bytes(), group).await;
            let outcome = f.provider.verify(&f.request).await;
            let as_expected = match (refusal, &outcome) {
                (None, Ok(VerifyResponse::Valid { .. })) => true,
                (Some(reason), Err(FacilitatorLocalError::Other(msg))) => msg.contains(reason),
                _ => false,
            };
            if !as_expected {
                wrong.push(format!("{case}: {outcome:?}"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Appends a copy of the group's last transaction to `f`'s payload, past
    /// what algonaut lets a test group: 16 transactions.
    fn append_a_transaction(f: &mut Fixture) {
        let ExactPaymentPayload::Algorand(payload) = &mut f.request.payment_payload.payload else {
            unreachable!("the fixture builds an Algorand payload")
        };
        let last = payload.payment_group.last().unwrap().clone();
        payload.payment_group.push(last);
    }

    /// The fee cap grows with the group, so the group is held to the 16
    /// transactions algod accepts.
    #[tokio::test]
    async fn verify_rejects_a_group_larger_than_algod_accepts() {
        scripted_store();
        let group = Group {
            trailing: 14,
            ..Group::default()
        };
        let mut f = fixture_of(b"17 transactions", group).await;
        append_a_transaction(&mut f);

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("a group of 17 transactions must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("at most 16 allowed")),
            "got {err:?}"
        );
    }

    /// One msgpack map holding the entries of two maps whose keys differ.
    fn merged_msgpack_maps(a: &[u8], b: &[u8]) -> Vec<u8> {
        fn entries(map: &[u8]) -> (usize, &[u8]) {
            match map[0] {
                header @ 0x80..=0x8f => (usize::from(header & 0x0f), &map[1..]),
                0xde => (usize::from(u16::from_be_bytes([map[1], map[2]])), &map[3..]),
                other => panic!("not a msgpack map header: {other:#04x}"),
            }
        }
        let (a_len, a_entries) = entries(a);
        let (b_len, b_entries) = entries(b);
        let mut merged = vec![0xde];
        merged.extend_from_slice(&u16::try_from(a_len + b_len).unwrap().to_be_bytes());
        merged.extend_from_slice(a_entries);
        merged.extend_from_slice(b_entries);
        merged
    }

    /// Puts at index 0 one blob that reads both as `f`'s fee transaction and
    /// as its signed transfer, and points the payment index at it.
    fn put_the_payment_at_the_fee_transaction_index(f: &mut Fixture) {
        let ExactPaymentPayload::Algorand(payload) = &mut f.request.payment_payload.payload else {
            unreachable!("the fixture builds an Algorand payload")
        };
        let fee_blob = BASE64.decode(&payload.payment_group[0]).unwrap();
        let payment_blob = BASE64.decode(&payload.payment_group[1]).unwrap();
        payload.payment_group[0] = BASE64.encode(merged_msgpack_maps(&fee_blob, &payment_blob));
        payload.payment_index = 0;
    }

    /// Index 0 is the fee transaction, which settle replaces with the one the
    /// facilitator signs: a payment checked there is never broadcast. One
    /// blob that reads both as the fee transaction and as the signed
    /// transfer must not verify.
    #[tokio::test]
    async fn verify_rejects_the_payment_at_the_fee_transaction_index() {
        scripted_store();
        let mut f = fixture(b"payment at index 0").await;
        put_the_payment_at_the_fee_transaction_index(&mut f);

        let err = f
            .provider
            .verify(&f.request)
            .await
            .expect_err("a payment at the fee transaction's index must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("payment index 0")),
            "got {err:?}"
        );
    }

    /// settle checks the fee transaction as verify does, before it claims,
    /// signs or broadcasts the group.
    #[tokio::test]
    async fn settle_rejects_a_fee_transaction_that_is_not_a_zero_amount_payment() {
        let store = scripted_store();
        let mut settled = Vec::new();
        for (case, fee_txn, reason) in fee_transactions_that_are_not_a_zero_amount_payment() {
            let f = fixture_with(format!("settle: {case}").as_bytes(), fee_txn, None).await;
            if let Err(problem) = settle_refused(&f, &store, reason).await {
                settled.push(format!("{case}: {problem}"));
            }
        }
        assert!(
            settled.is_empty(),
            "fee transactions settle did not refuse: {settled:#?}"
        );
    }

    #[tokio::test]
    async fn settle_rejects_a_fee_transaction_with_a_lease() {
        let store = scripted_store();
        let f = fixture_with(
            b"settle: fee lease",
            |f| Pay::new(f, f, MicroAlgos(0)).build(),
            Some(HashDigest([9u8; 32])),
        )
        .await;

        settle_refused(&f, &store, "forbidden fields: lease")
            .await
            .unwrap_or_else(|problem| panic!("a fee transaction with a lease: {problem}"));
    }

    #[tokio::test]
    async fn settle_rejects_the_payment_at_the_fee_transaction_index() {
        let store = scripted_store();
        let mut f = fixture(b"settle: payment at index 0").await;
        put_the_payment_at_the_fee_transaction_index(&mut f);

        settle_refused(&f, &store, "payment index 0")
            .await
            .unwrap_or_else(|problem| {
                panic!("a payment at the fee transaction's index: {problem}")
            });
    }

    #[tokio::test]
    async fn settle_rejects_a_fee_payment_to_another_receiver() {
        let store = scripted_store();
        let other = Account::generate().address();
        let f = fixture_with(
            b"settle: fee to another receiver",
            move |f| Pay::new(f, other, MicroAlgos(0)).build(),
            None,
        )
        .await;

        settle_refused(&f, &store, "receiver mismatch")
            .await
            .unwrap_or_else(|problem| panic!("a fee payment to another receiver: {problem}"));
    }

    #[tokio::test]
    async fn settle_rejects_a_fee_note_no_client_writes() {
        let store = scripted_store();
        let mut settled = Vec::new();
        for (case, note) in fee_notes_no_client_writes() {
            let group = Group {
                fee_note: Some(note),
                ..Group::default()
            };
            let f = fixture_of(format!("settle: fee note: {case}").as_bytes(), group).await;
            if let Err(problem) = settle_refused(&f, &store, "forbidden fields: note").await {
                settled.push(format!("{case}: {problem}"));
            }
        }
        assert!(
            settled.is_empty(),
            "fee notes settle did not refuse: {settled:#?}"
        );
    }

    #[tokio::test]
    async fn settle_rejects_a_fee_above_the_cap_of_its_group() {
        let store = scripted_store();
        let group = Group {
            fee: 10_001,
            ..Group::default()
        };
        let f = fixture_of(b"settle: 2 transactions, fee 10_001", group).await;

        settle_refused(&f, &store, "exceeds maximum allowed 10000")
            .await
            .unwrap_or_else(|problem| panic!("a fee above the cap of its group: {problem}"));
    }

    #[tokio::test]
    async fn settle_rejects_a_group_larger_than_algod_accepts() {
        let store = scripted_store();
        let group = Group {
            trailing: 14,
            ..Group::default()
        };
        let mut f = fixture_of(b"settle: 17 transactions", group).await;
        append_a_transaction(&mut f);

        settle_refused(&f, &store, "at most 16 allowed")
            .await
            .unwrap_or_else(|problem| panic!("a group of 17 transactions: {problem}"));
    }

    /// What the refusals above rely on: a group settle accepts is claimed
    /// and reaches algod's broadcast, which the stub records.
    #[tokio::test]
    async fn settle_claims_and_broadcasts_a_group_it_accepts() {
        let store = scripted_store();
        let f = fixture(b"settle: accepted group").await;

        let outcome = f.provider.settle(&f.request).await;
        let key = algorand_nonce_key(f.provider.chain_name(), &f.group_id);
        assert!(
            store.used.lock().unwrap().contains(&key),
            "the group was not claimed; settle answered {outcome:?}"
        );
        assert_eq!(
            *f.algod_calls.lock().unwrap(),
            ["POST /v2/transactions"],
            "settle answered {outcome:?}"
        );
    }

    /// The signature `account` makes over `message`, as a wallet makes it.
    fn raw_signature(account: &Account, message: &[u8]) -> algonaut::crypto::Signature {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&account.seed());
        algonaut::crypto::Signature(key.sign(message).to_bytes())
    }

    /// The multisig account of `keys` with `version` and `threshold`.
    fn multisig_account(version: u8, threshold: u8, keys: &[Account]) -> AlgoAddress {
        MultisigAddress {
            version,
            threshold,
            public_keys: keys
                .iter()
                .map(|key| key.address().as_public_key())
                .collect(),
        }
        .address()
    }

    /// A multisignature over `message` of `keys` with `version` and
    /// `threshold`, carrying the signatures of the keys at `signers`.
    fn multisignature(
        version: u8,
        threshold: u8,
        keys: &[Account],
        signers: &[usize],
        message: &[u8],
    ) -> MultisigSignature {
        MultisigSignature {
            version,
            threshold,
            subsigs: keys
                .iter()
                .enumerate()
                .map(|(i, key)| MultisigSubsig {
                    key: key.address().as_public_key(),
                    sig: signers.contains(&i).then(|| raw_signature(key, message)),
                })
                .collect(),
        }
    }

    /// `txn` carrying `sig`, as a client sends it.
    fn signed(txn: AlgoTransaction, sig: TransactionSignature) -> SignedTransaction {
        SignedTransaction {
            transaction_id: txn.id().unwrap(),
            transaction: txn,
            sig,
            auth_address: None,
        }
    }

    /// A program the tests sign as logic; it is never run.
    fn program(byte: u8) -> CompiledTeal {
        CompiledTeal(vec![0x06, 0x81, byte])
    }

    /// A payer that is a multisig account of `keys` keys, with `version` and
    /// `threshold`, signing with the keys at `signers`.
    fn multisig_payer(keys: usize, version: u8, threshold: u8, signers: &'static [usize]) -> Payer {
        let keys: Vec<Account> = (0..keys).map(|_| Account::generate()).collect();
        Payer {
            address: multisig_account(version, threshold, &keys),
            sign: Box::new(move |txn| {
                let message = txn.bytes_to_sign().unwrap();
                let msig = multisignature(version, threshold, &keys, signers, &message);
                signed(txn, TransactionSignature::Multi(msig))
            }),
        }
    }

    /// A payer whose transfer carries `sig` over `program(1)` as a logic
    /// signature.
    fn logic_payer(address: AlgoAddress, sig: LogicSignature) -> Payer {
        Payer {
            address,
            sign: Box::new(move |txn| {
                let logic = SignedLogic {
                    logic: program(1),
                    args: vec![],
                    sig,
                };
                signed(txn, TransactionSignature::Logic(logic))
            }),
        }
    }

    /// Authorizations of a payment's sender, each named: an ordinary
    /// account's signature, a multisig account's multisignature, and a logic
    /// signature of a contract account or of a delegating one (as algonaut
    /// encodes a logic signature).
    fn authorizations_the_sender_gives() -> Vec<(&'static str, Payer)> {
        let contract = ContractAccount::new(program(1));
        let delegator = Account::generate();
        vec![
            ("single signature", Payer::single()),
            (
                "multisig 2 of 3, two signatures",
                multisig_payer(3, 1, 2, &[0, 1]),
            ),
            (
                "multisig 2 of 3, three signatures",
                multisig_payer(3, 1, 2, &[0, 1, 2]),
            ),
            (
                "multisig of 255 keys, threshold 1",
                multisig_payer(255, 1, 1, &[254]),
            ),
            (
                "contract account",
                Payer {
                    address: *contract.address(),
                    sign: Box::new(move |txn| contract.sign(txn, vec![]).unwrap()),
                },
            ),
            (
                "delegated logic signature",
                logic_payer(
                    delegator.address(),
                    LogicSignature::DelegatedSig(delegator.generate_program_sig(&program(1))),
                ),
            ),
        ]
    }

    /// Authorizations the payment's sender did not give, or that algod
    /// refuses, each named.
    fn authorizations_the_sender_did_not_give() -> Vec<(&'static str, Payer)> {
        let zeroed = Account::generate().address();
        let rekeyed = Account::generate().address();
        let rekeyed_to = Account::generate();
        let mut small_order_key = [0; 32];
        small_order_key[0] = 1;
        let other_keys = {
            let keys: Vec<Account> = (0..3).map(|_| Account::generate()).collect();
            let address = multisig_account(1, 2, &keys);
            let mut others = keys;
            others[2] = Account::generate();
            Payer {
                address,
                sign: Box::new(move |txn| {
                    let message = txn.bytes_to_sign().unwrap();
                    let msig = multisignature(1, 2, &others, &[0, 1], &message);
                    signed(txn, TransactionSignature::Multi(msig))
                }),
            }
        };
        let invalid_beside_valid = {
            let keys: Vec<Account> = (0..3).map(|_| Account::generate()).collect();
            Payer {
                address: multisig_account(1, 2, &keys),
                sign: Box::new(move |txn| {
                    let message = txn.bytes_to_sign().unwrap();
                    let mut msig = multisignature(1, 2, &keys, &[0, 1], &message);
                    msig.subsigs[2].sig = Some(algonaut::crypto::Signature([1; 64]));
                    signed(txn, TransactionSignature::Multi(msig))
                }),
            }
        };
        let contract = ContractAccount::new(program(1));
        let delegator = Account::generate().address();
        let not_the_delegator = Account::generate();
        let delegators: Vec<Account> = (0..3).map(|_| Account::generate()).collect();
        vec![
            (
                "single signature of zeros",
                Payer {
                    address: zeroed,
                    sign: Box::new(|txn| {
                        let zeros = algonaut::crypto::Signature([0; 64]);
                        signed(txn, TransactionSignature::Single(zeros))
                    }),
                },
            ),
            (
                "single signature by another key, as a rekeyed account's",
                Payer {
                    address: rekeyed,
                    sign: Box::new(move |txn| rekeyed_to.sign_transaction(txn).unwrap()),
                },
            ),
            (
                "single signature that only holds for a small-order key",
                Payer {
                    // The identity point as a key, and R = identity, s = 0:
                    // the cofactorless equation holds for any message.
                    address: AlgoAddress(small_order_key),
                    sign: Box::new(move |txn| {
                        let mut forged = [0; 64];
                        forged[..32].copy_from_slice(&small_order_key);
                        signed(
                            txn,
                            TransactionSignature::Single(algonaut::crypto::Signature(forged)),
                        )
                    }),
                },
            ),
            (
                "multisig below its threshold",
                multisig_payer(3, 1, 2, &[0]),
            ),
            ("multisig of other keys", other_keys),
            (
                "multisig with an invalid signature beside two valid ones",
                invalid_beside_valid,
            ),
            ("multisig of version 2", multisig_payer(3, 2, 2, &[0, 1])),
            ("multisig of version 0", multisig_payer(3, 0, 2, &[0, 1])),
            ("multisig of threshold 0", multisig_payer(3, 1, 0, &[])),
            (
                "multisig of 256 keys, threshold 1",
                multisig_payer(256, 1, 1, &[255]),
            ),
            (
                "contract account of another program",
                Payer {
                    address: *ContractAccount::new(program(2)).address(),
                    sign: Box::new(move |txn| contract.sign(txn, vec![]).unwrap()),
                },
            ),
            (
                "delegated logic signature by another key",
                logic_payer(
                    delegator,
                    LogicSignature::DelegatedSig(
                        not_the_delegator.generate_program_sig(&program(1)),
                    ),
                ),
            ),
            (
                "delegated logic multisignature 2 of 3, a field algod refuses since consensus v41",
                logic_payer(
                    multisig_account(1, 2, &delegators),
                    LogicSignature::DelegatedMultiSig(multisignature(
                        1,
                        2,
                        &delegators,
                        &[0, 1],
                        &program(1).bytes_to_sign(),
                    )),
                ),
            ),
        ]
    }

    /// verify reports as the payer the account that authorized the transfer,
    /// with any authorization algod accepts from it.
    #[tokio::test]
    async fn verify_accepts_every_authorization_the_sender_gives() {
        scripted_store();
        let mut refused = Vec::new();
        for (case, payer) in authorizations_the_sender_gives() {
            let group = Group {
                payer,
                ..Group::default()
            };
            let f = fixture_of(format!("authorized: {case}").as_bytes(), group).await;
            match f.provider.verify(&f.request).await {
                Ok(VerifyResponse::Valid { payer })
                    if payer == MixedAddress::Algorand(f.payer.clone()) => {}
                other => refused.push(format!("{case}: {other:?}")),
            }
        }
        assert!(
            refused.is_empty(),
            "authorizations verify refused: {refused:#?}"
        );
    }

    /// A transfer its sender did not authorize does not verify: verify would
    /// report as the payer an account that never agreed to pay.
    #[tokio::test]
    async fn verify_rejects_an_authorization_the_sender_did_not_give() {
        scripted_store();
        let mut verified = Vec::new();
        for (case, payer) in authorizations_the_sender_did_not_give() {
            let group = Group {
                payer,
                ..Group::default()
            };
            let f = fixture_of(format!("not authorized: {case}").as_bytes(), group).await;
            match f.provider.verify(&f.request).await {
                Err(FacilitatorLocalError::InvalidSignature(payer, _))
                    if payer == MixedAddress::Algorand(f.payer.clone()) => {}
                other => verified.push(format!("{case}: {other:?}")),
            }
        }
        assert!(
            verified.is_empty(),
            "authorizations verify did not refuse as invalid_signature: {verified:#?}"
        );
    }

    #[tokio::test]
    async fn settle_rejects_an_authorization_the_sender_did_not_give() {
        let store = scripted_store();
        let mut settled = Vec::new();
        for (case, payer) in authorizations_the_sender_did_not_give() {
            let group = Group {
                payer,
                ..Group::default()
            };
            let f = fixture_of(format!("settle: not authorized: {case}").as_bytes(), group).await;
            if let Err(problem) = settle_refused(&f, &store, "Invalid signature").await {
                settled.push(format!("{case}: {problem}"));
            }
        }
        assert!(
            settled.is_empty(),
            "authorizations settle did not refuse: {settled:#?}"
        );
    }

    /// The groups the known clients build, from `tests/fixtures/algorand/`.
    const CLIENT_GROUPS: [(&str, &str); 3] = [
        (
            "uvd-x402-sdk (TypeScript)",
            include_str!("../../tests/fixtures/algorand/uvd-x402-sdk-typescript.json"),
        ),
        (
            "uvd-x402-sdk (Python)",
            include_str!("../../tests/fixtures/algorand/uvd-x402-sdk-python.json"),
        ),
        (
            "@x402/avm",
            include_str!("../../tests/fixtures/algorand/x402-avm.json"),
        ),
    ];

    /// What the known clients send passes every check: verify reports their
    /// payer, and settle claims the group and broadcasts it. Verify and
    /// settle run in the one test because the group ids are fixed and the
    /// replay store is shared.
    #[tokio::test]
    async fn the_groups_the_known_clients_build_verify_and_settle() {
        let store = scripted_store();
        for (client, json) in CLIENT_GROUPS {
            let f = client_fixture(json).await;

            // settle broadcasts algonaut's re-encoding of the transactions it
            // decoded: the group id the client computed has to hold over
            // them, or algod refuses the group. (The payer's signature over
            // the re-encoded payment is what verify checks below.)
            let ExactPaymentPayload::Algorand(payload) = &f.request.payment_payload.payload else {
                unreachable!("the fixture builds an Algorand payload")
            };
            let fee_tx: AlgoTransaction =
                rmp_serde::from_slice(&BASE64.decode(&payload.payment_group[0]).unwrap()).unwrap();
            let payment: SignedTransaction =
                rmp_serde::from_slice(&BASE64.decode(&payload.payment_group[1]).unwrap()).unwrap();
            let (mut fee_body, mut payment_body) = (fee_tx.clone(), payment.transaction);
            fee_body.group = None;
            payment_body.group = None;
            TxGroup::assign_group_id(&mut [&mut fee_body, &mut payment_body]).unwrap();
            assert_eq!(
                fee_body.group, fee_tx.group,
                "{client}: the group id does not hold over the transactions as settle re-encodes them"
            );

            let verified = f.provider.verify(&f.request).await;
            assert!(
                matches!(&verified, Ok(VerifyResponse::Valid { payer }) if *payer == MixedAddress::Algorand(f.payer.clone())),
                "{client}: verify answered {verified:?}"
            );

            let settled = f.provider.settle(&f.request).await;
            let key = algorand_nonce_key(f.provider.chain_name(), &f.group_id);
            assert!(
                store.used.lock().unwrap().contains(&key),
                "{client}: the group was not claimed; settle answered {settled:?}"
            );
            assert_eq!(
                *f.algod_calls.lock().unwrap(),
                ["POST /v2/transactions"],
                "{client}: settle answered {settled:?}"
            );
        }
    }
}

/// A broadcast whose answer was lost keeps the group's id; one algod refused
/// stays a failed submission.
#[cfg(test)]
mod broadcast_outcome_tests {
    use super::*;
    use algonaut::error::{RequestError, RequestErrorDetails, ServiceError};

    fn http(status: u16, message: &str) -> ServiceError {
        ServiceError::Request(RequestError::new(
            None,
            RequestErrorDetails::Http {
                status,
                message: message.to_string(),
            },
        ))
    }

    #[test]
    fn only_a_refusal_proves_the_group_never_entered_the_pool() {
        let id = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string();
        for refused in [
            http(
                400,
                "TransactionPool.Remember: transaction group had an invalid signature",
            ),
            http(401, "invalid API token"),
            ServiceError::Msg("msgpack encoding failed".into()),
        ] {
            assert!(
                matches!(
                    broadcast_error(refused, id.clone()),
                    AlgorandError::SubmissionFailed(_)
                ),
                "a refusal became unconfirmed"
            );
        }
        for lost in [
            http(502, ""),
            http(
                400,
                "TransactionPool.Remember: transaction already in ledger: ABC",
            ),
            ServiceError::Request(RequestError::new(None, RequestErrorDetails::Timeout)),
            ServiceError::Request(RequestError::new(
                None,
                RequestErrorDetails::Client {
                    description: "error decoding response body".into(),
                },
            )),
        ] {
            match broadcast_error(lost, id.clone()) {
                AlgorandError::SubmissionUnconfirmed { tx_id, .. } => assert_eq!(tx_id, id),
                other => panic!("expected SubmissionUnconfirmed, got {other:?}"),
            }
        }
    }

    /// The settle answer: a group submitted with no verdict is an error
    /// carrying its id, never the `200 success:false` that says nothing was
    /// paid.
    #[test]
    fn a_group_with_no_verdict_is_not_a_failed_settlement() {
        let payer = MixedAddress::Algorand("PAYER".to_string());
        let id = "A".repeat(52);
        for unconfirmed in [
            AlgorandError::SubmissionUnconfirmed {
                tx_id: id.clone(),
                detail: "timeout".into(),
            },
            AlgorandError::TransactionNotConfirmed {
                tx_id: id.clone(),
                attempts: 20,
            },
        ] {
            match settle_outcome(Err(unconfirmed), payer.clone(), Network::AlgorandTestnet) {
                Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                    assert_eq!(tx, TransactionHash::Algorand(id.clone()));
                    assert_eq!(network, Network::AlgorandTestnet);
                }
                other => panic!("expected the unconfirmed error, got {other:?}"),
            }
        }
        let refused = settle_outcome(
            Err(AlgorandError::SubmissionFailed("rejected".into())),
            payer.clone(),
            Network::AlgorandTestnet,
        )
        .expect("a refusal is a settle verdict");
        assert!(!refused.success);
        assert!(refused.transaction.is_none());
        let settled = settle_outcome(Ok(id.clone()), payer, Network::AlgorandTestnet).unwrap();
        assert!(settled.success);
        assert_eq!(settled.transaction, Some(TransactionHash::Algorand(id)));
    }

    /// The predicate is wired to the real broadcast: algod's gateway error is
    /// unconfirmed under the group's first transaction id, a pool refusal is
    /// a failed submission.
    #[tokio::test]
    async fn the_broadcast_reports_a_lost_answer_under_the_first_id() {
        use algonaut::core::{MicroAlgos, Round, SuggestedTransactionParams};
        use algonaut::crypto::HashDigest;
        use algonaut::transaction::builder::TxnFee;
        use algonaut::transaction::{Pay, TxnBuilder};
        use axum::{response::IntoResponse, routing::post, Json, Router};

        for (status, unconfirmed) in [
            (axum::http::StatusCode::BAD_GATEWAY, true),
            (axum::http::StatusCode::BAD_REQUEST, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let app = Router::new().route(
                "/v2/transactions",
                post(move || async move {
                    (
                        status,
                        Json(serde_json::json!({"message": "TransactionPool.Remember: fixture"})),
                    )
                        .into_response()
                }),
            );
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let facilitator = Account::generate();
            let provider = AlgorandProvider::try_new(
                facilitator.mnemonic(),
                Some(format!("http://{addr}/")),
                Network::AlgorandTestnet,
            )
            .unwrap();
            let params = SuggestedTransactionParams {
                genesis_id: "testnet-v1.0".to_string(),
                genesis_hash: HashDigest([7u8; 32]),
                consensus_version: "future".to_string(),
                fee_per_byte: MicroAlgos(0),
                min_fee: MicroAlgos(1_000),
                first_valid: Round(1),
                last_valid: Round(500),
            };
            let fee_tx = TxnBuilder::with_fee(
                &params,
                TxnFee::Fixed(MicroAlgos(1_000)),
                Pay::new(facilitator.address(), facilitator.address(), MicroAlgos(0)).build(),
            )
            .build()
            .unwrap();
            let signed = facilitator.sign_transaction(fee_tx).unwrap();
            let first_id = signed.transaction_id.clone();
            match (provider.broadcast(&[signed]).await, unconfirmed) {
                (Err(AlgorandError::SubmissionUnconfirmed { tx_id, .. }), true) => {
                    assert_eq!(tx_id, first_id)
                }
                (Err(AlgorandError::SubmissionFailed(_)), false) => {}
                (other, _) => panic!("{status}: unexpected {other:?}"),
            }
        }
    }
}
