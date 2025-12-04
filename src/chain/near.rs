//! NEAR Protocol payment provider implementation.
//!
//! This module implements NEAR payments using signed transactions.
//! The facilitator receives pre-signed transactions from users and submits them on-chain.
//!
//! Note: NEP-366 meta-transactions require near-primitives 0.27+ which is not yet
//! compatible with near-jsonrpc-client 0.13. This implementation uses a simpler
//! pre-signed transaction model.

use near_crypto::{PublicKey, SecretKey};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::SignedTransaction;
use near_primitives::types::{AccountId, BlockReference, Finality, Gas, Nonce};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::Network;
use crate::types::{
    ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, PaymentRequirements, Scheme,
    SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, TokenAmount, TransactionHash, VerifyRequest, VerifyResponse,
    X402Version,
};

/// Gas limit for ft_transfer calls (30 TGas is typically sufficient)
#[allow(dead_code)]
const FT_TRANSFER_GAS: Gas = 30_000_000_000_000;

/// NEAR network chain configuration
#[derive(Clone, Debug)]
pub struct NearChain {
    pub network: Network,
}

impl TryFrom<Network> for NearChain {
    type Error = FacilitatorLocalError;

    fn try_from(value: Network) -> Result<Self, Self::Error> {
        match value {
            Network::Near => Ok(Self { network: value }),
            Network::NearTestnet => Ok(Self { network: value }),
            // All other networks are unsupported by this provider
            _ => Err(FacilitatorLocalError::UnsupportedNetwork(None)),
        }
    }
}

/// NEAR account address wrapper
#[derive(Clone, Debug)]
pub struct NearAddress {
    pub account_id: AccountId,
}

impl NearAddress {
    /// Create a new NearAddress from an AccountId
    pub fn new(account_id: AccountId) -> Self {
        Self { account_id }
    }
}

impl TryFrom<String> for NearAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        AccountId::from_str(&value)
            .map(|account_id| Self { account_id })
            .map_err(|e| FacilitatorLocalError::InvalidAddress(format!("Invalid NEAR account: {e}")))
    }
}

impl TryFrom<MixedAddress> for NearAddress {
    type Error = FacilitatorLocalError;

    fn try_from(value: MixedAddress) -> Result<Self, Self::Error> {
        match value {
            MixedAddress::Near(account_id_str) => Self::try_from(account_id_str),
            _ => Err(FacilitatorLocalError::InvalidAddress(
                "expected NEAR address".to_string(),
            )),
        }
    }
}

impl From<NearAddress> for MixedAddress {
    fn from(value: NearAddress) -> Self {
        MixedAddress::Near(value.account_id.to_string())
    }
}

impl From<AccountId> for NearAddress {
    fn from(account_id: AccountId) -> Self {
        Self { account_id }
    }
}

/// NEP-141 ft_transfer arguments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtTransferArgs {
    pub receiver_id: String,
    pub amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// NEAR Protocol payment provider
///
/// Implements USDC payments on NEAR by relaying pre-signed transactions.
/// The facilitator submits user-signed transactions on-chain.
#[derive(Clone)]
pub struct NearProvider {
    /// The relayer's secret key for signing transactions (for future meta-tx support)
    #[allow(dead_code)]
    secret_key: Arc<SecretKey>,
    /// The relayer's account ID
    account_id: AccountId,
    /// NEAR RPC client
    rpc_client: Arc<JsonRpcClient>,
    /// Network configuration
    chain: NearChain,
}

impl Debug for NearProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NearProvider")
            .field("account_id", &self.account_id)
            .field("chain", &self.chain)
            .finish()
    }
}

impl NearProvider {
    /// Create a new NEAR provider
    pub fn try_new(
        secret_key: SecretKey,
        account_id: String,
        rpc_url: String,
        network: Network,
    ) -> Result<Self, FacilitatorLocalError> {
        let chain = NearChain::try_from(network)?;
        let account_id = AccountId::from_str(&account_id)
            .map_err(|e| FacilitatorLocalError::InvalidAddress(format!("Invalid account ID: {e}")))?;

        tracing::info!(
            network = %network,
            account_id = %account_id,
            "Initialized NEAR provider"
        );

        let rpc_client = JsonRpcClient::connect(&rpc_url);

        Ok(Self {
            secret_key: Arc::new(secret_key),
            account_id,
            rpc_client: Arc::new(rpc_client),
            chain,
        })
    }

    /// Get the relayer's public key
    pub fn public_key(&self) -> PublicKey {
        self.secret_key.public_key()
    }

    /// Get the relayer's account ID as a MixedAddress
    pub fn relayer_address(&self) -> MixedAddress {
        MixedAddress::Near(self.account_id.to_string())
    }

    /// Query the current nonce for an access key
    #[allow(dead_code)]
    async fn get_nonce(&self) -> Result<Nonce, FacilitatorLocalError> {
        let public_key = self.public_key();
        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::Final),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: self.account_id.clone(),
                public_key: public_key.clone(),
            },
        };

        let response = self
            .rpc_client
            .call(request)
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("Failed to query nonce: {e}")))?;

        match response.kind {
            QueryResponseKind::AccessKey(access_key) => Ok(access_key.nonce),
            _ => Err(FacilitatorLocalError::ContractCall(
                "Unexpected query response kind".to_string(),
            )),
        }
    }

    /// Get the latest block hash for transaction construction
    #[allow(dead_code)]
    async fn get_block_hash(&self) -> Result<CryptoHash, FacilitatorLocalError> {
        let request = methods::block::RpcBlockRequest {
            block_reference: BlockReference::Finality(Finality::Final),
        };

        let response = self
            .rpc_client
            .call(request)
            .await
            .map_err(|e| FacilitatorLocalError::ContractCall(format!("Failed to get block: {e}")))?;

        Ok(response.header.hash)
    }

    /// Decode a signed transaction from base64
    fn decode_signed_transaction(
        &self,
        encoded: &str,
    ) -> Result<SignedTransaction, FacilitatorLocalError> {
        // Decode from base64
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| {
                FacilitatorLocalError::DecodingError(format!(
                    "Failed to decode signed transaction from base64: {e}"
                ))
            })?;

        // Deserialize using borsh
        let signed_tx: SignedTransaction = borsh::from_slice(&bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to deserialize SignedTransaction: {e}"
            ))
        })?;

        Ok(signed_tx)
    }

    /// Verify a payment request
    async fn verify_payment(
        &self,
        request: &VerifyRequest,
    ) -> Result<VerifyPaymentResult, FacilitatorLocalError> {
        let payload = &request.payment_payload;
        let requirements = &request.payment_requirements;

        // Extract NEAR payload
        let near_payload = match &payload.payload {
            ExactPaymentPayload::Near(p) => p,
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

        // Decode the signed transaction (field is called signed_delegate_action for now,
        // but contains a serialized SignedTransaction in this simplified implementation)
        let signed_tx = self.decode_signed_transaction(&near_payload.signed_delegate_action)?;

        // Extract payer from the transaction's signer
        let payer = NearAddress::new(signed_tx.transaction.signer_id().clone());

        // Verify the transaction is for the USDC contract
        let usdc_contract = match &requirements.asset {
            MixedAddress::Near(contract) => contract.clone(),
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "Asset must be a NEAR address".to_string(),
                ))
            }
        };

        if signed_tx.transaction.receiver_id().to_string() != usdc_contract {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "Transaction receiver {} does not match USDC contract {}",
                signed_tx.transaction.receiver_id(),
                usdc_contract
            )));
        }

        Ok(VerifyPaymentResult { payer, signed_tx })
    }

    /// Submit a signed transaction
    async fn submit_transaction(
        &self,
        signed_tx: SignedTransaction,
    ) -> Result<CryptoHash, FacilitatorLocalError> {
        let request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction: signed_tx };

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to submit transaction: {e}"))
        })?;

        // Check for execution errors
        if let near_primitives::views::FinalExecutionStatus::Failure(err) = response.status {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "Transaction failed: {:?}",
                err
            )));
        }

        Ok(response.transaction.hash)
    }
}

/// Result of verifying a NEAR payment
pub struct VerifyPaymentResult {
    pub payer: NearAddress,
    pub signed_tx: SignedTransaction,
}

impl FromEnvByNetworkBuild for NearProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let env_var = from_env::rpc_env_name_from_network(network);
        let rpc_url = match std::env::var(env_var).ok() {
            Some(rpc_url) => rpc_url,
            None => {
                tracing::warn!(network=%network, "no RPC URL configured, skipping");
                return Ok(None);
            }
        };

        let (secret_key, account_id) =
            from_env::SignerType::from_env()?.make_near_signer(network)?;

        let provider = NearProvider::try_new(secret_key, account_id, rpc_url, network)?;
        Ok(Some(provider))
    }
}

impl NetworkProviderOps for NearProvider {
    fn signer_address(&self) -> MixedAddress {
        self.relayer_address()
    }

    fn network(&self) -> Network {
        self.chain.network
    }
}

impl Facilitator for NearProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;

        // Submit the transaction
        let tx_hash = match self.submit_transaction(verification.signed_tx).await {
            Ok(hash) => hash,
            Err(e) => {
                tracing::error!(error = %e, "Failed to submit NEAR transaction");
                return Ok(SettleResponse {
                    success: false,
                    error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                    payer: verification.payer.into(),
                    transaction: None,
                    network: self.network(),
                });
            }
        };

        // Convert hash to TransactionHash::Near
        let tx_hash_bytes: [u8; 32] = tx_hash.0;

        Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer: verification.payer.into(),
            transaction: Some(TransactionHash::Near(tx_hash_bytes)),
            network: self.network(),
        })
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: self.signer_address(),
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}
