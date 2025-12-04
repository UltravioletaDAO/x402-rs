//! NEAR Protocol payment provider implementation.
//!
//! This module implements NEAR payments using NEP-366 meta-transactions.
//! Users sign a DelegateAction off-chain, and the facilitator wraps it in a
//! transaction, paying the gas fees on behalf of the user.
//!
//! Flow:
//! 1. User creates and signs a DelegateAction -> SignedDelegateAction
//! 2. User sends SignedDelegateAction to facilitator (base64 encoded)
//! 3. Facilitator wraps it in Action::Delegate and creates a Transaction
//! 4. Facilitator signs the Transaction with its own key (pays gas)
//! 5. Facilitator submits to NEAR network
//! 6. NEAR executes the inner actions as if user submitted them

use near_crypto::{InMemorySigner, PublicKey, SecretKey, Signer};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::action::delegate::SignedDelegateAction;
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Action, Transaction, TransactionV0};
use near_primitives::types::{AccountId, BlockReference, Finality, Nonce};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env;
use crate::network::Network;
use crate::types::{
    ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, Scheme, SettleRequest,
    SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse,
    TransactionHash, VerifyRequest, VerifyResponse, X402Version,
};

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
/// Implements USDC payments on NEAR using NEP-366 meta-transactions.
/// The facilitator receives SignedDelegateAction from users and wraps them
/// in transactions, paying the gas fees.
#[derive(Clone)]
pub struct NearProvider {
    /// The relayer's signer for signing transactions
    signer: Arc<Signer>,
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

        // Create an in-memory signer for the relayer and convert to Signer enum
        let signer: Signer = InMemorySigner::from_secret_key(account_id.clone(), secret_key).into();

        tracing::info!(
            network = %network,
            account_id = %account_id,
            "Initialized NEAR provider with NEP-366 meta-transaction support"
        );

        let rpc_client = JsonRpcClient::connect(&rpc_url);

        Ok(Self {
            signer: Arc::new(signer),
            account_id,
            rpc_client: Arc::new(rpc_client),
            chain,
        })
    }

    /// Get the relayer's public key
    pub fn public_key(&self) -> PublicKey {
        self.signer.public_key()
    }

    /// Get the relayer's account ID as a MixedAddress
    pub fn relayer_address(&self) -> MixedAddress {
        MixedAddress::Near(self.account_id.to_string())
    }

    /// Query the current nonce for the relayer's access key
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

    /// Decode a SignedDelegateAction from base64
    fn decode_signed_delegate_action(
        &self,
        encoded: &str,
    ) -> Result<SignedDelegateAction, FacilitatorLocalError> {
        // Decode from base64
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map_err(|e| {
                FacilitatorLocalError::DecodingError(format!(
                    "Failed to decode SignedDelegateAction from base64: {e}"
                ))
            })?;

        // Deserialize using borsh
        let signed_delegate_action: SignedDelegateAction = borsh::from_slice(&bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to deserialize SignedDelegateAction: {e}"
            ))
        })?;

        Ok(signed_delegate_action)
    }

    /// Verify a SignedDelegateAction
    fn verify_delegate_action(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<(), FacilitatorLocalError> {
        // Verify the signature
        if !signed_delegate_action.verify() {
            let sender_address =
                MixedAddress::Near(signed_delegate_action.delegate_action.sender_id.to_string());
            return Err(FacilitatorLocalError::InvalidSignature(
                sender_address,
                "Invalid SignedDelegateAction signature".to_string(),
            ));
        }

        Ok(())
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

        // Decode the SignedDelegateAction
        let signed_delegate_action =
            self.decode_signed_delegate_action(&near_payload.signed_delegate_action)?;

        // Verify the signature
        self.verify_delegate_action(&signed_delegate_action)?;

        // Extract payer from the delegate action's sender
        let payer = NearAddress::new(signed_delegate_action.delegate_action.sender_id.clone());

        // Verify the delegate action targets the USDC contract
        let usdc_contract = match &requirements.asset {
            MixedAddress::Near(contract) => contract.clone(),
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "Asset must be a NEAR address".to_string(),
                ))
            }
        };

        if signed_delegate_action.delegate_action.receiver_id.to_string() != usdc_contract {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "DelegateAction receiver {} does not match USDC contract {}",
                signed_delegate_action.delegate_action.receiver_id, usdc_contract
            )));
        }

        Ok(VerifyPaymentResult {
            payer,
            signed_delegate_action,
        })
    }

    /// Submit a meta-transaction (NEP-366)
    ///
    /// Wraps the SignedDelegateAction in a Transaction with Action::Delegate,
    /// signs it with the relayer's key, and submits to the network.
    /// The relayer pays the gas fees.
    async fn submit_meta_transaction(
        &self,
        signed_delegate_action: SignedDelegateAction,
    ) -> Result<CryptoHash, FacilitatorLocalError> {
        // Get current nonce and block hash for the relayer's account
        let nonce = self.get_nonce().await? + 1;
        let block_hash = self.get_block_hash().await?;

        // The receiver of the outer transaction is the sender of the delegate action
        // This is because the delegate action is executed "as if" the sender submitted it
        let receiver_id = signed_delegate_action.delegate_action.sender_id.clone();

        // Create the Action::Delegate wrapping the SignedDelegateAction
        let actions = vec![Action::Delegate(Box::new(signed_delegate_action))];

        // Create the transaction using TransactionV0 - the relayer is the signer (pays gas)
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key: self.public_key(),
            nonce,
            receiver_id,
            block_hash,
            actions,
        });

        // Sign the transaction with the relayer's key
        let signed_tx = transaction.sign(&*self.signer);

        tracing::info!(
            relayer = %self.account_id,
            nonce = nonce,
            "Submitting NEP-366 meta-transaction (relayer pays gas)"
        );

        // Submit the transaction
        let request =
            methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest { signed_transaction: signed_tx };

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to submit meta-transaction: {e}"))
        })?;

        // Check for execution errors
        if let near_primitives::views::FinalExecutionStatus::Failure(err) = response.status {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "Meta-transaction failed: {:?}",
                err
            )));
        }

        tracing::info!(
            tx_hash = %response.transaction.hash,
            "NEP-366 meta-transaction submitted successfully"
        );

        Ok(response.transaction.hash)
    }
}

/// Result of verifying a NEAR payment
pub struct VerifyPaymentResult {
    pub payer: NearAddress,
    pub signed_delegate_action: SignedDelegateAction,
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

        // Submit the meta-transaction (relayer pays gas!)
        let tx_hash = match self
            .submit_meta_transaction(verification.signed_delegate_action)
            .await
        {
            Ok(hash) => hash,
            Err(e) => {
                tracing::error!(error = %e, "Failed to submit NEAR meta-transaction");
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
