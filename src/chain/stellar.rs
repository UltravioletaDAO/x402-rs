//! Stellar/Soroban payment provider implementation.
//!
//! This module implements Stellar payments using Soroban smart contract
//! authorization entries. Users sign authorization entries off-chain,
//! and the facilitator wraps them in transactions and pays the fees.
//!
//! Flow:
//! 1. User creates and signs a Soroban authorization entry for USDC transfer
//! 2. User sends authorization entry XDR to facilitator (base64 encoded)
//! 3. Facilitator verifies the authorization signature
//! 4. Facilitator constructs and submits the transaction (pays fees)
//! 5. Stellar network executes the authorized transfer

use alloy::hex;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use stellar_strkey::{
    ed25519::PrivateKey as StellarPrivateKey, ed25519::PublicKey as StellarPublicKey, Contract,
};
use stellar_xdr::curr::{
    AccountId, DecoratedSignature, Hash, HashIdPreimage, HashIdPreimageSorobanAuthorization,
    HostFunction, Int128Parts, InvokeContractArgs, InvokeHostFunctionOp, Limits, Memo,
    MuxedAccount, Operation, OperationBody, Preconditions, PublicKey, ReadXdr, ScAddress, ScVal,
    SequenceNumber, SorobanAddressCredentials, SorobanAuthorizationEntry,
    SorobanAuthorizedFunction, SorobanAuthorizedInvocation, SorobanCredentials,
    SorobanTransactionData, Transaction, TransactionEnvelope, TransactionExt,
    TransactionSignaturePayload, TransactionSignaturePayloadTaggedTransaction,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::{Network, USDCDeployment};
use crate::nonce_store::{stellar_nonce_key, stellar_ttl_seconds, NonceStore, NonceStoreError};
use crate::types::{
    ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, Scheme, SettleRequest,
    SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse,
    TokenAmount, TransactionHash, VerifyRequest, VerifyResponse, X402Version,
};

// =============================================================================
// Constants
// =============================================================================

/// Network passphrase for Stellar mainnet
pub const STELLAR_MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
/// Network passphrase for Stellar testnet
pub const STELLAR_TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

// =============================================================================
// Error Types
// =============================================================================

/// Stellar-specific errors
#[derive(Debug, thiserror::Error)]
pub enum StellarError {
    #[error("Invalid XDR encoding: {0}")]
    InvalidXdr(String),

    #[error("Authorization expired at ledger {expiry_ledger} (current: {current_ledger})")]
    AuthExpired {
        expiry_ledger: u32,
        current_ledger: u32,
    },

    #[error("Invalid authorization signature for address {address}")]
    InvalidSignature { address: String },

    #[error("Nonce {nonce} already used for address {from}")]
    NonceReused { from: String, nonce: u64 },

    #[error("Simulation failed: {error}")]
    SimulationFailed { error: String },

    #[error("Transaction submission failed: {0}")]
    SubmissionFailed(String),

    #[error("Transaction not found after {attempts} attempts")]
    TransactionNotFound { attempts: u32 },

    #[error("Transaction failed with status: {status}")]
    TransactionFailed { status: String },

    #[error("Token contract mismatch: expected {expected}, got {actual}")]
    TokenContractMismatch { expected: String, actual: String },

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Missing credentials in authorization entry")]
    MissingCredentials,

    #[error("Unsupported credential type")]
    UnsupportedCredentialType,

    // B4: auth-entry content validation errors
    #[error("Authorization entry must invoke ContractFn, not CreateContractHostFn")]
    InvalidInvocationType,

    #[error(
        "Authorization entry contract mismatch: expected USDC contract {expected}, got {actual}"
    )]
    InvalidContractAddress { expected: String, actual: String },

    #[error("Authorization entry function must be 'transfer', got '{actual}'")]
    InvalidFunctionName { actual: String },

    #[error("Authorization entry args count must be 3, got {actual}")]
    InvalidArgsCount { actual: usize },

    #[error("Authorization entry sender must match payer {expected}, got {actual}")]
    InvalidSender { expected: String, actual: String },

    #[error("Authorization entry recipient must match pay_to {expected}, got {actual}")]
    InvalidRecipient { expected: String, actual: String },

    #[error("Authorization entry amount must match max_amount_required {expected}, got {actual}")]
    InvalidAmount { expected: String, actual: String },

    #[error("Authorization entry has unexpected sub-invocations (depth > 0 not permitted)")]
    UnexpectedSubInvocations,

    #[error("Authorization entry args contain unexpected ScVal types: {0}")]
    InvalidArgType(String),

    // F5: nonce store unavailable (fail-closed). Returning this rejects the
    // payment instead of allowing a potential replay through a downed store.
    #[error("Nonce store unavailable: {0}")]
    NonceStoreUnavailable(String),
}

impl From<StellarError> for FacilitatorLocalError {
    fn from(e: StellarError) -> Self {
        FacilitatorLocalError::Other(e.to_string())
    }
}

// =============================================================================
// Chain Configuration
// =============================================================================

/// Stellar network chain configuration
#[derive(Clone, Debug)]
pub struct StellarChain {
    pub network: Network,
    pub network_passphrase: String,
}

impl StellarChain {
    /// Get the Horizon API URL for this network
    pub fn horizon_url(&self) -> &'static str {
        match self.network {
            Network::Stellar => "https://horizon.stellar.org",
            Network::StellarTestnet => "https://horizon-testnet.stellar.org",
            _ => unreachable!("StellarChain only supports Stellar networks"),
        }
    }

    /// Get the default Soroban RPC URL for this network
    pub fn default_soroban_rpc_url(&self) -> &'static str {
        match self.network {
            Network::Stellar => "https://soroban-rpc.mainnet.stellar.gateway.fm",
            Network::StellarTestnet => "https://soroban-testnet.stellar.org",
            _ => unreachable!("StellarChain only supports Stellar networks"),
        }
    }

    /// Get the network ID hash (SHA256 of passphrase)
    pub fn network_id(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(self.network_passphrase.as_bytes());
        let result = hasher.finalize();
        Hash(result.into())
    }

    /// Compute the SHA256 hash a Stellar signer must sign for a classic
    /// transaction.
    ///
    /// The preimage is the canonical Stellar `TransactionSignaturePayload`
    /// XDR: `{ network_id, tagged_transaction: Tx(tx) }`. Wire-equivalent to
    /// the legacy concat `network_id || ENVELOPE_TYPE_TX || tx.to_xdr()`,
    /// but resilient to future envelope-type additions and self-documenting.
    pub fn compute_transaction_hash(&self, tx: &Transaction) -> Result<Vec<u8>, StellarError> {
        let payload = TransactionSignaturePayload {
            network_id: self.network_id(),
            tagged_transaction: TransactionSignaturePayloadTaggedTransaction::Tx(tx.clone()),
        };

        let preimage = payload.to_xdr(Limits::none()).map_err(|e| {
            StellarError::InvalidXdr(format!(
                "Failed to encode transaction signature payload: {}",
                e
            ))
        })?;

        let hash = Sha256::digest(&preimage);
        Ok(hash.to_vec())
    }
}

impl TryFrom<Network> for StellarChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Stellar => Ok(Self {
                network: value,
                network_passphrase: STELLAR_MAINNET_PASSPHRASE.to_string(),
            }),
            Network::StellarTestnet => Ok(Self {
                network: value,
                network_passphrase: STELLAR_TESTNET_PASSPHRASE.to_string(),
            }),
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

// =============================================================================
// Address Types
// =============================================================================

/// Stellar account address wrapper
#[derive(Clone, Debug)]
pub struct StellarAddress {
    /// The public key in G... format (for accounts) or C... format (for contracts)
    pub address: String,
}

impl StellarAddress {
    /// Create a new StellarAddress
    pub fn new(address: String) -> Self {
        Self { address }
    }

    /// Check if this is a valid Stellar address (G... for accounts, C... for contracts)
    pub fn is_valid(&self) -> bool {
        StellarPublicKey::from_string(&self.address).is_ok()
            || Contract::from_string(&self.address).is_ok()
    }

    /// Get the raw 32-byte public key if this is a G... address
    pub fn public_key_bytes(&self) -> Option<[u8; 32]> {
        StellarPublicKey::from_string(&self.address)
            .ok()
            .map(|pk| pk.0)
    }
}

impl TryFrom<String> for StellarAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let addr = StellarAddress::new(value);
        if addr.is_valid() {
            Ok(addr)
        } else {
            Err(FacilitatorLocalError::InvalidAddress(format!(
                "Invalid Stellar address: {}",
                addr.address
            )))
        }
    }
}

impl TryFrom<MixedAddress> for StellarAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Stellar(address) => Self::try_from(address),
            _ => Err(FacilitatorLocalError::InvalidAddress(
                "expected Stellar address".to_string(),
            )),
        }
    }
}

impl From<StellarAddress> for MixedAddress {
    fn from(value: StellarAddress) -> Self {
        MixedAddress::Stellar(value.address)
    }
}

// =============================================================================
// RPC Types
// =============================================================================

/// Soroban RPC request wrapper (with params)
#[derive(Debug, Serialize)]
struct RpcRequest<T: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: T,
}

/// Soroban RPC request wrapper (without params)
/// Some RPC methods like getLatestLedger reject empty params objects
#[derive(Debug, Serialize)]
struct RpcRequestNoParams {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
}

/// Soroban RPC response wrapper
#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: u64,
    result: Option<T>,
    error: Option<RpcError>,
}

/// Soroban RPC error
#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// getLatestLedger response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetLatestLedgerResult {
    sequence: u32,
    #[allow(dead_code)]
    id: String,
}

/// simulateTransaction params
#[derive(Debug, Serialize)]
struct SimulateTransactionParams {
    transaction: String,
}

/// simulateTransaction response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimulateTransactionResult {
    #[allow(dead_code)]
    latest_ledger: u32,
    min_resource_fee: Option<String>,
    /// The SorobanTransactionData XDR (base64) to set in Transaction.ext
    transaction_data: Option<String>,
    error: Option<String>,
    #[serde(default)]
    results: Vec<SimulateHostFunctionResult>,
}

#[derive(Debug, Deserialize)]
struct SimulateHostFunctionResult {
    #[allow(dead_code)]
    xdr: Option<String>,
    #[allow(dead_code)]
    auth: Option<Vec<String>>,
}

/// sendTransaction params
#[derive(Debug, Serialize)]
struct SendTransactionParams {
    transaction: String,
}

/// sendTransaction response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendTransactionResult {
    status: String,
    hash: String,
    #[allow(dead_code)]
    latest_ledger: u32,
    error_result_xdr: Option<String>,
}

/// getTransaction params
#[derive(Debug, Serialize)]
struct GetTransactionParams {
    hash: String,
}

/// getTransaction response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetTransactionResult {
    status: String,
    #[allow(dead_code)]
    latest_ledger: u32,
    #[allow(dead_code)]
    ledger: Option<u32>,
    result_xdr: Option<String>,
}

/// Horizon account response
#[derive(Debug, Deserialize)]
struct HorizonAccount {
    sequence: String,
}

// =============================================================================
// Provider Implementation
// =============================================================================

/// Stellar payment provider
///
/// Implements USDC payments on Stellar using Soroban smart contract
/// Global nonce store for replay protection across all Stellar providers.
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

/// authorization entries. The facilitator receives pre-signed authorization
/// entries and wraps them in transactions, paying the fees.
#[derive(Clone)]
pub struct StellarProvider {
    /// The facilitator's ed25519 signing key
    signing_key: Arc<SigningKey>,
    /// The facilitator's public key in G... format
    public_key: String,
    /// HTTP client for Horizon/Soroban RPC
    http_client: Arc<reqwest::Client>,
    /// Network configuration
    chain: StellarChain,
    /// Custom RPC URL (from environment) or None to use defaults
    rpc_url: Option<String>,
}

impl Debug for StellarProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StellarProvider")
            .field("public_key", &self.public_key)
            .field("chain", &self.chain)
            .finish()
    }
}

impl StellarProvider {
    /// Create a new Stellar provider
    pub fn try_new(
        secret_key: String,
        rpc_url: Option<String>,
        network: Network,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = StellarChain::try_from(network)?;

        // Decode secret key and derive signing key
        let stellar_private_key = StellarPrivateKey::from_string(&secret_key).map_err(|e| {
            FacilitatorLocalError::InvalidAddress(format!("Invalid Stellar secret key: {}", e))
        })?;

        let signing_key = SigningKey::from_bytes(&stellar_private_key.0);
        let verifying_key = signing_key.verifying_key();
        let stellar_public_key = StellarPublicKey(verifying_key.to_bytes());
        let public_key = stellar_public_key.to_string();

        tracing::info!(
            network = %network,
            public_key = %public_key,
            rpc_url = %crate::redact::rpc_url(rpc_url.as_deref().unwrap_or(chain.default_soroban_rpc_url())),
            "Initialized Stellar provider"
        );

        Ok(Self {
            signing_key: Arc::new(signing_key),
            public_key,
            http_client: Arc::new(reqwest::Client::new()),
            chain,
            rpc_url,
        })
    }

    /// Get the facilitator's public key as a MixedAddress
    pub fn facilitator_address(&self) -> MixedAddress {
        MixedAddress::Stellar(self.public_key.clone())
    }

    /// Get the effective RPC URL (custom or default)
    pub fn effective_rpc_url(&self) -> &str {
        self.rpc_url
            .as_deref()
            .unwrap_or_else(|| self.chain.default_soroban_rpc_url())
    }

    /// Make an RPC request to Soroban RPC
    async fn rpc_request<P: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, StellarError> {
        let request = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };

        let response = self
            .http_client
            .post(self.effective_rpc_url())
            .json(&request)
            .send()
            .await
            .map_err(|e| StellarError::RpcError(e.to_string()))?;

        let rpc_response: RpcResponse<R> = response
            .json()
            .await
            .map_err(|e| StellarError::RpcError(e.to_string()))?;

        if let Some(error) = rpc_response.error {
            return Err(StellarError::RpcError(format!(
                "RPC error {}: {}",
                error.code, error.message
            )));
        }

        rpc_response
            .result
            .ok_or_else(|| StellarError::RpcError("Empty response".to_string()))
    }

    /// Make an RPC request without params to Soroban RPC
    /// Some methods like getLatestLedger reject empty params objects
    async fn rpc_request_no_params<R: for<'de> Deserialize<'de>>(
        &self,
        method: &'static str,
    ) -> Result<R, StellarError> {
        let request = RpcRequestNoParams {
            jsonrpc: "2.0",
            id: 1,
            method,
        };

        let response = self
            .http_client
            .post(self.effective_rpc_url())
            .json(&request)
            .send()
            .await
            .map_err(|e| StellarError::RpcError(e.to_string()))?;

        let rpc_response: RpcResponse<R> = response
            .json()
            .await
            .map_err(|e| StellarError::RpcError(e.to_string()))?;

        if let Some(error) = rpc_response.error {
            return Err(StellarError::RpcError(format!(
                "RPC error {}: {}",
                error.code, error.message
            )));
        }

        rpc_response
            .result
            .ok_or_else(|| StellarError::RpcError("Empty response".to_string()))
    }

    /// Get the current ledger sequence number
    async fn get_latest_ledger(&self) -> Result<u32, StellarError> {
        let result: GetLatestLedgerResult = self.rpc_request_no_params("getLatestLedger").await?;
        Ok(result.sequence)
    }

    /// Get the account sequence number from Horizon
    async fn get_account_sequence(&self, account_id: &str) -> Result<i64, StellarError> {
        let url = format!("{}/accounts/{}", self.chain.horizon_url(), account_id);
        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| StellarError::RpcError(format!("Horizon request failed: {}", e)))?;

        let account: HorizonAccount = response.json().await.map_err(|e| {
            StellarError::RpcError(format!("Failed to parse Horizon response: {}", e))
        })?;

        account
            .sequence
            .parse()
            .map_err(|e| StellarError::RpcError(format!("Invalid sequence number: {}", e)))
    }

    /// Decode and validate the authorization entry XDR
    fn decode_authorization_entry(
        &self,
        xdr_base64: &str,
    ) -> Result<SorobanAuthorizationEntry, StellarError> {
        let xdr_bytes = BASE64
            .decode(xdr_base64)
            .map_err(|e| StellarError::InvalidXdr(format!("Base64 decode failed: {}", e)))?;

        SorobanAuthorizationEntry::from_xdr(&xdr_bytes, Limits::none())
            .map_err(|e| StellarError::InvalidXdr(format!("XDR decode failed: {}", e)))
    }

    /// Verify the signature on an authorization entry
    fn verify_authorization_signature(
        &self,
        auth_entry: &SorobanAuthorizationEntry,
        expected_address: &str,
    ) -> Result<(), StellarError> {
        // Extract credentials from the authorization entry
        let credentials = match &auth_entry.credentials {
            SorobanCredentials::Address(addr_creds) => addr_creds,
            SorobanCredentials::SourceAccount => {
                // SECURITY (audit 01): SourceAccount credentials carry NO payer signature.
                // On the payment path the tx source account is the facilitator, so accepting
                // SourceAccount would let the facilitator's own signature authorize the
                // transfer's `from` -- a self-drain. The x402 Stellar spec mandates
                // SorobanAddressCredentials (payer-signed). Reject hard.
                tracing::warn!(
                    "Authorization entry uses SourceAccount credentials on payment path - rejecting"
                );
                return Err(StellarError::UnsupportedCredentialType);
            }
        };

        // SECURITY (audit 01): the credential address (the account whose signature
        // authorizes this transfer) MUST be the declared payer, not some other account.
        let cred_addr_str = match &credentials.address {
            ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(b)))) => {
                StellarPublicKey(*b).to_string()
            }
            other => {
                tracing::warn!(
                    ?other,
                    "auth credential address is not an ed25519 account - rejecting"
                );
                return Err(StellarError::InvalidSignature {
                    address: expected_address.to_string(),
                });
            }
        };
        if cred_addr_str != expected_address {
            tracing::warn!(
                credential_address = %cred_addr_str,
                expected = %expected_address,
                "auth credential address does not match declared payer - rejecting"
            );
            return Err(StellarError::InvalidSignature {
                address: expected_address.to_string(),
            });
        }

        // Get the signature from credentials
        // Stellar supports two signature formats:
        // 1. ScVal::Bytes - single ed25519 signature (64 bytes)
        // 2. ScVal::Vec - multi-sig with AccountEd25519Signature entries
        let signature_bytes = match &credentials.signature {
            stellar_xdr::curr::ScVal::Bytes(bytes) => bytes.as_slice(),
            stellar_xdr::curr::ScVal::Vec(Some(vec)) if !vec.is_empty() => {
                // Multi-sig format: Vec<AccountEd25519Signature>
                // Each entry is a Map with "public_key" (32 bytes) and "signature" (64 bytes)
                // We need to find a signature from the expected address and verify it
                tracing::debug!(
                    "Authorization uses Vec signature format (multi-sig), {} entries",
                    vec.len()
                );
                return self.verify_multisig_authorization(vec, expected_address, auth_entry);
            }
            stellar_xdr::curr::ScVal::Vec(None) | stellar_xdr::curr::ScVal::Vec(Some(_)) => {
                // Empty Vec is invalid
                tracing::warn!("Empty Vec signature format - rejecting");
                return Err(StellarError::InvalidSignature {
                    address: expected_address.to_string(),
                });
            }
            other => {
                // SECURITY: Reject unknown signature formats
                tracing::warn!(
                    "Unexpected signature format in authorization entry: {:?} - rejecting",
                    std::mem::discriminant(other)
                );
                return Err(StellarError::InvalidSignature {
                    address: expected_address.to_string(),
                });
            }
        };

        if signature_bytes.len() != 64 {
            return Err(StellarError::InvalidSignature {
                address: expected_address.to_string(),
            });
        }

        // Get the public key from the expected address
        let public_key_bytes = StellarPublicKey::from_string(expected_address)
            .map_err(|_| StellarError::InvalidSignature {
                address: expected_address.to_string(),
            })?
            .0;

        let verifying_key = VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| {
            StellarError::InvalidSignature {
                address: expected_address.to_string(),
            }
        })?;

        // Compute the signature preimage using credentials and invocation
        let preimage =
            self.compute_auth_entry_preimage(credentials, &auth_entry.root_invocation)?;

        // Verify the signature
        let signature =
            Signature::from_slice(signature_bytes).map_err(|_| StellarError::InvalidSignature {
                address: expected_address.to_string(),
            })?;

        verifying_key
            .verify(&preimage, &signature)
            .map_err(|_| StellarError::InvalidSignature {
                address: expected_address.to_string(),
            })
    }

    /// Verify multi-sig authorization (Vec<AccountEd25519Signature> format)
    ///
    /// In Stellar multi-sig, the signature is a Vec of AccountEd25519Signature entries.
    /// Each entry contains a public_key (32 bytes) and signature (64 bytes).
    /// We need to find an entry matching the expected address and verify it.
    fn verify_multisig_authorization(
        &self,
        signatures: &stellar_xdr::curr::VecM<stellar_xdr::curr::ScVal>,
        expected_address: &str,
        auth_entry: &SorobanAuthorizationEntry,
    ) -> Result<(), StellarError> {
        use stellar_xdr::curr::ScVal;

        // Extract credentials to get nonce and expiration for preimage
        let credentials = match &auth_entry.credentials {
            SorobanCredentials::Address(addr_creds) => addr_creds,
            SorobanCredentials::SourceAccount => {
                return Err(StellarError::InvalidSignature {
                    address: expected_address.to_string(),
                });
            }
        };

        // Get the expected public key bytes from the address
        let expected_pubkey = StellarPublicKey::from_string(expected_address)
            .map_err(|_| StellarError::InvalidSignature {
                address: expected_address.to_string(),
            })?
            .0;

        // Compute the preimage hash for signature verification
        let preimage =
            self.compute_auth_entry_preimage(credentials, &auth_entry.root_invocation)?;

        // Search through all signature entries for one matching our expected address
        for (idx, entry) in signatures.iter().enumerate() {
            // Each entry should be a Map with "public_key" and "signature" keys
            let map = match entry {
                ScVal::Map(Some(map)) => map,
                _ => {
                    tracing::debug!("Signature entry {} is not a Map, skipping", idx);
                    continue;
                }
            };

            let mut found_pubkey: Option<[u8; 32]> = None;
            let mut found_sig: Option<[u8; 64]> = None;

            // Extract public_key and signature from the map
            for pair in map.iter() {
                let key_name = match &pair.key {
                    ScVal::Symbol(sym) => sym.to_string(),
                    _ => continue,
                };

                match key_name.as_str() {
                    "public_key" => {
                        if let ScVal::Bytes(bytes) = &pair.val {
                            if bytes.len() == 32 {
                                let mut arr = [0u8; 32];
                                arr.copy_from_slice(bytes.as_slice());
                                found_pubkey = Some(arr);
                            }
                        }
                    }
                    "signature" => {
                        if let ScVal::Bytes(bytes) = &pair.val {
                            if bytes.len() == 64 {
                                let mut arr = [0u8; 64];
                                arr.copy_from_slice(bytes.as_slice());
                                found_sig = Some(arr);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Check if this entry matches our expected address
            if let (Some(pubkey), Some(sig)) = (found_pubkey, found_sig) {
                if pubkey == expected_pubkey {
                    tracing::debug!(
                        "Found matching signature entry for address {}",
                        expected_address
                    );

                    // Verify the ed25519 signature
                    let verifying_key = VerifyingKey::from_bytes(&pubkey).map_err(|_| {
                        StellarError::InvalidSignature {
                            address: expected_address.to_string(),
                        }
                    })?;

                    let signature = Signature::from_slice(&sig).map_err(|_| {
                        StellarError::InvalidSignature {
                            address: expected_address.to_string(),
                        }
                    })?;

                    return verifying_key.verify(&preimage, &signature).map_err(|_| {
                        StellarError::InvalidSignature {
                            address: expected_address.to_string(),
                        }
                    });
                }
            }
        }

        // No matching signature found for the expected address
        tracing::warn!(
            "No valid signature found for address {} in {} multi-sig entries",
            expected_address,
            signatures.len()
        );
        Err(StellarError::InvalidSignature {
            address: expected_address.to_string(),
        })
    }

    /// Compute the preimage for authorization entry signing
    ///
    /// The preimage is SHA256 of the XDR-encoded HashIdPreimageSorobanAuthorization,
    /// which contains:
    /// - network_id: The network passphrase hash
    /// - nonce: The authorization nonce
    /// - signature_expiration_ledger: When the signature expires
    /// - invocation: The authorized function invocation (without the signature)
    ///
    /// IMPORTANT: This does NOT include the signature itself - you cannot verify
    /// a signature against data containing that signature.
    fn compute_auth_entry_preimage(
        &self,
        credentials: &SorobanAddressCredentials,
        invocation: &SorobanAuthorizedInvocation,
    ) -> Result<Vec<u8>, StellarError> {
        // Build the HashIdPreimageSorobanAuthorization structure
        // This is what the Stellar SDK signs when creating authorization signatures
        let preimage_data = HashIdPreimageSorobanAuthorization {
            network_id: self.chain.network_id(),
            nonce: credentials.nonce,
            signature_expiration_ledger: credentials.signature_expiration_ledger,
            invocation: invocation.clone(),
        };

        // Wrap in HashIdPreimage enum for proper XDR encoding
        let preimage = HashIdPreimage::SorobanAuthorization(preimage_data);

        // Encode to XDR
        let preimage_xdr = preimage
            .to_xdr(Limits::none())
            .map_err(|e| StellarError::InvalidXdr(format!("Failed to encode preimage: {}", e)))?;

        // Hash the XDR-encoded preimage
        let hash = Sha256::digest(&preimage_xdr);
        Ok(hash.to_vec())
    }

    /// Get the chain name for nonce store keys
    fn chain_name(&self) -> &'static str {
        match self.chain.network {
            Network::Stellar => "stellar",
            Network::StellarTestnet => "stellar-testnet",
            _ => "stellar-unknown",
        }
    }

    /// Check if a nonce has been used (read-only, for verification)
    async fn check_nonce_unused(&self, from: &str, nonce: u64) -> Result<(), StellarError> {
        let store = get_global_nonce_store().await;
        let key = stellar_nonce_key(self.chain_name(), from, nonce);

        match store.is_used(&key).await {
            Ok(true) => Err(StellarError::NonceReused {
                from: from.to_string(),
                nonce,
            }),
            Ok(false) => Ok(()),
            Err(e) => {
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(
                    %correlation_id,
                    error = %e,
                    from = %from,
                    nonce = nonce,
                    "Nonce store read failed during verify, failing closed"
                );
                Err(StellarError::NonceStoreUnavailable(format!(
                    "verification_unavailable (ref: {correlation_id})"
                )))
            }
        }
    }

    /// Atomically check and mark a nonce as used (for settlement)
    /// Returns error if nonce was already used (replay attempt)
    async fn check_and_mark_nonce_used(
        &self,
        from: &str,
        nonce: u64,
        current_ledger: u32,
        expiry_ledger: u32,
    ) -> Result<(), StellarError> {
        let store = get_global_nonce_store().await;
        let key = stellar_nonce_key(self.chain_name(), from, nonce);
        let ttl = stellar_ttl_seconds(current_ledger, expiry_ledger);

        match store.check_and_mark_used(&key, ttl).await {
            Ok(()) => Ok(()),
            Err(NonceStoreError::NonceAlreadyUsed(_)) => Err(StellarError::NonceReused {
                from: from.to_string(),
                nonce,
            }),
            Err(e) => {
                // F5: fail-closed. Connection/read/write errors must reject the
                // settlement; failing open here is a replay vector when the
                // store goes down.
                let correlation_id = uuid::Uuid::new_v4();
                tracing::error!(
                    %correlation_id,
                    error = %e,
                    from = %from,
                    nonce = nonce,
                    "Nonce store write failed during settle, failing closed"
                );
                Err(StellarError::NonceStoreUnavailable(format!(
                    "settlement_unavailable (ref: {correlation_id})"
                )))
            }
        }
    }

    /// Validate the semantic content of a Soroban authorization entry (B4 fix).
    ///
    /// Checks that the entry authorizes exactly the expected USDC transfer:
    /// - root_invocation.function is ContractFn (not CreateContractHostFn)
    /// - contract_address matches the known USDC contract for this network
    /// - function_name is "transfer"
    /// - args has exactly 3 elements: [from: payer, to: pay_to, amount: max_amount_required]
    /// - sub_invocations is empty (no nested calls)
    ///
    /// This must be called BEFORE signing the authorization entry preimage to prevent
    /// a malicious client from getting the facilitator to authorize arbitrary Soroban calls.
    fn validate_soroban_auth_entry(
        &self,
        expected_from: &str, // SECURITY (audit 01): the PAYER (stellar_payload.from)
        auth_entry: &SorobanAuthorizationEntry,
        expected_pay_to: &str,
        expected_amount: TokenAmount,
    ) -> Result<(), StellarError> {
        // --- Check 1: must be ContractFn, not CreateContractHostFn ---
        let invoke_args: &InvokeContractArgs = match &auth_entry.root_invocation.function {
            SorobanAuthorizedFunction::ContractFn(args) => args,
            SorobanAuthorizedFunction::CreateContractHostFn(_) => {
                tracing::warn!(
                    network = %self.chain.network,
                    "B4: auth entry uses CreateContractHostFn, rejecting"
                );
                return Err(StellarError::InvalidInvocationType);
            }
        };

        // --- Check 2: contract address must be the known USDC contract ---
        let expected_usdc = USDCDeployment::by_network(self.chain.network).ok_or_else(|| {
            StellarError::TokenContractMismatch {
                expected: "unknown (no USDC deployment for network)".to_string(),
                actual: format!("{:?}", invoke_args.contract_address),
            }
        })?;
        let expected_contract_str = match &expected_usdc.0.asset.address {
            MixedAddress::Stellar(s) => s.clone(),
            other => {
                return Err(StellarError::TokenContractMismatch {
                    expected: format!("{:?}", other),
                    actual: format!("{:?}", invoke_args.contract_address),
                });
            }
        };
        // Convert the expected C... strkey to raw 32-byte hash for comparison with ScAddress::Contract
        let expected_contract_bytes = Contract::from_string(&expected_contract_str)
            .map_err(|e| {
                StellarError::InvalidXdr(format!(
                    "Could not parse known USDC contract address '{}': {}",
                    expected_contract_str, e
                ))
            })?
            .0;
        let actual_contract_bytes: [u8; 32] = match &invoke_args.contract_address {
            ScAddress::Contract(Hash(bytes)) => *bytes,
            ScAddress::Account(_) => {
                tracing::warn!(
                    network = %self.chain.network,
                    expected = %expected_contract_str,
                    "B4: auth entry contract_address is an Account address, expected Contract"
                );
                return Err(StellarError::InvalidContractAddress {
                    expected: expected_contract_str,
                    actual: "Account(...)".to_string(),
                });
            }
        };
        if actual_contract_bytes != expected_contract_bytes {
            // Re-encode actual bytes as a C... strkey for the error message
            let actual_str = Contract(actual_contract_bytes).to_string();
            tracing::warn!(
                network = %self.chain.network,
                expected = %expected_contract_str,
                actual = %actual_str,
                "B4: auth entry contract address does not match USDC contract"
            );
            return Err(StellarError::InvalidContractAddress {
                expected: expected_contract_str,
                actual: actual_str,
            });
        }

        // --- Check 3: function name must be "transfer" ---
        // ScSymbol wraps StringM<32> which holds raw bytes; decode as UTF-8 for comparison.
        let fn_name = std::str::from_utf8(invoke_args.function_name.as_slice())
            .map_err(|e| StellarError::InvalidXdr(format!("Non-UTF-8 function name: {}", e)))?
            .to_string();
        if fn_name != "transfer" {
            tracing::warn!(
                network = %self.chain.network,
                actual_fn = %fn_name,
                "B4: auth entry function name is not 'transfer'"
            );
            return Err(StellarError::InvalidFunctionName { actual: fn_name });
        }

        // --- Check 4: exactly 3 args ---
        if invoke_args.args.len() != 3 {
            tracing::warn!(
                network = %self.chain.network,
                args_count = invoke_args.args.len(),
                "B4: auth entry does not have exactly 3 args"
            );
            return Err(StellarError::InvalidArgsCount {
                actual: invoke_args.args.len(),
            });
        }

        // --- Check 5a: args[0] (transfer `from`) must be the PAYER, never the facilitator ---
        // SECURITY (audit 01): previously this required args[0] == self.public_key
        // (the facilitator), which made every accepted transfer drain the facilitator's
        // own USDC. Bind it to the declared payer and hard-reject the facilitator.
        let expected_from_bytes = StellarPublicKey::from_string(expected_from)
            .map_err(|e| {
                StellarError::InvalidXdr(format!(
                    "Could not parse payer public key '{}': {}",
                    expected_from, e
                ))
            })?
            .0;
        let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)
            .map_err(|e| {
                StellarError::InvalidXdr(format!(
                    "Could not parse facilitator public key '{}': {}",
                    self.public_key, e
                ))
            })?
            .0;
        match &invoke_args.args[0] {
            ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(key_bytes),
            )))) => {
                // Never allow the facilitator to be the source of funds.
                if *key_bytes == facilitator_bytes {
                    tracing::warn!(
                        network = %self.chain.network,
                        "B4/audit01: auth entry `from` is the facilitator - rejecting self-drain"
                    );
                    return Err(StellarError::InvalidSender {
                        expected: expected_from.to_string(),
                        actual: self.public_key.clone(),
                    });
                }
                if *key_bytes != expected_from_bytes {
                    let actual_pk = StellarPublicKey(*key_bytes).to_string();
                    tracing::warn!(
                        network = %self.chain.network,
                        expected_sender = %expected_from,
                        actual_sender = %actual_pk,
                        "B4/audit01: auth entry `from` does not match declared payer"
                    );
                    return Err(StellarError::InvalidSender {
                        expected: expected_from.to_string(),
                        actual: actual_pk,
                    });
                }
            }
            ScVal::Address(ScAddress::Contract(Hash(bytes))) => {
                let actual_str = Contract(*bytes).to_string();
                tracing::warn!(
                    network = %self.chain.network,
                    expected_sender = %expected_from,
                    actual_sender = %actual_str,
                    "B4/audit01: auth entry `from` is a contract address, expected payer account"
                );
                return Err(StellarError::InvalidSender {
                    expected: expected_from.to_string(),
                    actual: actual_str,
                });
            }
            other => {
                tracing::warn!(
                    network = %self.chain.network,
                    "B4/audit01: auth entry args[0] has unexpected ScVal type"
                );
                return Err(StellarError::InvalidArgType(format!(
                    "args[0] must be ScVal::Address(Account), got {:?}",
                    std::mem::discriminant(other)
                )));
            }
        }

        // --- Check 5b: args[1] = ScVal::Address matching pay_to ---
        let actual_recipient_str: String = match &invoke_args.args[1] {
            ScVal::Address(ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(key_bytes),
            )))) => StellarPublicKey(*key_bytes).to_string(),
            ScVal::Address(ScAddress::Contract(Hash(bytes))) => {
                // Contracts can legitimately receive tokens; encode as C... strkey
                Contract(*bytes).to_string()
            }
            other => {
                tracing::warn!(
                    network = %self.chain.network,
                    "B4: auth entry args[1] has unexpected ScVal type"
                );
                return Err(StellarError::InvalidArgType(format!(
                    "args[1] must be ScVal::Address, got {:?}",
                    std::mem::discriminant(other)
                )));
            }
        };
        if actual_recipient_str != expected_pay_to {
            tracing::warn!(
                network = %self.chain.network,
                expected_recipient = %expected_pay_to,
                actual_recipient = %actual_recipient_str,
                "B4: auth entry recipient does not match pay_to"
            );
            return Err(StellarError::InvalidRecipient {
                expected: expected_pay_to.to_string(),
                actual: actual_recipient_str,
            });
        }

        // --- Check 5c: args[2] = ScVal::I128 matching expected_amount ---
        // Stellar USDC uses 7 decimals; max_amount_required is already in those base units.
        // ScVal::I128(Int128Parts { hi, lo }) represents: (hi as i128) << 64 | (lo as u64 as i128)
        let actual_amount_i128: i128 = match &invoke_args.args[2] {
            ScVal::I128(Int128Parts { hi, lo }) => {
                let hi = *hi as i128;
                let lo = *lo as u64 as i128; // lo is u64 in XDR, treat unsigned
                hi.wrapping_shl(64).wrapping_add(lo)
            }
            other => {
                tracing::warn!(
                    network = %self.chain.network,
                    "B4: auth entry args[2] has unexpected ScVal type (expected I128)"
                );
                return Err(StellarError::InvalidArgType(format!(
                    "args[2] must be ScVal::I128, got {:?}",
                    std::mem::discriminant(other)
                )));
            }
        };
        // Amounts must be positive (negative transfers make no sense)
        if actual_amount_i128 < 0 {
            tracing::warn!(
                network = %self.chain.network,
                actual_amount = actual_amount_i128,
                "B4: auth entry amount is negative"
            );
            return Err(StellarError::InvalidAmount {
                expected: expected_amount.to_string(),
                actual: actual_amount_i128.to_string(),
            });
        }
        let actual_amount_u128 = actual_amount_i128 as u128;
        // expected_amount is a TokenAmount(U256); for Stellar USDC the amount fits in u128
        let expected_raw: u128 = {
            let u256 = expected_amount.0;
            // Reject if it doesn't fit in u128 (would be astronomically large for any realistic transfer)
            if u256 > alloy::primitives::U256::from(u128::MAX) {
                return Err(StellarError::InvalidAmount {
                    expected: expected_amount.to_string(),
                    actual: actual_amount_i128.to_string(),
                });
            }
            u256.to::<u128>()
        };
        if actual_amount_u128 != expected_raw {
            tracing::warn!(
                network = %self.chain.network,
                expected_amount = expected_raw,
                actual_amount = actual_amount_u128,
                "B4: auth entry amount does not match max_amount_required"
            );
            return Err(StellarError::InvalidAmount {
                expected: expected_amount.to_string(),
                actual: actual_amount_u128.to_string(),
            });
        }

        // --- Check 6: no sub-invocations (defense in depth) ---
        // The Soroban USDC token contract's transfer function does not sub-invoke
        // other contracts. Rejecting sub-invocations prevents nested reentrancy attacks
        // and ensures we know exactly what operation is being authorized.
        if !auth_entry.root_invocation.sub_invocations.is_empty() {
            tracing::warn!(
                network = %self.chain.network,
                sub_invocations_count = auth_entry.root_invocation.sub_invocations.len(),
                "B4: auth entry has unexpected sub-invocations"
            );
            return Err(StellarError::UnexpectedSubInvocations);
        }

        tracing::debug!(
            network = %self.chain.network,
            contract = %expected_contract_str,
            recipient = %actual_recipient_str,
            amount = actual_amount_u128,
            "B4: auth entry content validated successfully"
        );
        Ok(())
    }

    /// Verify a payment request
    async fn verify_payment(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyPaymentResult, FacilitatorLocalError> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;

        // Extract Stellar payload
        let stellar_payload = match &payload.payload {
            ExactPaymentPayload::Stellar(p) => p,
            _ => return Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        };

        // Verify network matches
        if payload.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                payload.network,
            ));
        }

        if requirements.network != self.network() {
            return Err(FacilitatorLocalError::NetworkMismatch(
                None,
                self.network(),
                requirements.network,
            ));
        }

        // Verify scheme matches
        if payload.scheme != requirements.scheme {
            return Err(FacilitatorLocalError::SchemeMismatch(
                None,
                requirements.scheme,
                payload.scheme,
            ));
        }

        // Validate payer address
        let payer = StellarAddress::try_from(stellar_payload.from.clone())?;

        // SECURITY (audit 01): the facilitator must never be a payer/source of funds.
        if payer.address == self.public_key {
            return Err(StellarError::InvalidSender {
                expected: "any payer != facilitator".to_string(),
                actual: self.public_key.clone(),
            }
            .into());
        }

        // Decode the authorization entry XDR
        let auth_entry = self
            .decode_authorization_entry(&stellar_payload.authorization_entry_xdr)
            .map_err(FacilitatorLocalError::from)?;

        // B4: Validate that the auth entry authorizes exactly the expected USDC transfer
        // before doing anything else with it (especially before signing).
        let pay_to_str = match &requirements.pay_to {
            MixedAddress::Stellar(s) => s.as_str(),
            other => {
                return Err(FacilitatorLocalError::InvalidAddress(format!(
                    "pay_to is not a Stellar address: {:?}",
                    other
                )));
            }
        };
        self.validate_soroban_auth_entry(
            &payer.address, // expected_from = payer (SECURITY audit 01)
            &auth_entry,
            pay_to_str,
            requirements.max_amount_required,
        )
        .map_err(FacilitatorLocalError::from)?;

        // Get current ledger for expiration check
        let current_ledger = self
            .get_latest_ledger()
            .await
            .map_err(FacilitatorLocalError::from)?;

        // Check expiration
        if stellar_payload.signature_expiration_ledger <= current_ledger {
            return Err(StellarError::AuthExpired {
                expiry_ledger: stellar_payload.signature_expiration_ledger,
                current_ledger,
            }
            .into());
        }

        // Check nonce hasn't been used
        self.check_nonce_unused(&stellar_payload.from, stellar_payload.nonce)
            .await
            .map_err(FacilitatorLocalError::from)?;

        // Verify the signature
        self.verify_authorization_signature(&auth_entry, &stellar_payload.from)
            .map_err(FacilitatorLocalError::from)?;

        tracing::debug!(
            payer = %payer.address,
            to = %stellar_payload.to,
            amount = %stellar_payload.amount,
            token_contract = %stellar_payload.token_contract,
            nonce = stellar_payload.nonce,
            expiry_ledger = stellar_payload.signature_expiration_ledger,
            current_ledger = current_ledger,
            "Verified Stellar payment authorization"
        );

        Ok(VerifyPaymentResult {
            payer,
            auth_entry,
            to: stellar_payload.to.clone(),
            amount: stellar_payload.amount.clone(),
            token_contract: stellar_payload.token_contract.clone(),
            nonce: stellar_payload.nonce,
            expiry_ledger: stellar_payload.signature_expiration_ledger,
        })
    }

    /// Build an unsigned transaction for simulation (no signature, TransactionExt::V0)
    ///
    /// CRITICAL: We extract the invocation directly from the client's auth entry
    /// to ensure the transaction matches exactly what the client signed.
    fn build_unsigned_transaction(
        &self,
        verification: &VerifyPaymentResult,
        sequence: i64,
        fee: u32,
    ) -> Result<(Transaction, [u8; 32]), StellarError> {
        tracing::debug!(
            from = %verification.payer.address,
            to = %verification.to,
            amount = %verification.amount,
            token_contract = %verification.token_contract,
            sequence = sequence,
            fee = fee,
            "Building unsigned Stellar transaction using client's auth entry invocation"
        );

        // 1. Extract InvokeContractArgs from client's auth entry
        // This is CRITICAL: we must use the exact invocation the client signed,
        // not reconstruct it, otherwise the signature will be invalid.
        let invoke_args = match &verification.auth_entry.root_invocation.function {
            SorobanAuthorizedFunction::ContractFn(args) => {
                tracing::debug!(
                    contract = ?args.contract_address,
                    function = ?args.function_name,
                    args_count = args.args.len(),
                    "Extracted invocation from client's auth entry"
                );
                args.clone()
            }
            SorobanAuthorizedFunction::CreateContractHostFn(_) => {
                return Err(StellarError::InvalidXdr(
                    "CreateContractHostFn not supported for payments".to_string(),
                ));
            }
        };

        // 2. Build InvokeHostFunctionOp with client's auth entry
        let auth: VecM<SorobanAuthorizationEntry> = vec![verification.auth_entry.clone()]
            .try_into()
            .map_err(|_| StellarError::InvalidXdr("Failed to create auth vector".to_string()))?;

        let invoke_op = InvokeHostFunctionOp {
            host_function: HostFunction::InvokeContract(invoke_args),
            auth,
        };

        // 3. Build Operation
        let operation = Operation {
            source_account: None, // Use transaction source account
            body: OperationBody::InvokeHostFunction(invoke_op),
        };

        let operations: VecM<Operation, 100> = vec![operation].try_into().map_err(|_| {
            StellarError::InvalidXdr("Failed to create operations vector".to_string())
        })?;

        // 4. Build MuxedAccount from facilitator's public key
        let facilitator_bytes = StellarPublicKey::from_string(&self.public_key)
            .map_err(|e| StellarError::InvalidXdr(format!("Invalid facilitator key: {}", e)))?
            .0;
        let source_account = MuxedAccount::Ed25519(Uint256(facilitator_bytes));

        // 5. Build Transaction with V0 ext (for simulation)
        let transaction = Transaction {
            source_account,
            fee,
            seq_num: SequenceNumber(sequence),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        };

        Ok((transaction, facilitator_bytes))
    }

    /// Build a signed transaction envelope with SorobanTransactionData from simulation
    fn build_signed_envelope(
        &self,
        verification: &VerifyPaymentResult,
        sequence: i64,
        fee: u32,
        soroban_data: SorobanTransactionData,
    ) -> Result<String, StellarError> {
        tracing::debug!(
            from = %verification.payer.address,
            to = %verification.to,
            amount = %verification.amount,
            sequence = sequence,
            fee = fee,
            "Building signed Stellar transaction envelope with Soroban data"
        );

        // Build the transaction structure (same as unsigned but with V1 ext)
        let (mut transaction, facilitator_bytes) =
            self.build_unsigned_transaction(verification, sequence, fee)?;

        // Update ext with SorobanTransactionData from simulation
        transaction.ext = TransactionExt::V1(soroban_data);
        // Update fee (already includes resource fee)
        transaction.fee = fee;

        // Sign the transaction
        let tx_hash = self.chain.compute_transaction_hash(&transaction)?;
        let signature_bytes = self.signing_key.sign(&tx_hash).to_bytes();

        // Create signature hint (last 4 bytes of public key)
        let hint_bytes: [u8; 4] = facilitator_bytes[28..32]
            .try_into()
            .map_err(|_| StellarError::InvalidXdr("Failed to create signature hint".to_string()))?;

        let decorated_sig = DecoratedSignature {
            hint: stellar_xdr::curr::SignatureHint(hint_bytes),
            signature: stellar_xdr::curr::Signature(
                signature_bytes.to_vec().try_into().map_err(|_| {
                    StellarError::InvalidXdr("Invalid signature length".to_string())
                })?,
            ),
        };

        let signatures: VecM<DecoratedSignature, 20> =
            vec![decorated_sig].try_into().map_err(|_| {
                StellarError::InvalidXdr("Failed to create signatures vector".to_string())
            })?;

        // Build TransactionV1Envelope
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: transaction,
            signatures,
        });

        // Encode to XDR and base64
        let envelope_xdr = envelope
            .to_xdr(Limits::none())
            .map_err(|e| StellarError::InvalidXdr(format!("Failed to encode envelope: {}", e)))?;

        let envelope_base64 = BASE64.encode(&envelope_xdr);

        tracing::debug!(
            envelope_len = envelope_xdr.len(),
            envelope_base64_len = envelope_base64.len(),
            "Built signed transaction envelope successfully"
        );

        Ok(envelope_base64)
    }

    /// Build unsigned envelope for simulation (no signature needed)
    fn build_simulation_envelope(
        &self,
        verification: &VerifyPaymentResult,
        sequence: i64,
        fee: u32,
    ) -> Result<String, StellarError> {
        let (transaction, _) = self.build_unsigned_transaction(verification, sequence, fee)?;

        // For simulation, we don't need signatures
        let signatures: VecM<DecoratedSignature, 20> = vec![].try_into().map_err(|_| {
            StellarError::InvalidXdr("Failed to create empty signatures".to_string())
        })?;

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: transaction,
            signatures,
        });

        let envelope_xdr = envelope
            .to_xdr(Limits::none())
            .map_err(|e| StellarError::InvalidXdr(format!("Failed to encode envelope: {}", e)))?;

        Ok(BASE64.encode(&envelope_xdr))
    }

    /// Submit a Stellar transaction with the authorization entry
    async fn submit_transaction(
        &self,
        verification: &VerifyPaymentResult,
    ) -> Result<[u8; 32], FacilitatorLocalError> {
        // Atomically check and mark nonce as used BEFORE submitting to blockchain
        // This prevents concurrent replay attempts
        let current_ledger = self
            .get_latest_ledger()
            .await
            .map_err(FacilitatorLocalError::from)?;
        self.check_and_mark_nonce_used(
            &verification.payer.address,
            verification.nonce,
            current_ledger,
            verification.expiry_ledger,
        )
        .await
        .map_err(FacilitatorLocalError::from)?;

        tracing::info!("submit_transaction: Getting facilitator account sequence");
        // Get facilitator's account sequence number
        let account_sequence = self
            .get_account_sequence(&self.public_key)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Failed to get account sequence");
                FacilitatorLocalError::from(e)
            })?;

        // Increment sequence for next transaction
        let next_sequence = account_sequence
            .checked_add(1)
            .ok_or_else(|| FacilitatorLocalError::Other("Sequence number overflow".to_string()))?;

        tracing::info!(
            account_sequence = account_sequence,
            next_sequence = next_sequence,
            "submit_transaction: Got account sequence"
        );

        // Base fee for simulation (will be updated with resource fee after)
        let base_fee = 100u32; // 100 stroops base

        // Step 1: Build unsigned envelope for simulation
        tracing::info!("submit_transaction: Building simulation envelope");
        let sim_envelope = self
            .build_simulation_envelope(verification, next_sequence, base_fee)
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Failed to build simulation envelope");
                FacilitatorLocalError::from(e)
            })?;

        tracing::info!(
            envelope_len = sim_envelope.len(),
            "submit_transaction: Built simulation envelope"
        );

        // Step 2: Simulate the transaction
        tracing::info!("submit_transaction: Simulating transaction");
        let sim_result: SimulateTransactionResult = self
            .rpc_request(
                "simulateTransaction",
                SimulateTransactionParams {
                    transaction: sim_envelope,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Simulation RPC failed");
                FacilitatorLocalError::from(e)
            })?;

        if let Some(ref error) = sim_result.error {
            tracing::error!(error = %error, "submit_transaction: Simulation returned error");
            return Err(StellarError::SimulationFailed {
                error: error.clone(),
            }
            .into());
        }

        tracing::info!(
            min_resource_fee = ?sim_result.min_resource_fee,
            transaction_data = ?sim_result.transaction_data.as_ref().map(|d| d.len()),
            results_count = sim_result.results.len(),
            "submit_transaction: Simulation successful"
        );

        // Step 3: Extract SorobanTransactionData from simulation result
        let transaction_data_xdr = sim_result.transaction_data.ok_or_else(|| {
            tracing::error!("submit_transaction: No transactionData in simulation result");
            StellarError::SimulationFailed {
                error: "No transactionData in simulation result".to_string(),
            }
        })?;

        let soroban_data_bytes = BASE64.decode(&transaction_data_xdr).map_err(|e| {
            tracing::error!(error = %e, "submit_transaction: Failed to decode transactionData base64");
            StellarError::InvalidXdr(format!("Failed to decode transactionData: {}", e))
        })?;

        let soroban_data = SorobanTransactionData::from_xdr(&soroban_data_bytes, Limits::none())
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Failed to parse SorobanTransactionData XDR");
                StellarError::InvalidXdr(format!("Failed to parse SorobanTransactionData: {}", e))
            })?;

        tracing::debug!("submit_transaction: Parsed SorobanTransactionData successfully");

        // Step 4: Calculate final fee = base_fee + min_resource_fee + margin
        let resource_fee_str = sim_result.min_resource_fee.as_ref().ok_or_else(|| {
            tracing::error!("submit_transaction: No minResourceFee in simulation result");
            StellarError::SimulationFailed {
                error: "No minResourceFee in simulation result".to_string(),
            }
        })?;

        let resource_fee: u64 = resource_fee_str.parse().map_err(|e| {
            tracing::error!(
                error = %e,
                resource_fee_str = %resource_fee_str,
                "submit_transaction: Failed to parse minResourceFee"
            );
            StellarError::SimulationFailed {
                error: format!(
                    "Failed to parse minResourceFee '{}': {}",
                    resource_fee_str, e
                ),
            }
        })?;

        // Add 15% margin to resource fee for safety, plus base fee
        let fee_with_margin = resource_fee.saturating_mul(115).saturating_div(100);
        let final_fee = (base_fee as u64).saturating_add(fee_with_margin);

        // Cap at u32::MAX (about 4.2 billion stroops = 429 XLM, way more than needed)
        let final_fee_u32 = if final_fee > u32::MAX as u64 {
            tracing::warn!(
                calculated_fee = final_fee,
                capped_fee = u32::MAX,
                "submit_transaction: Fee exceeds u32::MAX, capping"
            );
            u32::MAX
        } else {
            final_fee as u32
        };

        tracing::info!(
            base_fee = base_fee,
            resource_fee = resource_fee,
            fee_with_margin = fee_with_margin,
            final_fee = final_fee_u32,
            "submit_transaction: Calculated final fee with 15% margin"
        );

        // Step 5: Build signed envelope with SorobanTransactionData
        tracing::info!("submit_transaction: Building signed envelope with Soroban data");
        let signed_envelope = self
            .build_signed_envelope(verification, next_sequence, final_fee_u32, soroban_data)
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Failed to build signed envelope");
                FacilitatorLocalError::from(e)
            })?;

        tracing::info!(
            envelope_len = signed_envelope.len(),
            "submit_transaction: Built signed envelope"
        );

        // Step 6: Submit the signed transaction
        tracing::info!("submit_transaction: Sending transaction");
        let send_result: SendTransactionResult = self
            .rpc_request(
                "sendTransaction",
                SendTransactionParams {
                    transaction: signed_envelope,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "submit_transaction: Send RPC failed");
                FacilitatorLocalError::from(e)
            })?;

        if send_result.status == "ERROR" {
            let error_msg = send_result
                .error_result_xdr
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string());
            tracing::error!(
                status = %send_result.status,
                error_xdr = ?send_result.error_result_xdr,
                "submit_transaction: Transaction submission failed"
            );
            return Err(StellarError::SubmissionFailed(error_msg).into());
        }

        tracing::info!(
            tx_hash = %send_result.hash,
            status = %send_result.status,
            "submit_transaction: Transaction submitted successfully"
        );

        // Poll for transaction result
        let tx_hash = self.wait_for_transaction(&send_result.hash).await?;

        Ok(tx_hash)
    }

    /// Wait for a transaction to be confirmed
    async fn wait_for_transaction(&self, hash: &str) -> Result<[u8; 32], FacilitatorLocalError> {
        const MAX_ATTEMPTS: u32 = 30;
        const POLL_INTERVAL_MS: u64 = 1000;

        for attempt in 1..=MAX_ATTEMPTS {
            tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;

            let result: GetTransactionResult = self
                .rpc_request(
                    "getTransaction",
                    GetTransactionParams {
                        hash: hash.to_string(),
                    },
                )
                .await
                .map_err(FacilitatorLocalError::from)?;

            match result.status.as_str() {
                "SUCCESS" => {
                    tracing::info!(
                        tx_hash = %hash,
                        ledger = ?result.ledger,
                        "Stellar transaction confirmed"
                    );

                    // Convert hex hash to bytes
                    let hash_bytes = hex::decode(hash).map_err(|e| {
                        FacilitatorLocalError::Other(format!("Invalid tx hash: {}", e))
                    })?;

                    let mut tx_hash = [0u8; 32];
                    if hash_bytes.len() >= 32 {
                        tx_hash.copy_from_slice(&hash_bytes[..32]);
                    }

                    return Ok(tx_hash);
                }
                "FAILED" => {
                    let error = result.result_xdr.unwrap_or_else(|| "Unknown".to_string());
                    return Err(StellarError::TransactionFailed { status: error }.into());
                }
                "NOT_FOUND" => {
                    tracing::debug!(
                        tx_hash = %hash,
                        attempt = attempt,
                        "Transaction not yet confirmed, polling..."
                    );
                }
                status => {
                    tracing::debug!(
                        tx_hash = %hash,
                        status = %status,
                        attempt = attempt,
                        "Transaction in progress"
                    );
                }
            }
        }

        Err(StellarError::TransactionNotFound {
            attempts: MAX_ATTEMPTS,
        }
        .into())
    }
}

/// Result of verifying a Stellar payment
pub struct VerifyPaymentResult {
    pub payer: StellarAddress,
    pub auth_entry: SorobanAuthorizationEntry,
    pub to: String,
    pub amount: String,
    pub token_contract: String,
    pub nonce: u64,
    pub expiry_ledger: u32,
}

// =============================================================================
// Trait Implementations
// =============================================================================

impl FromEnvByNetworkBuild for StellarProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let rpc_url = std::env::var(from_env::rpc_env_name_from_network(network)).ok();

        // Get secret key from environment
        let secret_key = match from_env::SignerType::from_env()?.get_stellar_secret_key(network) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!(network=%network, error=%e, "no Stellar secret key configured, skipping");
                return Ok(None);
            }
        };

        let provider = StellarProvider::try_new(secret_key, rpc_url, network)?;
        Ok(Some(provider))
    }
}

impl NetworkProviderOps for StellarProvider {
    fn signer_address(&self) -> MixedAddress {
        self.facilitator_address()
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for StellarProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        tracing::info!("Stellar settle: Starting verification");
        let verification = self.verify_payment(request).await?;
        tracing::info!(
            payer = %verification.payer.address,
            amount = %verification.amount,
            "Stellar settle: Verification successful, submitting transaction"
        );

        // Submit the transaction
        let tx_hash = match self.submit_transaction(&verification).await {
            Ok(hash) => {
                tracing::info!(
                    tx_hash = ?hex::encode(&hash),
                    "Stellar settle: Transaction submitted successfully"
                );
                hash
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    error_debug = ?e,
                    "Stellar settle: Failed to submit transaction"
                );
                let response = SettleResponse {
                    success: false,
                    error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                    payer: verification.payer.into(),
                    transaction: None,
                    network: self.network(),
                    proof_of_payment: None,
                    extensions: None,
                };
                tracing::info!(
                    success = response.success,
                    error_reason = ?response.error_reason,
                    "Stellar settle: Returning failure response"
                );
                return Ok(response);
            }
        };

        let response = SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.into(),
            transaction: Some(TransactionHash::Stellar(tx_hash)),
            network: self.network(),
            proof_of_payment: None, // ERC-8004 not supported on Stellar
            extensions: None,
        };
        tracing::info!(
            success = response.success,
            tx_hash = ?response.transaction,
            "Stellar settle: Returning success response"
        );
        Ok(response)
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None, // TODO: Add Stellar token support
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
    fn test_stellar_address_validation() {
        // Valid G... address
        let valid_g = StellarAddress::new(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
        );
        assert!(valid_g.is_valid());

        // Valid C... contract address
        let valid_c = StellarAddress::new(
            "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHK3M".to_string(),
        );
        assert!(valid_c.is_valid());

        // Invalid address
        let invalid = StellarAddress::new("invalid".to_string());
        assert!(!invalid.is_valid());
    }

    #[test]
    fn test_network_passphrase() {
        let mainnet = StellarChain::try_from(Network::Stellar).unwrap();
        assert_eq!(mainnet.network_passphrase, STELLAR_MAINNET_PASSPHRASE);

        let testnet = StellarChain::try_from(Network::StellarTestnet).unwrap();
        assert_eq!(testnet.network_passphrase, STELLAR_TESTNET_PASSPHRASE);
    }

    /// Build a deterministic Transaction fixture used by the signature-payload
    /// invariant tests. Empty operations are valid XDR (variable-length array)
    /// and keep the fixture self-contained without any Soroban auth setup.
    fn signature_payload_fixture_tx() -> Transaction {
        let source_account = MuxedAccount::Ed25519(Uint256([0u8; 32]));
        let operations: VecM<Operation, 100> = vec![]
            .try_into()
            .expect("empty operations vec is a valid VecM");
        Transaction {
            source_account,
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations,
            ext: TransactionExt::V0,
        }
    }

    /// Frozen reference impl of the historical signature payload formula:
    /// network_id || ENVELOPE_TYPE_TX (= 2u32 BE) || tx.to_xdr().
    /// Kept inline in the test so any drift in the production signing path
    /// fails this assertion regardless of how production builds the bytes.
    fn manual_signature_payload(network_id: &[u8; 32], tx: &Transaction) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(network_id);
        buf.extend_from_slice(&2u32.to_be_bytes()); // ENVELOPE_TYPE_TX
        buf.extend_from_slice(&tx.to_xdr(Limits::none()).unwrap());
        buf
    }

    #[test]
    fn stellar_signature_payload_byte_invariant() {
        // Pins the SHA256 of the historical signature payload formula on the
        // deterministic fixture below. After the canonical-XDR migration the
        // production path computes the same bytes a different way, so this
        // hash must stay constant. If it ever changes, the signing wire
        // format has drifted and every Stellar settlement is at risk.
        let tx = signature_payload_fixture_tx();
        let chain = StellarChain::try_from(Network::Stellar).unwrap();
        let network_id = chain.network_id().0;

        let preimage = manual_signature_payload(&network_id, &tx);
        let frozen_hash = Sha256::digest(&preimage);

        // Pinned at handoff stellar-canonical-xdr-migration (2026-05-05).
        let expected_hex = "cac2ac369c44ca0a1120c3f6e2d8262b5870b0879a33a2f515d9fa1e6a700365";
        assert_eq!(hex::encode(frozen_hash), expected_hex);

        // Production path must produce the same hash as the historical formula.
        let production_hash = chain.compute_transaction_hash(&tx).unwrap();
        assert_eq!(hex::encode(&production_hash), expected_hex);
    }

    #[test]
    fn stellar_canonical_xdr_matches_manual_concat() {
        use stellar_xdr::curr::{
            TransactionSignaturePayload, TransactionSignaturePayloadTaggedTransaction,
        };

        let tx = signature_payload_fixture_tx();
        let chain = StellarChain::try_from(Network::Stellar).unwrap();
        let network_id = chain.network_id().0;

        let manual = manual_signature_payload(&network_id, &tx);

        let canonical = TransactionSignaturePayload {
            network_id: Hash(network_id),
            tagged_transaction: TransactionSignaturePayloadTaggedTransaction::Tx(tx.clone()),
        }
        .to_xdr(Limits::none())
        .unwrap();

        assert_eq!(
            manual, canonical,
            "canonical TransactionSignaturePayload XDR must equal manual concat"
        );
    }

    // ==========================================================================
    // B4: validate_soroban_auth_entry unit tests
    // ==========================================================================

    // Shared constants for B4 tests
    const TESTNET_USDC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
    const MAINNET_USDC: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";
    // Valid G-addresses for tests (seed-derived; the previous hand-written constants had
    // invalid strkey checksums and panicked in account_id() -- audit 01 test fix).
    // TEST_RECIPIENT = ed25519([2u8;32]), OTHER_ADDRESS = ed25519([1u8;32]).
    const TEST_RECIPIENT: &str = "GCATS5YOVB6ROX2WUNKGNQ2MP3GMXDMKSG2O4N5CLX3A6W4PZGZZI55U";
    // A different valid G-address for negative tests.
    const OTHER_ADDRESS: &str = "GCFIRY65OQE7DFP5KLNS2PF2LVZMUZYJX4OZIEQ36N2IQANUB5XVYOJR";

    /// Build a minimal StellarProvider with an all-zeros signing key for testing.
    ///
    /// This must not make any network calls; we only use it for the synchronous
    /// validate_soroban_auth_entry method.
    fn test_provider(network: Network) -> StellarProvider {
        // All-zeros 32-byte key is valid ed25519 (though useless for real signing)
        let signing_key = SigningKey::from_bytes(&[0u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let stellar_pk = StellarPublicKey(verifying_key.to_bytes());
        StellarProvider {
            signing_key: Arc::new(signing_key),
            public_key: stellar_pk.to_string(),
            http_client: Arc::new(reqwest::Client::new()),
            chain: StellarChain::try_from(network).unwrap(),
            rpc_url: None,
        }
    }

    /// Build the 32-byte contract hash from a C... strkey.
    fn contract_hash(addr: &str) -> Hash {
        Hash(Contract::from_string(addr).unwrap().0)
    }

    /// Build the AccountId for a G... strkey.
    fn account_id(addr: &str) -> AccountId {
        let pk_bytes = StellarPublicKey::from_string(addr).unwrap().0;
        AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk_bytes)))
    }

    /// Build a well-formed SorobanAuthorizationEntry for the given params.
    fn make_auth_entry(
        provider: &StellarProvider,
        contract: &str,
        fn_name: &str,
        sender_addr: &str,
        recipient_addr: &str,
        amount_i128: i128,
        sub_invocations: Vec<SorobanAuthorizedInvocation>,
    ) -> SorobanAuthorizationEntry {
        use stellar_xdr::curr::{
            ScSymbol, SorobanAuthorizedFunction, SorobanAuthorizedInvocation, StringM,
        };

        let args: Vec<ScVal> = vec![
            ScVal::Address(ScAddress::Account(account_id(sender_addr))),
            ScVal::Address(ScAddress::Account(account_id(recipient_addr))),
            ScVal::I128(Int128Parts {
                hi: (amount_i128 >> 64) as i64,
                lo: amount_i128 as u64,
            }),
        ];
        let args_vevm: VecM<ScVal> = args.try_into().unwrap();

        let fn_sym: StringM<32> = fn_name.try_into().unwrap();
        let fn_sc_sym = ScSymbol(fn_sym);

        let sub_vec: VecM<SorobanAuthorizedInvocation> = sub_invocations.try_into().unwrap();

        let invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: ScAddress::Contract(contract_hash(contract)),
                function_name: fn_sc_sym,
                args: args_vevm,
            }),
            sub_invocations: sub_vec,
        };

        // Credentials are not validated by validate_soroban_auth_entry; use SourceAccount for simplicity
        SorobanAuthorizationEntry {
            credentials: SorobanCredentials::SourceAccount,
            root_invocation: invocation,
        }
    }

    fn token_amount(v: u128) -> TokenAmount {
        TokenAmount(alloy::primitives::U256::from(v))
    }

    #[test]
    fn b4_payer_as_from_passes() {
        // SECURITY (audit 01): a payer-as-`from` entry (NOT the facilitator) must pass.
        let provider = test_provider(Network::StellarTestnet);
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            OTHER_ADDRESS, // from = payer (not the facilitator)
            TEST_RECIPIENT,
            1_000_000_0i128, // 1 USDC at 7 decimals
            vec![],
        );
        assert!(provider
            .validate_soroban_auth_entry(
                OTHER_ADDRESS,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000)
            )
            .is_ok());
    }

    #[test]
    fn b4_facilitator_as_from_rejected() {
        // SECURITY (audit 01): the self-drain primitive -- `from` = facilitator -- must be rejected.
        let provider = test_provider(Network::StellarTestnet);
        let facilitator = provider.public_key.clone();
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            &facilitator, // from = facilitator -> must be rejected
            TEST_RECIPIENT,
            10_000_000i128,
            vec![],
        );
        let err = provider
            .validate_soroban_auth_entry(
                &facilitator,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidSender { .. }),
            "facilitator-as-from must be rejected, got {:?}",
            err
        );
    }

    #[test]
    fn source_account_credentials_rejected() {
        // SECURITY (audit 01): SourceAccount credentials carry no payer signature and must
        // be rejected on the payment path (make_auth_entry emits SourceAccount).
        let provider = test_provider(Network::StellarTestnet);
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            OTHER_ADDRESS,
            TEST_RECIPIENT,
            10_000_000i128,
            vec![],
        );
        let err = provider
            .verify_authorization_signature(&entry, OTHER_ADDRESS)
            .unwrap_err();
        assert!(
            matches!(err, StellarError::UnsupportedCredentialType),
            "SourceAccount credentials must be rejected, got {:?}",
            err
        );
    }

    #[test]
    fn b4_wrong_contract_rejected() {
        let provider = test_provider(Network::StellarTestnet);
        // Use the mainnet USDC contract on a testnet entry -- should fail
        let entry = make_auth_entry(
            &provider,
            MAINNET_USDC,
            "transfer",
            OTHER_ADDRESS, // payer
            TEST_RECIPIENT,
            10_000_000i128,
            vec![],
        );
        let err = provider
            .validate_soroban_auth_entry(
                OTHER_ADDRESS,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidContractAddress { .. }),
            "expected InvalidContractAddress, got {:?}",
            err
        );
    }

    #[test]
    fn b4_wrong_function_name_rejected() {
        let provider = test_provider(Network::StellarTestnet);
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "approve",
            OTHER_ADDRESS, // payer
            TEST_RECIPIENT,
            10_000_000i128,
            vec![],
        );
        let err = provider
            .validate_soroban_auth_entry(
                OTHER_ADDRESS,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidFunctionName { .. }),
            "expected InvalidFunctionName, got {:?}",
            err
        );
    }

    #[test]
    fn b4_wrong_recipient_rejected() {
        let provider = test_provider(Network::StellarTestnet);
        // Payer is TEST_RECIPIENT; entry recipient is OTHER_ADDRESS but pay_to claims TEST_RECIPIENT.
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            TEST_RECIPIENT, // from = payer
            OTHER_ADDRESS,  // different recipient in the entry
            10_000_000i128,
            vec![],
        );
        // But we claim pay_to is TEST_RECIPIENT
        let err = provider
            .validate_soroban_auth_entry(
                TEST_RECIPIENT,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidRecipient { .. }),
            "expected InvalidRecipient, got {:?}",
            err
        );
    }

    #[test]
    fn b4_wrong_amount_rejected() {
        let provider = test_provider(Network::StellarTestnet);
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            OTHER_ADDRESS, // from = payer
            TEST_RECIPIENT,
            5_000_000i128, // entry says 0.5 USDC
            vec![],
        );
        // requirements say 1 USDC
        let err = provider
            .validate_soroban_auth_entry(
                OTHER_ADDRESS,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidAmount { .. }),
            "expected InvalidAmount, got {:?}",
            err
        );
    }

    #[test]
    fn b4_from_payer_mismatch_rejected() {
        // SECURITY (audit 01): entry `from` != declared payer must be rejected.
        let provider = test_provider(Network::StellarTestnet);
        // Entry says from = OTHER_ADDRESS but we declare the payer as TEST_RECIPIENT.
        let entry = make_auth_entry(
            &provider,
            TESTNET_USDC,
            "transfer",
            OTHER_ADDRESS,
            TEST_RECIPIENT,
            10_000_000i128,
            vec![],
        );
        let err = provider
            .validate_soroban_auth_entry(
                TEST_RECIPIENT,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidSender { .. }),
            "from != declared payer must be rejected, got {:?}",
            err
        );
    }

    #[test]
    fn b4_create_contract_fn_rejected() {
        use stellar_xdr::curr::{
            ContractExecutable, ContractIdPreimage, ContractIdPreimageFromAddress,
            CreateContractArgs,
        };

        let provider = test_provider(Network::StellarTestnet);

        // Build an entry that uses CreateContractHostFn
        let invocation = SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::CreateContractHostFn(CreateContractArgs {
                contract_id_preimage: ContractIdPreimage::Address(ContractIdPreimageFromAddress {
                    address: ScAddress::Contract(contract_hash(TESTNET_USDC)),
                    salt: Uint256([0u8; 32]),
                }),
                executable: ContractExecutable::StellarAsset,
            }),
            sub_invocations: vec![].try_into().unwrap(),
        };
        let entry = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::SourceAccount,
            root_invocation: invocation,
        };
        let err = provider
            .validate_soroban_auth_entry(
                OTHER_ADDRESS,
                &entry,
                TEST_RECIPIENT,
                token_amount(10_000_000),
            )
            .unwrap_err();
        assert!(
            matches!(err, StellarError::InvalidInvocationType),
            "expected InvalidInvocationType, got {:?}",
            err
        );
    }
}
