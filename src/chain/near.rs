//! NEAR Protocol payment provider implementation.
//!
//! This module implements NEAR payments using NEP-366 meta-transactions.
//! Users sign a DelegateAction off-chain, and the facilitator wraps it in a
//! transaction, paying the gas fees on behalf of the user.
//!
//! Flow:
//! 1. User creates and signs a DelegateAction -> SignedDelegateAction
//! 2. User sends SignedDelegateAction to facilitator (base64 encoded)
//! 3. Facilitator checks if USDC recipient is registered (storage_balance_of)
//! 4. If not registered, facilitator calls storage_deposit (pays ~0.00125 NEAR)
//! 5. Facilitator wraps SignedDelegateAction in Action::Delegate
//! 6. Facilitator signs the Transaction with its own key (pays gas)
//! 7. Facilitator submits to NEAR network
//! 8. NEAR executes the inner actions as if user submitted them

use near_crypto::{InMemorySigner, PublicKey, SecretKey, Signer};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryError};
use near_primitives::action::delegate::{NonDelegateAction, SignedDelegateAction};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Action, FunctionCallAction, Transaction, TransactionV0};
use near_primitives::types::{AccountId, BlockReference, Finality, Gas, Nonce};
use near_token::NearToken;
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

/// Storage deposit amount in yoctoNEAR (0.00125 NEAR = 1.25e21 yoctoNEAR)
const STORAGE_DEPOSIT_AMOUNT: NearToken = NearToken::from_yoctonear(1_250_000_000_000_000_000_000);

/// Gas for storage_deposit call (5 TGas should be enough)
const STORAGE_DEPOSIT_GAS: Gas = Gas::from_gas(5_000_000_000_000);

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
            .map_err(|e| {
                FacilitatorLocalError::InvalidAddress(format!("Invalid NEAR account: {e}"))
            })
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

/// NEP-141 storage_deposit arguments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDepositArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_only: Option<bool>,
}

/// NEP-141 storage_balance_of response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBalance {
    pub total: String,
    pub available: String,
}

/// Validate that a slice of NonDelegateActions (from a DelegateAction) satisfies
/// the payment requirements:
///
/// - At least one action must be present.
/// - Every action must be a FunctionCall with method_name == "ft_transfer".
/// - Each ft_transfer's JSON args must decode to { receiver_id, amount }.
/// - receiver_id must exactly equal expected_receiver.
/// - amount must exactly equal expected_amount (decimal string in atomic units).
///
/// Any deviation is rejected. Mixed action envelopes (e.g., Transfer + FunctionCall)
/// are also rejected because we cannot safely reason about their combined effect.
pub(crate) fn validate_delegate_actions_inner(
    actions: &[NonDelegateAction],
    expected_receiver: &str,
    expected_amount: &str,
) -> Result<(), FacilitatorLocalError> {
    if actions.is_empty() {
        return Err(FacilitatorLocalError::Other(
            "DelegateAction contains no inner actions".to_string(),
        ));
    }

    for (i, non_delegate_action) in actions.iter().enumerate() {
        let action: Action = non_delegate_action.clone().into();
        match action {
            Action::FunctionCall(fc) => {
                if fc.method_name != "ft_transfer" {
                    tracing::warn!(
                        index = i,
                        method = %fc.method_name,
                        "Rejected DelegateAction: inner action is not ft_transfer"
                    );
                    return Err(FacilitatorLocalError::Other(format!(
                        "DelegateAction action[{}] method '{}' is not ft_transfer",
                        i, fc.method_name
                    )));
                }

                let args: FtTransferArgs = serde_json::from_slice(&fc.args).map_err(|e| {
                    FacilitatorLocalError::DecodingError(format!(
                        "Failed to parse ft_transfer args at action[{}]: {e}",
                        i
                    ))
                })?;

                if args.receiver_id != expected_receiver {
                    tracing::warn!(
                        index = i,
                        got = %args.receiver_id,
                        expected = %expected_receiver,
                        "Rejected DelegateAction: ft_transfer receiver_id mismatch"
                    );
                    return Err(FacilitatorLocalError::Other(format!(
                        "DelegateAction action[{}] ft_transfer receiver_id '{}' does not match pay_to '{}'",
                        i, args.receiver_id, expected_receiver
                    )));
                }

                if args.amount != expected_amount {
                    tracing::warn!(
                        index = i,
                        got = %args.amount,
                        expected = %expected_amount,
                        "Rejected DelegateAction: ft_transfer amount mismatch"
                    );
                    return Err(FacilitatorLocalError::Other(format!(
                        "DelegateAction action[{}] ft_transfer amount '{}' does not match required amount '{}'",
                        i, args.amount, expected_amount
                    )));
                }
            }
            other => {
                tracing::warn!(
                    index = i,
                    action_type = ?other,
                    "Rejected DelegateAction: unexpected action type (only ft_transfer allowed)"
                );
                return Err(FacilitatorLocalError::Other(format!(
                    "DelegateAction action[{}] is not a FunctionCall (only ft_transfer allowed)",
                    i
                )));
            }
        }
    }

    Ok(())
}

/// NEAR Protocol payment provider
///
/// Implements USDC payments on NEAR using NEP-366 meta-transactions.
/// The facilitator receives SignedDelegateAction from users and wraps them
/// in transactions, paying the gas fees.
///
/// Features:
/// - Auto-registration: If the USDC recipient is not registered on the token
///   contract, the facilitator will call storage_deposit before the transfer.
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
        let account_id = AccountId::from_str(&account_id).map_err(|e| {
            FacilitatorLocalError::InvalidAddress(format!("Invalid account ID: {e}"))
        })?;

        // Create an in-memory signer for the relayer and convert to Signer enum
        let signer: Signer = InMemorySigner::from_secret_key(account_id.clone(), secret_key).into();

        tracing::info!(
            network = %network,
            account_id = %account_id,
            "Initialized NEAR provider with NEP-366 meta-transaction support and auto-registration"
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

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to query nonce: {e}"))
        })?;

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

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to get block: {e}"))
        })?;

        Ok(response.header.hash)
    }

    /// Extract the USDC receiver from a SignedDelegateAction
    ///
    /// Parses the ft_transfer action args to get the actual recipient of the USDC.
    fn extract_usdc_receiver(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<AccountId, FacilitatorLocalError> {
        // Look for ft_transfer action in the delegate actions
        for non_delegate_action in &signed_delegate_action.delegate_action.actions {
            // Convert NonDelegateAction to Action to pattern match
            let action: Action = non_delegate_action.clone().into();
            if let Action::FunctionCall(func_call) = action {
                if func_call.method_name == "ft_transfer" {
                    // Parse the args as JSON to get receiver_id
                    let args: FtTransferArgs =
                        serde_json::from_slice(&func_call.args).map_err(|e| {
                            FacilitatorLocalError::DecodingError(format!(
                                "Failed to parse ft_transfer args: {e}"
                            ))
                        })?;

                    let receiver_id = AccountId::from_str(&args.receiver_id).map_err(|e| {
                        FacilitatorLocalError::InvalidAddress(format!(
                            "Invalid receiver_id in ft_transfer: {e}"
                        ))
                    })?;

                    return Ok(receiver_id);
                }
            }
        }

        Err(FacilitatorLocalError::DecodingError(
            "No ft_transfer action found in SignedDelegateAction".to_string(),
        ))
    }

    /// Check if an account is registered on a NEP-141 token contract
    ///
    /// Calls storage_balance_of view method. Returns true if registered, false otherwise.
    async fn is_account_registered(
        &self,
        token_contract: &AccountId,
        account_id: &AccountId,
    ) -> Result<bool, FacilitatorLocalError> {
        let args = serde_json::json!({
            "account_id": account_id.to_string()
        });

        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::Final),
            request: near_primitives::views::QueryRequest::CallFunction {
                account_id: token_contract.clone(),
                method_name: "storage_balance_of".to_string(),
                args: near_primitives::types::FunctionArgs::from(args.to_string().into_bytes()),
            },
        };

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to call storage_balance_of: {e}"))
        })?;

        match response.kind {
            QueryResponseKind::CallResult(result) => {
                // If the result is "null" or empty, account is not registered
                let result_str = String::from_utf8_lossy(&result.result);
                let is_registered = result_str != "null" && !result_str.is_empty();

                tracing::debug!(
                    token_contract = %token_contract,
                    account_id = %account_id,
                    is_registered = is_registered,
                    "Checked storage balance"
                );

                Ok(is_registered)
            }
            _ => Err(FacilitatorLocalError::ContractCall(
                "Unexpected query response kind for storage_balance_of".to_string(),
            )),
        }
    }

    /// Register an account on a NEP-141 token contract by calling storage_deposit
    ///
    /// The facilitator pays the storage deposit (~0.00125 NEAR).
    async fn register_account(
        &self,
        token_contract: &AccountId,
        account_id: &AccountId,
    ) -> Result<CryptoHash, FacilitatorLocalError> {
        let nonce = self.get_nonce().await? + 1;
        let block_hash = self.get_block_hash().await?;

        // Prepare storage_deposit args
        let args = StorageDepositArgs {
            account_id: Some(account_id.to_string()),
            registration_only: Some(true),
        };
        let args_json = serde_json::to_vec(&args).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to serialize storage_deposit args: {e}"
            ))
        })?;

        // Create storage_deposit action
        let actions = vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "storage_deposit".to_string(),
            args: args_json,
            gas: STORAGE_DEPOSIT_GAS,
            deposit: STORAGE_DEPOSIT_AMOUNT,
        }))];

        // Create and sign transaction
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key: self.public_key(),
            nonce,
            receiver_id: token_contract.clone(),
            block_hash,
            actions,
        });

        let signed_tx = transaction.sign(&*self.signer);

        tracing::info!(
            relayer = %self.account_id,
            token_contract = %token_contract,
            account_to_register = %account_id,
            deposit_near = "0.00125",
            "Registering account on token contract (storage_deposit)"
        );

        // Submit the transaction
        let request = methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest {
            signed_transaction: signed_tx,
        };

        let response = self.rpc_client.call(request).await.map_err(|e| {
            FacilitatorLocalError::ContractCall(format!("Failed to submit storage_deposit: {e}"))
        })?;

        // Check for execution errors
        if let near_primitives::views::FinalExecutionStatus::Failure(err) = response.status {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "storage_deposit failed: {:?}",
                err
            )));
        }

        tracing::info!(
            tx_hash = %response.transaction.hash,
            account_registered = %account_id,
            "Account registered successfully on token contract"
        );

        Ok(response.transaction.hash)
    }

    /// Ensure the USDC recipient is registered on the token contract
    ///
    /// If not registered, automatically calls storage_deposit (facilitator pays).
    async fn ensure_recipient_registered(
        &self,
        signed_delegate_action: &SignedDelegateAction,
    ) -> Result<(), FacilitatorLocalError> {
        // Get the token contract (receiver of the delegate action)
        let token_contract = &signed_delegate_action.delegate_action.receiver_id;

        // Extract the USDC recipient from ft_transfer args
        let usdc_receiver = self.extract_usdc_receiver(signed_delegate_action)?;

        // Check if the recipient is registered
        let is_registered = self
            .is_account_registered(token_contract, &usdc_receiver)
            .await?;

        if !is_registered {
            tracing::warn!(
                token_contract = %token_contract,
                usdc_receiver = %usdc_receiver,
                "USDC recipient not registered, auto-registering..."
            );

            // Register the recipient (facilitator pays storage deposit)
            self.register_account(token_contract, &usdc_receiver)
                .await?;
        }

        Ok(())
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
        let signed_delegate_action: SignedDelegateAction =
            borsh::from_slice(&bytes).map_err(|e| {
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

    /// Validate inner actions of a SignedDelegateAction against payment requirements.
    ///
    /// Delegates to the free function `validate_delegate_actions_inner` so the logic
    /// is independently unit-testable without a live NearProvider.
    fn validate_delegate_actions(
        &self,
        signed_delegate_action: &SignedDelegateAction,
        expected_receiver: &str,
        expected_amount: &str,
    ) -> Result<(), FacilitatorLocalError> {
        validate_delegate_actions_inner(
            &signed_delegate_action.delegate_action.actions,
            expected_receiver,
            expected_amount,
        )
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

        if signed_delegate_action
            .delegate_action
            .receiver_id
            .to_string()
            != usdc_contract
        {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "DelegateAction receiver {} does not match USDC contract {}",
                signed_delegate_action.delegate_action.receiver_id, usdc_contract
            )));
        }

        // Validate inner actions: every action must be ft_transfer and its
        // args must match the payment requirements exactly (receiver_id and amount).
        let expected_receiver = match &requirements.pay_to {
            MixedAddress::Near(account_id_str) => account_id_str.clone(),
            _ => {
                return Err(FacilitatorLocalError::InvalidAddress(
                    "pay_to must be a NEAR address".to_string(),
                ))
            }
        };
        let expected_amount = requirements.max_amount_required.to_string();

        self.validate_delegate_actions(
            &signed_delegate_action,
            &expected_receiver,
            &expected_amount,
        )?;

        Ok(VerifyPaymentResult {
            payer,
            signed_delegate_action,
        })
    }

    /// Refuse a delegate action whose nonce the payer's access key has reached.
    ///
    /// The runtime executes a delegate action only while its nonce is above the
    /// access key's, and executing it raises the key's nonce to that value, so a
    /// delegate action that already ran fails here. That also covers "already
    /// executed": the relayer's transaction hash does not exist until settle,
    /// the access key nonce does. Read at optimistic finality so a settlement
    /// that just landed counts before its block is final.
    async fn check_delegate_nonce_unused(
        &self,
        signed_delegate_action: &SignedDelegateAction,
        timeout: std::time::Duration,
    ) -> Result<(), FacilitatorLocalError> {
        let delegate_action = &signed_delegate_action.delegate_action;
        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::None),
            request: near_primitives::views::QueryRequest::ViewAccessKey {
                account_id: delegate_action.sender_id.clone(),
                public_key: delegate_action.public_key.clone(),
            },
        };

        let response = match tokio::time::timeout(timeout, self.rpc_client.call(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                if let Some(
                    RpcQueryError::UnknownAccessKey { .. } | RpcQueryError::UnknownAccount { .. },
                ) = e.handler_error()
                {
                    return Err(FacilitatorLocalError::Other(format!(
                        "Access key {} does not exist for {}",
                        delegate_action.public_key, delegate_action.sender_id
                    )));
                }
                return Err(FacilitatorLocalError::ContractCall(format!(
                    "Failed to query access key: {e}"
                )));
            }
            Err(_) => {
                return Err(FacilitatorLocalError::ContractCall(format!(
                    "Access key query timed out after {}ms",
                    timeout.as_millis()
                )))
            }
        };

        match response.kind {
            QueryResponseKind::AccessKey(access_key)
                if delegate_action.nonce <= access_key.nonce =>
            {
                Err(FacilitatorLocalError::Other(format!(
                    "Delegate action nonce {} already used for {} (access key nonce {})",
                    delegate_action.nonce, delegate_action.sender_id, access_key.nonce
                )))
            }
            QueryResponseKind::AccessKey(_) => Ok(()),
            _ => Err(FacilitatorLocalError::ContractCall(
                "Unexpected query response kind".to_string(),
            )),
        }
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
        let request = methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest {
            signed_transaction: signed_tx,
        };

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
        // Settle is unchanged: the runtime refuses a used nonce on submission.
        self.check_delegate_nonce_unused(
            &verification.signed_delegate_action,
            crate::chain::rpc_http_timeout(),
        )
        .await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;

        // IMPORTANT: Ensure recipient is registered BEFORE submitting meta-transaction
        // This prevents the "account not registered" error and avoids wasting the user's nonce
        if let Err(e) = self
            .ensure_recipient_registered(&verification.signed_delegate_action)
            .await
        {
            tracing::error!(error = %e, "Failed to ensure recipient registration");
            return Ok(SettleResponse {
                success: false,
                error_reason: Some(FacilitatorErrorReason::UnexpectedSettleError),
                payer: verification.payer.into(),
                transaction: None,
                network: self.network(),
                proof_of_payment: None,
                extensions: None,
            });
        }

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
                    proof_of_payment: None,
                    extensions: None,
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
            proof_of_payment: None, // ERC-8004 not supported on NEAR
            extensions: None,
        })
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            network_aliases: None,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None, // TODO: Add NEAR token support
                escrow: None,
            }),
        }];
        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_primitives::transaction::TransferAction;

    /// Build a NonDelegateAction wrapping a ft_transfer FunctionCall with the given args JSON.
    fn make_ft_transfer_action(args_json: &str) -> NonDelegateAction {
        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer".to_string(),
            args: args_json.as_bytes().to_vec(),
            gas: Gas::from_gas(5_000_000_000_000),
            deposit: NearToken::from_yoctonear(1),
        }));
        NonDelegateAction::try_from(action).expect("FunctionCall is a valid NonDelegateAction")
    }

    /// Build a NonDelegateAction wrapping an arbitrary method name.
    fn make_func_call_action(method: &str, args_json: &str) -> NonDelegateAction {
        let action = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: method.to_string(),
            args: args_json.as_bytes().to_vec(),
            gas: Gas::from_gas(5_000_000_000_000),
            deposit: NearToken::from_yoctonear(0),
        }));
        NonDelegateAction::try_from(action).expect("FunctionCall is a valid NonDelegateAction")
    }

    #[test]
    fn valid_single_ft_transfer_passes() {
        let args = r#"{"receiver_id":"merchant.near","amount":"1000000"}"#;
        let actions = vec![make_ft_transfer_action(args)];
        let result = validate_delegate_actions_inner(&actions, "merchant.near", "1000000");
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    #[test]
    fn wrong_receiver_id_is_rejected() {
        let args = r#"{"receiver_id":"attacker.near","amount":"1000000"}"#;
        let actions = vec![make_ft_transfer_action(args)];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on receiver_id mismatch");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("receiver_id"),
                    "error message should mention receiver_id, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn wrong_amount_is_rejected() {
        let args = r#"{"receiver_id":"merchant.near","amount":"1"}"#;
        let actions = vec![make_ft_transfer_action(args)];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on amount mismatch");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("amount"),
                    "error message should mention amount, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn non_ft_transfer_method_is_rejected() {
        let actions = vec![make_func_call_action(
            "storage_deposit",
            r#"{"account_id":"attacker.near"}"#,
        )];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on non-ft_transfer method");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("ft_transfer"),
                    "error message should mention ft_transfer, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn non_function_call_action_is_rejected() {
        let transfer_action = Action::Transfer(TransferAction {
            deposit: NearToken::from_yoctonear(1_000_000),
        });
        let actions = vec![NonDelegateAction::try_from(transfer_action)
            .expect("Transfer is a valid NonDelegateAction")];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on non-FunctionCall action");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("FunctionCall"),
                    "error message should mention FunctionCall, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn malformed_args_json_is_rejected() {
        // Valid ft_transfer method but args are not valid JSON
        let actions = vec![make_func_call_action("ft_transfer", "not json at all")];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on malformed JSON args");
        match err {
            FacilitatorLocalError::DecodingError(msg) => {
                assert!(
                    msg.contains("parse ft_transfer args"),
                    "error message should mention parse, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn empty_actions_list_is_rejected() {
        let err = validate_delegate_actions_inner(&[], "merchant.near", "1000000")
            .expect_err("should fail on empty actions");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("no inner actions"),
                    "error message should mention no inner actions, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }

    #[test]
    fn mixed_actions_first_valid_second_invalid_is_rejected() {
        // Two ft_transfer actions: first is correct, second has wrong receiver.
        let good = r#"{"receiver_id":"merchant.near","amount":"1000000"}"#;
        let bad = r#"{"receiver_id":"attacker.near","amount":"1000000"}"#;
        let actions = vec![make_ft_transfer_action(good), make_ft_transfer_action(bad)];
        let err = validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect_err("should fail on second action's receiver_id mismatch");
        match err {
            FacilitatorLocalError::Other(msg) => {
                assert!(
                    msg.contains("receiver_id"),
                    "error should be about receiver_id, got: {msg}"
                );
            }
            other => panic!("unexpected error variant: {:?}", other),
        }
    }
}

/// `verify` against the payer's access key nonce.
///
/// Uses nothing newer than `verify` and the provider constructor, so the
/// module also runs against the code before the check. The RPC node is a local
/// JSON-RPC stub.
#[cfg(test)]
mod replay_verify_tests {
    use super::*;
    use crate::types::{ExactNearPayload, PaymentPayload, PaymentRequirements, TokenAmount};
    use axum::http::StatusCode;
    use axum::{routing::post, Json, Router};
    use near_crypto::KeyType;
    use near_primitives::action::delegate::DelegateAction;
    use serde_json::{json, Value};

    const USDC: &str = "usdc.testnet";
    const MERCHANT: &str = "merchant.testnet";
    const PAYER: &str = "payer.testnet";
    const AMOUNT: u64 = 1_000_000;
    pub(super) const DELEGATE_NONCE: u64 = 42;

    #[derive(Clone, Copy)]
    pub(super) enum Node {
        /// Answers view_access_key for PAYER with this nonce.
        AccessKeyNonce(u64),
        /// Answers every call with HTTP 503.
        Down,
        /// Never answers.
        Hangs,
    }

    pub(super) async fn near_stub(node: Node) -> String {
        let app = Router::new().route(
            "/",
            post(move |Json(req): Json<Value>| async move {
                match node {
                    Node::AccessKeyNonce(nonce) => {
                        let params = &req["params"];
                        let body = if params["request_type"] == "view_access_key"
                            && params["account_id"] == PAYER
                        {
                            json!({"jsonrpc": "2.0", "id": req["id"], "result": {
                                "nonce": nonce,
                                "permission": "FullAccess",
                                "block_height": 1,
                                "block_hash": "11111111111111111111111111111111",
                            }})
                        } else {
                            json!({"jsonrpc": "2.0", "id": req["id"], "error": {
                                "code": -32601, "message": format!("unexpected call {req}"),
                            }})
                        };
                        (StatusCode::OK, Json(body))
                    }
                    Node::Down => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))),
                    Node::Hangs => {
                        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
                        (StatusCode::OK, Json(json!({})))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    pub(super) async fn provider(node: Node) -> NearProvider {
        NearProvider::try_new(
            SecretKey::from_random(KeyType::ED25519),
            "facilitator.testnet".to_string(),
            near_stub(node).await,
            Network::NearTestnet,
        )
        .unwrap()
    }

    /// A USDC transfer delegate action signed by PAYER with DELEGATE_NONCE.
    pub(super) fn signed_delegate_action() -> SignedDelegateAction {
        let payer: Signer = InMemorySigner::from_secret_key(
            PAYER.parse().unwrap(),
            SecretKey::from_random(KeyType::ED25519),
        );
        let transfer = Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer".to_string(),
            args: json!({"receiver_id": MERCHANT, "amount": AMOUNT.to_string()})
                .to_string()
                .into_bytes(),
            gas: Gas::from_gas(30_000_000_000_000),
            deposit: NearToken::from_yoctonear(1),
        }));
        let delegate_action = DelegateAction {
            sender_id: PAYER.parse().unwrap(),
            receiver_id: USDC.parse().unwrap(),
            actions: vec![NonDelegateAction::try_from(transfer).unwrap()],
            nonce: DELEGATE_NONCE,
            max_block_height: 1_000_000,
            public_key: payer.public_key(),
        };
        let signature = payer.sign(delegate_action.get_nep461_hash().as_bytes());
        SignedDelegateAction {
            delegate_action,
            signature,
        }
    }

    fn request() -> VerifyRequest {
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            borsh::to_vec(&signed_delegate_action()).unwrap(),
        );
        VerifyRequest {
            x402_version: X402Version::V1,
            payment_payload: PaymentPayload {
                x402_version: X402Version::V1,
                scheme: Scheme::Exact,
                network: Network::NearTestnet,
                payload: ExactPaymentPayload::Near(ExactNearPayload {
                    signed_delegate_action: encoded,
                }),
            },
            payment_requirements: PaymentRequirements {
                scheme: Scheme::Exact,
                network: Network::NearTestnet,
                max_amount_required: TokenAmount(alloy::primitives::U256::from(AMOUNT)),
                resource: url::Url::parse("https://example.com/paid").unwrap(),
                description: String::new(),
                mime_type: "application/json".to_string(),
                output_schema: None,
                pay_to: MixedAddress::Near(MERCHANT.to_string()),
                max_timeout_seconds: 60,
                asset: MixedAddress::Near(USDC.to_string()),
                extra: None,
            },
        }
    }

    #[tokio::test]
    async fn verify_rejects_a_delegate_action_the_access_key_nonce_has_reached() {
        // Executing the delegate action set the access key nonce to its nonce.
        let provider = provider(Node::AccessKeyNonce(DELEGATE_NONCE)).await;

        let err = provider
            .verify(&request())
            .await
            .expect_err("an executed delegate action must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("already used")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_accepts_a_delegate_action_with_a_fresh_nonce() {
        let provider = provider(Node::AccessKeyNonce(DELEGATE_NONCE - 1)).await;

        let response = provider
            .verify(&request())
            .await
            .expect("a delegate action above the access key nonce verifies");
        assert!(
            matches!(&response, VerifyResponse::Valid { payer } if *payer == MixedAddress::Near(PAYER.to_string())),
            "got {response:?}"
        );
    }

    #[tokio::test]
    async fn verify_fails_closed_when_the_rpc_cannot_answer() {
        let provider = provider(Node::Down).await;

        let err = provider
            .verify(&request())
            .await
            .expect_err("an unanswered access key read must not vouch for a delegate action");
        assert!(
            matches!(&err, FacilitatorLocalError::ContractCall(_)),
            "got {err:?}"
        );
    }
}

#[cfg(test)]
mod replay_verify_bound_tests {
    use super::replay_verify_tests::{provider, signed_delegate_action, Node};
    use super::*;

    #[tokio::test]
    async fn access_key_read_gives_up_at_its_timeout() {
        let provider = provider(Node::Hangs).await;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.check_delegate_nonce_unused(
                &signed_delegate_action(),
                std::time::Duration::from_millis(200),
            ),
        )
        .await
        .expect("the check must return on its own timeout, not hang");
        assert!(
            matches!(&result, Err(FacilitatorLocalError::ContractCall(msg)) if msg.contains("timed out")),
            "got {result:?}"
        );
    }
}
