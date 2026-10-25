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
use near_primitives::action::delegate::{DelegateAction, NonDelegateAction, SignedDelegateAction};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Action, FunctionCallAction, Transaction, TransactionV0};
use near_primitives::types::{AccountId, BlockReference, Finality, Gas, Nonce};
use near_primitives::views::{ExecutionStatusView, FinalExecutionOutcomeView};
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

/// Most gas the inner `ft_transfer` of a delegate action may attach.
///
/// The relayer prepays the gas and the deposit a delegate action attaches
/// (NEP-366). Measured 2026-10-25 on the 20 meta-transactions in this
/// relayer's NEAR mainnet history (2025-12-04 to 2026-09-07): every one
/// carried a single `ft_transfer` with exactly 30 TGas -- the value the
/// TypeScript and Python SDKs and the 402milly client hard-code -- and its
/// receipt burnt at most 2.78 TGas. So the cap is what every known client
/// sends, not the protocol's 300 TGas.
const MAX_FT_TRANSFER_GAS: Gas = Gas::from_teragas(30);

/// The deposit `ft_transfer` takes: NEP-141 asserts exactly one yoctoNEAR.
const FT_TRANSFER_DEPOSIT: NearToken = NearToken::from_yoctonear(1);

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
/// - Exactly one action must be present.
/// - It must be a FunctionCall with method_name == "ft_transfer".
/// - Its JSON args must decode to { receiver_id, amount }.
/// - receiver_id must exactly equal expected_receiver.
/// - amount must exactly equal expected_amount (decimal string in atomic units).
/// - It must attach exactly [`FT_TRANSFER_DEPOSIT`] and at most
///   [`MAX_FT_TRANSFER_GAS`].
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

                if fc.deposit != FT_TRANSFER_DEPOSIT {
                    tracing::warn!(
                        index = i,
                        deposit = %fc.deposit.as_yoctonear(),
                        "Rejected DelegateAction: ft_transfer deposit is not 1 yoctoNEAR"
                    );
                    return Err(FacilitatorLocalError::Other(format!(
                        "DelegateAction action[{}] attaches {} yoctoNEAR; ft_transfer takes exactly {}",
                        i,
                        fc.deposit.as_yoctonear(),
                        FT_TRANSFER_DEPOSIT.as_yoctonear()
                    )));
                }

                if fc.gas > MAX_FT_TRANSFER_GAS {
                    tracing::warn!(
                        index = i,
                        gas = fc.gas.as_gas(),
                        "Rejected DelegateAction: ft_transfer attaches more gas than the cap"
                    );
                    return Err(FacilitatorLocalError::Other(format!(
                        "DelegateAction action[{}] attaches {} gas, above the {} gas cap",
                        i,
                        fc.gas.as_gas(),
                        MAX_FT_TRANSFER_GAS.as_gas()
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

    // After the loop, so an envelope with a bad action is refused for that action.
    if actions.len() != 1 {
        tracing::warn!(
            count = actions.len(),
            "Rejected DelegateAction: more than one inner action"
        );
        return Err(FacilitatorLocalError::Other(format!(
            "DelegateAction carries {} inner actions; exactly one ft_transfer is accepted",
            actions.len()
        )));
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

        // Only the network's allow-listed asset (USDC): settle registers the
        // recipient on this contract and reads the transfer back from its receipt.
        crate::chain::assert_supported_asset(
            self.network(),
            Some(payer.clone().into()),
            &requirements.asset,
        )?;

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

        let transfer = ExpectedTransfer {
            token: signed_delegate_action.delegate_action.receiver_id.clone(),
            payer: signed_delegate_action.delegate_action.sender_id.clone(),
            receiver: expected_receiver,
            amount: expected_amount,
        };

        Ok(VerifyPaymentResult {
            payer,
            signed_delegate_action,
            transfer,
        })
    }

    /// The payer-side reads verify and settle make: the delegate action's nonce
    /// is unused and the payer holds the amount.
    async fn check_payer_state(
        &self,
        verification: &VerifyPaymentResult,
        request: &VerifyRequest,
    ) -> Result<(), FacilitatorLocalError> {
        let timeout = crate::chain::rpc_http_timeout();
        self.check_delegate_nonce_unused(&verification.signed_delegate_action, timeout)
            .await?;
        self.check_payer_balance(
            &verification.signed_delegate_action.delegate_action,
            request.payment_requirements.max_amount_required.0,
            timeout,
        )
        .await
    }

    /// Refuse a payer whose token balance is below the amount.
    ///
    /// Reads `ft_balance_of` on the token the delegate action calls, at
    /// optimistic finality like the nonce check, so a payer funded a block ago
    /// is not refused. A balance that cannot be read fails closed. An account
    /// the token does not know reads `"0"` (NEP-141).
    async fn check_payer_balance(
        &self,
        delegate_action: &DelegateAction,
        amount: alloy::primitives::U256,
        timeout: std::time::Duration,
    ) -> Result<(), FacilitatorLocalError> {
        let args = serde_json::json!({ "account_id": delegate_action.sender_id.to_string() });
        let request = methods::query::RpcQueryRequest {
            block_reference: BlockReference::Finality(Finality::None),
            request: near_primitives::views::QueryRequest::CallFunction {
                account_id: delegate_action.receiver_id.clone(),
                method_name: "ft_balance_of".to_string(),
                args: near_primitives::types::FunctionArgs::from(args.to_string().into_bytes()),
            },
        };

        let response = match tokio::time::timeout(timeout, self.rpc_client.call(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                return Err(FacilitatorLocalError::ContractCall(format!(
                    "Failed to call ft_balance_of: {e}"
                )))
            }
            Err(_) => {
                return Err(FacilitatorLocalError::ContractCall(format!(
                    "ft_balance_of query timed out after {}ms",
                    timeout.as_millis()
                )))
            }
        };

        let balance = match response.kind {
            QueryResponseKind::CallResult(result) => parse_ft_balance(&result.result)?,
            _ => {
                return Err(FacilitatorLocalError::ContractCall(
                    "Unexpected query response kind for ft_balance_of".to_string(),
                ))
            }
        };

        if alloy::primitives::U256::from(balance) < amount {
            tracing::warn!(
                payer = %delegate_action.sender_id,
                balance = %balance,
                required = %amount,
                "Rejected NEAR payment: payer balance below the amount"
            );
            return Err(FacilitatorLocalError::InsufficientFunds(
                MixedAddress::Near(delegate_action.sender_id.to_string()),
            ));
        }

        Ok(())
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
    /// The relayer pays the gas fees. The hash comes back only once
    /// [`confirm_transfer`] has found `transfer` in the outcome.
    async fn submit_meta_transaction(
        &self,
        signed_delegate_action: SignedDelegateAction,
        transfer: &ExpectedTransfer,
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

        let outcome = self.broadcast(signed_tx).await?;
        confirm_transfer(&outcome, transfer, self.network())
    }

    /// `broadcast_tx_commit` of a signed meta-transaction, and its outcome.
    async fn broadcast(
        &self,
        signed_tx: near_primitives::transaction::SignedTransaction,
    ) -> Result<FinalExecutionOutcomeView, FacilitatorLocalError> {
        // The hash exists before the transaction leaves. A submission that
        // fails without the node refusing it may still execute, and then the
        // caller gets this hash to look up rather than a failure to retry.
        let tx_hash = signed_tx.get_hash();

        // Submit the transaction
        let request = methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest {
            signed_transaction: signed_tx,
        };

        let response = self.rpc_client.call(request).await.map_err(|e| {
            if submission_may_have_landed(&e) {
                tracing::warn!(
                    %tx_hash,
                    error = %crate::redact::scrub_urls(&e.to_string()),
                    "NEAR submission outcome unknown; the meta-transaction may still execute"
                );
                return FacilitatorLocalError::SettlementUnconfirmed(
                    TransactionHash::Near(tx_hash.0),
                    self.network(),
                );
            }
            FacilitatorLocalError::ContractCall(format!("Failed to submit meta-transaction: {e}"))
        })?;

        // Check for execution errors. Not enough on its own: see `confirm_transfer`.
        if let near_primitives::views::FinalExecutionStatus::Failure(err) = &response.status {
            return Err(FacilitatorLocalError::ContractCall(format!(
                "Meta-transaction failed: {:?}",
                err
            )));
        }

        tracing::info!(
            tx_hash = %response.transaction.hash,
            "NEP-366 meta-transaction submitted successfully"
        );

        Ok(response)
    }
}

/// Parse an `ft_balance_of` answer: a JSON string holding a decimal u128.
fn parse_ft_balance(result: &[u8]) -> Result<u128, FacilitatorLocalError> {
    serde_json::from_slice::<String>(result)
        .ok()
        .and_then(|balance| balance.parse::<u128>().ok())
        .ok_or_else(|| {
            FacilitatorLocalError::ContractCall(format!(
                "ft_balance_of returned {:?}, not a decimal balance",
                String::from_utf8_lossy(result)
            ))
        })
}

/// The token movement a settled meta-transaction has to show.
#[derive(Debug, Clone)]
struct ExpectedTransfer {
    /// The NEP-141 contract: the delegate action's receiver.
    token: AccountId,
    /// The delegate action's sender, the `old_owner_id` of the event.
    payer: AccountId,
    /// `pay_to`, the `new_owner_id` of the event.
    receiver: String,
    /// The amount in atomic units, as a decimal string.
    amount: String,
}

/// The NEP-297 envelope a NEP-141 contract logs after `EVENT_JSON:`.
#[derive(Deserialize)]
struct Nep297Event {
    standard: String,
    event: String,
    #[serde(default)]
    data: serde_json::Value,
}

/// One transfer of a NEP-141 `ft_transfer` event (`memo` is not compared).
#[derive(Deserialize)]
struct FtTransferEvent {
    old_owner_id: String,
    new_owner_id: String,
    amount: String,
}

/// Whether `logs` carry the NEP-141 `ft_transfer` event of `transfer`.
fn logs_transfer(logs: &[String], transfer: &ExpectedTransfer) -> bool {
    logs.iter()
        .filter_map(|log| log.strip_prefix("EVENT_JSON:"))
        .filter_map(|json| serde_json::from_str::<Nep297Event>(json).ok())
        .filter(|event| event.standard == "nep141" && event.event == "ft_transfer")
        .filter_map(|event| serde_json::from_value::<Vec<FtTransferEvent>>(event.data).ok())
        .flatten()
        .any(|moved| {
            moved.old_owner_id == transfer.payer.as_str()
                && moved.new_owner_id == transfer.receiver
                && moved.amount == transfer.amount
        })
}

/// Whether an executed meta-transaction actually moved the tokens.
///
/// The transaction's own status does not say. Its final status is the one of
/// the receipt that ran the Delegate action on the payer's account, and that
/// receipt succeeds once it has created the receipt with the inner actions.
/// The `ft_transfer` runs in that second receipt, on the token contract, and
/// its result exists only in `receipts_outcome`. So this follows the
/// transaction to the Delegate receipt and to the receipt it created on the
/// token, and takes the payment as made only when that receipt ended in
/// `SuccessValue` AND logged the NEP-141 `ft_transfer` event for exactly this
/// payer, recipient and amount. Only the token contract can log in a receipt
/// it executes, so the event cannot come from anywhere else.
///
/// - The receipt (or the Delegate one) failed: a verdict, nothing was paid
///   (`ContractCall`, which settle answers as `success: false`).
/// - Anything else that is not that payment -- the receipt has not executed,
///   the outcome does not contain it, it succeeded without the event, there
///   is more than one, it returned a promise: no verdict. Tokens may have
///   moved, so the hash travels as `SettlementUnconfirmed` for the caller to
///   look up instead of paying again.
///
/// `ft_transfer_call` is NOT accepted (validation admits `ft_transfer` only)
/// and would need more than this: its receipt returns a promise
/// (`SuccessReceiptId`), the receiver's `ft_on_transfer` may hand tokens back,
/// and the closing `ft_resolve_transfer` receipt answers `SuccessValue` with
/// the amount actually used, which is what a settlement would have to compare,
/// together with the `ft_transfer` event of any refund.
fn confirm_transfer(
    outcome: &FinalExecutionOutcomeView,
    transfer: &ExpectedTransfer,
    network: Network,
) -> Result<CryptoHash, FacilitatorLocalError> {
    let tx_hash = outcome.transaction.hash;
    let no_verdict = |why: &str| {
        tracing::warn!(%tx_hash, why, "NEAR meta-transaction executed, transfer not confirmed yet");
        FacilitatorLocalError::SettlementUnconfirmed(TransactionHash::Near(tx_hash.0), network)
    };
    let not_paid = |why: String| {
        FacilitatorLocalError::ContractCall(format!(
            "Meta-transaction {tx_hash} executed without paying: {why}"
        ))
    };
    let receipt = |id: &CryptoHash| outcome.receipts_outcome.iter().find(|r| r.id == *id);

    let delegate = match &outcome.transaction_outcome.outcome.status {
        ExecutionStatusView::SuccessReceiptId(id) => match receipt(id) {
            Some(delegate) => delegate,
            None => return Err(no_verdict("Delegate receipt missing from the outcome")),
        },
        _ => return Err(no_verdict("transaction not converted to a receipt")),
    };
    match &delegate.outcome.status {
        ExecutionStatusView::Failure(e) => {
            return Err(not_paid(format!("Delegate action failed: {e:?}")))
        }
        ExecutionStatusView::Unknown => return Err(no_verdict("Delegate receipt not executed")),
        ExecutionStatusView::SuccessValue(_) | ExecutionStatusView::SuccessReceiptId(_) => {}
    }

    let on_token: Vec<_> = delegate
        .outcome
        .receipt_ids
        .iter()
        .filter_map(receipt)
        .filter(|r| r.outcome.executor_id == transfer.token)
        .collect();
    let ft_transfer = match on_token.as_slice() {
        [] => {
            return Err(no_verdict(
                "no receipt on the token contract in the outcome",
            ))
        }
        [only] => only,
        _ => return Err(no_verdict("more than one receipt on the token contract")),
    };

    match &ft_transfer.outcome.status {
        ExecutionStatusView::SuccessValue(_)
            if logs_transfer(&ft_transfer.outcome.logs, transfer) =>
        {
            Ok(tx_hash)
        }
        ExecutionStatusView::SuccessValue(_) => Err(no_verdict(
            "ft_transfer succeeded without the NEP-141 event of this payment",
        )),
        ExecutionStatusView::Failure(e) => Err(not_paid(format!("ft_transfer failed: {e:?}"))),
        ExecutionStatusView::SuccessReceiptId(_) => Err(no_verdict(
            "ft_transfer returned a promise; only a plain ft_transfer settles",
        )),
        ExecutionStatusView::Unknown => Err(no_verdict("ft_transfer receipt not executed")),
    }
}

/// The settle answer for a submitted meta-transaction.
///
/// Submitted with no verdict is not a failed settlement: reported as
/// `success: false` with no transaction, it tells the caller the payment did
/// not happen. It travels as an error so `IntoResponse` answers
/// `502 settlement_unconfirmed` with the hash.
fn settle_outcome(
    submitted: Result<CryptoHash, FacilitatorLocalError>,
    payer: MixedAddress,
    network: Network,
) -> Result<SettleResponse, FacilitatorLocalError> {
    match submitted {
        Ok(tx_hash) => Ok(SettleResponse {
            success: true,
            error_reason: None,
            payer,
            transaction: Some(TransactionHash::Near(tx_hash.0)),
            network,
            proof_of_payment: None, // ERC-8004 not supported on NEAR
            extensions: None,
        }),
        Err(e @ FacilitatorLocalError::SettlementUnconfirmed(..)) => {
            tracing::error!(error = %e, "NEAR settle: meta-transaction submitted, no verdict");
            Err(e)
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to submit NEAR meta-transaction");
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

/// Whether a failed `broadcast_tx_commit` may still have executed.
///
/// The node refused it when it answered that the transaction is invalid, that
/// it cannot judge it (`DoesNotTrackShard`) or that the request itself was
/// malformed, and a provider that answered 401/429/400 never forwarded it. A
/// request never serialized or never connected did not leave. Everything else
/// -- the node's own timeout (the transaction is executing), a routed request,
/// a node at its limits, a lost or unreadable answer, a gateway 5xx -- can end
/// executed.
fn submission_may_have_landed(
    error: &near_jsonrpc_client::errors::JsonRpcError<
        methods::broadcast_tx_commit::RpcTransactionError,
    >,
) -> bool {
    use methods::broadcast_tx_commit::RpcTransactionError;
    use near_jsonrpc_client::errors::{
        JsonRpcError, JsonRpcServerError, JsonRpcServerResponseStatusError,
        JsonRpcTransportSendError, RpcTransportError,
    };
    match error {
        JsonRpcError::TransportError(RpcTransportError::SendError(send)) => match send {
            JsonRpcTransportSendError::PayloadSerializeError(_) => false,
            JsonRpcTransportSendError::PayloadSendError(e) => !(e.is_connect() || e.is_builder()),
        },
        JsonRpcError::TransportError(RpcTransportError::RecvError(_)) => true,
        JsonRpcError::ServerError(server) => match server {
            JsonRpcServerError::HandlerError(
                RpcTransactionError::InvalidTransaction { .. }
                | RpcTransactionError::DoesNotTrackShard,
            ) => false,
            JsonRpcServerError::HandlerError(_) => true,
            JsonRpcServerError::RequestValidationError(_) => false,
            JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::Unauthorized
                | JsonRpcServerResponseStatusError::TooManyRequests
                | JsonRpcServerResponseStatusError::BadRequest,
            ) => false,
            JsonRpcServerError::ResponseStatusError(_)
            | JsonRpcServerError::InternalError { .. }
            | JsonRpcServerError::NonContextualError(_) => true,
        },
    }
}

/// Result of verifying a NEAR payment
pub struct VerifyPaymentResult {
    pub payer: NearAddress,
    pub signed_delegate_action: SignedDelegateAction,
    transfer: ExpectedTransfer,
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
        self.check_payer_state(&verification, request).await?;
        Ok(VerifyResponse::valid(verification.payer.into()))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let verification = self.verify_payment(request).await?;

        // The same reads as verify, before the relayer spends anything on this
        // payment (a registration, the gas). The transfer itself is still
        // confirmed in the outcome: these only make settle cheap to refuse.
        self.check_payer_state(&verification, request).await?;

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
        let submitted = self
            .submit_meta_transaction(verification.signed_delegate_action, &verification.transfer)
            .await;
        settle_outcome(submitted, verification.payer.into(), self.network())
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = vec![SupportedPaymentKind {
            network: self.network().to_string(),
            scheme: Scheme::Exact,
            x402_version: X402Version::V1,
            network_aliases: None,
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: Some(crate::network::exact_payment_tokens(self.network())),
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

    const GOOD_ARGS: &str = r#"{"receiver_id":"merchant.near","amount":"1000000"}"#;

    /// A ft_transfer with these args, gas and deposit.
    fn ft_transfer_with(gas: Gas, deposit: NearToken) -> NonDelegateAction {
        NonDelegateAction::try_from(Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "ft_transfer".to_string(),
            args: GOOD_ARGS.as_bytes().to_vec(),
            gas,
            deposit,
        })))
        .expect("FunctionCall is a valid NonDelegateAction")
    }

    fn rejection(actions: &[NonDelegateAction]) -> String {
        match validate_delegate_actions_inner(actions, "merchant.near", "1000000") {
            Err(FacilitatorLocalError::Other(msg)) => msg,
            other => panic!("expected an Other rejection, got {other:?}"),
        }
    }

    #[test]
    fn two_valid_ft_transfers_are_rejected() {
        // Each one would pass alone; together they are not the payment.
        let actions = vec![
            make_ft_transfer_action(GOOD_ARGS),
            make_ft_transfer_action(GOOD_ARGS),
        ];
        let msg = rejection(&actions);
        assert!(msg.contains("exactly one"), "got: {msg}");
    }

    #[test]
    fn a_deposit_other_than_one_yocto_is_rejected() {
        for deposit in [
            NearToken::from_yoctonear(0),
            NearToken::from_yoctonear(2),
            NearToken::from_near(1),
        ] {
            let actions = vec![ft_transfer_with(MAX_FT_TRANSFER_GAS, deposit)];
            let msg = rejection(&actions);
            assert!(msg.contains("yoctoNEAR"), "{deposit}: got {msg}");
        }
    }

    #[test]
    fn gas_above_the_cap_is_rejected() {
        for gas in [
            Gas::from_gas(MAX_FT_TRANSFER_GAS.as_gas() + 1),
            Gas::from_teragas(300),
        ] {
            let actions = vec![ft_transfer_with(gas, FT_TRANSFER_DEPOSIT)];
            let msg = rejection(&actions);
            assert!(msg.contains("gas cap"), "{gas}: got {msg}");
        }
    }

    #[test]
    fn the_gas_every_client_attaches_passes() {
        // 30 TGas and one yoctoNEAR: what the SDKs send, and the cap itself.
        let actions = vec![ft_transfer_with(
            Gas::from_teragas(30),
            NearToken::from_yoctonear(1),
        )];
        validate_delegate_actions_inner(&actions, "merchant.near", "1000000")
            .expect("the SDKs' ft_transfer is the payment");
    }

    #[test]
    fn the_only_near_asset_is_the_network_usdc() {
        // `verify_payment` asks `assert_supported_asset`; on NEAR that list must
        // stay USDC alone (settle registers recipients on it and reads it back).
        for (network, usdc) in [
            (
                Network::Near,
                "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
            ),
            (
                Network::NearTestnet,
                "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
            ),
        ] {
            assert_eq!(
                crate::network::supported_asset_addresses(network),
                vec![MixedAddress::Near(usdc.to_string())],
                "{network}"
            );
        }
    }

    #[test]
    fn an_ft_balance_is_a_decimal_string() {
        assert_eq!(parse_ft_balance(br#""1000000""#).unwrap(), 1_000_000);
        assert_eq!(parse_ft_balance(br#""0""#).unwrap(), 0);
        let malformed: [&[u8]; 5] = [b"1000000", br#""-1""#, br#""1e6""#, b"null", b""];
        for answer in malformed {
            assert!(
                matches!(
                    parse_ft_balance(answer),
                    Err(FacilitatorLocalError::ContractCall(_))
                ),
                "{:?}",
                String::from_utf8_lossy(answer)
            );
        }
    }
}

/// `confirm_transfer` against outcomes of meta-transactions this relayer
/// settled on NEAR mainnet, read back read-only on 2026-10-25 with the `tx`
/// method of the public archival RPC and stored in
/// `tests/fixtures/near/<hash>.json`. Same type `broadcast_tx_commit` returns,
/// at `FINAL` (with the refund receipts, which `broadcast_tx_commit` may not
/// wait for). The failure cases are the same outcomes with one receipt edited.
#[cfg(test)]
mod transfer_outcome_tests {
    use super::*;
    use serde_json::{json, Value};

    const USDC_MAINNET: &str = "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1";

    /// uvd-buyer.near -> 402milly.near, 1 USDC, with a memo.
    const NAMED: &str =
        include_str!("../../tests/fixtures/near/BY1QXFFLoojaNGr4bzqSW6Cmu1wc2G7sPLcHB6wXsSqM.json");
    /// Implicit account to implicit account, 0.01 USDC, two refund receipts.
    const IMPLICIT: &str =
        include_str!("../../tests/fixtures/near/7r3qYWXUgzv2v7QACsXNDYMiU4qiZf4qLRAY4pUMrDhd.json");

    fn named() -> (Value, ExpectedTransfer) {
        (
            serde_json::from_str(NAMED).unwrap(),
            ExpectedTransfer {
                token: USDC_MAINNET.parse().unwrap(),
                payer: "uvd-buyer.near".parse().unwrap(),
                receiver: "402milly.near".to_string(),
                amount: "1000000".to_string(),
            },
        )
    }

    fn implicit() -> (Value, ExpectedTransfer) {
        (
            serde_json::from_str(IMPLICIT).unwrap(),
            ExpectedTransfer {
                token: USDC_MAINNET.parse().unwrap(),
                payer: "aa5c387e7be96bcfd03676f983f38a03453dda0867c38329adeb01f8c356baf6"
                    .parse()
                    .unwrap(),
                receiver: "ea178ea75470f542eef239a1ba25f0b11f43b63ef59f3e2a18789913b6d84cc5"
                    .to_string(),
                amount: "10000".to_string(),
            },
        )
    }

    fn view(outcome: &Value) -> FinalExecutionOutcomeView {
        serde_json::from_value(outcome.clone()).expect("a broadcast_tx_commit outcome")
    }

    /// The outcome of the receipt executed by `executor`.
    fn receipt_of<'a>(outcome: &'a mut Value, executor: &str) -> &'a mut Value {
        outcome["receipts_outcome"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .map(|receipt| &mut receipt["outcome"])
            .find(|o| o["executor_id"] == executor)
            .unwrap_or_else(|| panic!("no receipt executed by {executor}"))
    }

    /// What `ft_transfer` answers when NEP-141 refuses it.
    fn panicked(message: &str) -> Value {
        json!({"Failure": {"ActionError": {"index": 0, "kind": {
            "FunctionCallError": {"ExecutionError": format!("Smart contract panicked: {message}")}
        }}}})
    }

    fn not_paid(outcome: &Value, transfer: &ExpectedTransfer) -> String {
        match confirm_transfer(&view(outcome), transfer, Network::Near) {
            Err(FacilitatorLocalError::ContractCall(msg)) => msg,
            other => panic!("expected a not-paid verdict, got {other:?}"),
        }
    }

    fn no_verdict(outcome: &Value, transfer: &ExpectedTransfer) {
        let hash: CryptoHash = outcome["transaction"]["hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        match confirm_transfer(&view(outcome), transfer, Network::Near) {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                assert_eq!(tx, TransactionHash::Near(hash.0));
                assert_eq!(network, Network::Near);
            }
            other => panic!("expected no verdict, got {other:?}"),
        }
    }

    #[test]
    fn recorded_settlements_are_paid() {
        for (outcome, transfer) in [named(), implicit()] {
            let hash: CryptoHash = outcome["transaction"]["hash"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                confirm_transfer(&view(&outcome), &transfer, Network::Near).unwrap(),
                hash
            );
        }
    }

    #[test]
    fn an_inner_failure_under_a_successful_transaction_is_not_paid() {
        for (mut outcome, transfer) in [named(), implicit()] {
            let ft = receipt_of(&mut outcome, USDC_MAINNET);
            ft["status"] = panicked("The account doesn't have enough balance");
            ft["logs"] = json!([]);
            // The transaction itself still reads as a success.
            assert!(matches!(
                view(&outcome).status,
                near_primitives::views::FinalExecutionStatus::SuccessValue(_)
            ));
            let msg = not_paid(&outcome, &transfer);
            assert!(msg.contains("ft_transfer failed"), "got: {msg}");
        }
    }

    #[test]
    fn a_failed_receipt_is_not_paid_even_with_the_event_logged() {
        // A contract that panics keeps the logs it wrote before the panic.
        for (mut outcome, transfer) in [named(), implicit()] {
            receipt_of(&mut outcome, USDC_MAINNET)["status"] = panicked("after the event");
            let msg = not_paid(&outcome, &transfer);
            assert!(msg.contains("ft_transfer failed"), "got: {msg}");
        }
    }

    /// A copy of the recorded token receipt under `id`, with `status` and no logs.
    fn another_token_receipt(outcome: &Value, id: &str, status: Value) -> Value {
        let mut copy = outcome["receipts_outcome"][1].clone();
        assert_eq!(copy["outcome"]["executor_id"], USDC_MAINNET);
        copy["id"] = json!(id);
        copy["outcome"]["status"] = status;
        copy["outcome"]["logs"] = json!([]);
        copy
    }

    #[test]
    fn two_receipts_on_the_token_are_not_one_transfer() {
        // The recorded transfer plus a second, failed receipt the Delegate created.
        let (mut outcome, transfer) = named();
        let id = "11111111111111111111111111111112";
        let second = another_token_receipt(&outcome, id, panicked("second call"));
        outcome["receipts_outcome"]
            .as_array_mut()
            .unwrap()
            .push(second);
        receipt_of(&mut outcome, "uvd-buyer.near")["receipt_ids"]
            .as_array_mut()
            .unwrap()
            .push(json!(id));
        no_verdict(&outcome, &transfer);
    }

    #[test]
    fn a_token_receipt_the_delegate_did_not_create_does_not_count() {
        // The Delegate's own receipt failed; a token receipt elsewhere in the
        // outcome (child of the refund) carries the event.
        let (mut outcome, transfer) = named();
        let ft = receipt_of(&mut outcome, USDC_MAINNET);
        let log = ft["logs"].clone();
        ft["status"] = panicked("The account doesn't have enough balance");
        ft["logs"] = json!([]);
        let id = "11111111111111111111111111111113";
        let mut elsewhere = another_token_receipt(&outcome, id, json!({"SuccessValue": ""}));
        elsewhere["outcome"]["logs"] = log;
        outcome["receipts_outcome"]
            .as_array_mut()
            .unwrap()
            .push(elsewhere);
        receipt_of(&mut outcome, "uvd-facilitator.near")["receipt_ids"] = json!([id]);
        let msg = not_paid(&outcome, &transfer);
        assert!(msg.contains("ft_transfer failed"), "got: {msg}");
    }

    #[test]
    fn refund_receipts_missing_from_the_outcome_do_not_matter() {
        // broadcast_tx_commit waits for every receipt but the refunds, so
        // those may be absent (the recorded outcomes are FINAL, with them).
        let (mut outcome, transfer) = implicit();
        outcome["receipts_outcome"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["outcome"]["executor_id"] != "uvd-facilitator.near");
        assert_eq!(outcome["receipts_outcome"].as_array().unwrap().len(), 2);
        confirm_transfer(&view(&outcome), &transfer, Network::Near)
            .expect("the transfer is there without its refunds");
    }

    #[test]
    fn a_delegate_receipt_not_executed_is_no_verdict() {
        let (mut outcome, transfer) = named();
        receipt_of(&mut outcome, "uvd-buyer.near")["status"] = json!("Unknown");
        no_verdict(&outcome, &transfer);
    }

    #[test]
    fn a_transaction_not_converted_to_a_receipt_is_no_verdict() {
        let (mut outcome, transfer) = named();
        outcome["transaction_outcome"]["outcome"]["status"] = json!("Unknown");
        no_verdict(&outcome, &transfer);
    }

    #[test]
    fn a_success_without_the_event_is_no_verdict() {
        for (mut outcome, transfer) in [named(), implicit()] {
            receipt_of(&mut outcome, USDC_MAINNET)["logs"] = json!([]);
            no_verdict(&outcome, &transfer);
        }
    }

    #[test]
    fn an_event_for_another_transfer_is_no_verdict() {
        let (outcome, transfer) = named();
        for (field, value) in [
            ("payer", "someone-else.near"),
            ("receiver", "attacker.near"),
            ("amount", "999999"),
        ] {
            let mut other = transfer.clone();
            match field {
                "payer" => other.payer = value.parse().unwrap(),
                "receiver" => other.receiver = value.to_string(),
                _ => other.amount = value.to_string(),
            }
            no_verdict(&outcome, &other);
        }
    }

    #[test]
    fn an_event_that_is_not_a_nep141_transfer_is_no_verdict() {
        let (outcome, transfer) = named();
        let log = receipt_of(&mut outcome.clone(), USDC_MAINNET)["logs"][0]
            .as_str()
            .unwrap()
            .to_string();
        for edited in [
            log.replace("\"standard\":\"nep141\"", "\"standard\":\"nep171\""),
            log.replace("\"event\":\"ft_transfer\"", "\"event\":\"ft_mint\""),
            log.replace("EVENT_JSON:", "EVENT_JSON "),
            log.replace("EVENT_JSON:", ""),
        ] {
            let mut outcome = outcome.clone();
            receipt_of(&mut outcome, USDC_MAINNET)["logs"] = json!([edited]);
            no_verdict(&outcome, &transfer);
        }
    }

    #[test]
    fn an_event_logged_by_another_account_does_not_count() {
        // The event moved to the relayer's refund receipt.
        let (mut outcome, transfer) = named();
        let log = receipt_of(&mut outcome, USDC_MAINNET)["logs"][0].clone();
        receipt_of(&mut outcome, USDC_MAINNET)["logs"] = json!([]);
        receipt_of(&mut outcome, "uvd-facilitator.near")["logs"] = json!([log]);
        no_verdict(&outcome, &transfer);
    }

    #[test]
    fn a_receipt_on_another_contract_is_not_the_transfer() {
        // The same transfer, run by a contract that is not the token.
        let (mut outcome, transfer) = named();
        receipt_of(&mut outcome, USDC_MAINNET)["executor_id"] = json!("usdt.tether-token.near");
        no_verdict(&outcome, &transfer);
    }

    #[test]
    fn an_unexecuted_transfer_is_no_verdict() {
        for (mut outcome, transfer) in [named(), implicit()] {
            let ft = receipt_of(&mut outcome, USDC_MAINNET);
            ft["status"] = json!("Unknown");
            ft["logs"] = json!([]);
            no_verdict(&outcome, &transfer);
        }
    }

    #[test]
    fn a_transfer_missing_from_the_outcome_is_no_verdict() {
        for (mut outcome, transfer) in [named(), implicit()] {
            outcome["receipts_outcome"]
                .as_array_mut()
                .unwrap()
                .retain(|r| r["outcome"]["executor_id"] != USDC_MAINNET);
            no_verdict(&outcome, &transfer);
        }
    }

    #[test]
    fn a_failed_delegate_action_is_not_paid() {
        let (mut outcome, transfer) = named();
        receipt_of(&mut outcome, "uvd-buyer.near")["status"] = json!({"Failure": {"ActionError": {
            "index": 0, "kind": "DelegateActionExpired"
        }}});
        let msg = not_paid(&outcome, &transfer);
        assert!(msg.contains("Delegate action failed"), "got: {msg}");
    }

    #[test]
    fn a_promise_from_the_token_is_no_verdict() {
        // What ft_transfer_call answers first; it is not accepted yet.
        let (mut outcome, transfer) = named();
        let refund = outcome["receipts_outcome"][2]["id"].clone();
        receipt_of(&mut outcome, USDC_MAINNET)["status"] = json!({ "SuccessReceiptId": refund });
        no_verdict(&outcome, &transfer);
    }
}

/// `verify` against the payer's access key nonce and token balance.
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

    /// The testnet USDC contract, the one token NEAR payments may move.
    pub(super) const USDC: &str =
        "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af";
    pub(super) const MERCHANT: &str = "merchant.testnet";
    pub(super) const PAYER: &str = "payer.testnet";
    pub(super) const AMOUNT: u64 = 1_000_000;
    pub(super) const DELEGATE_NONCE: u64 = 42;

    #[derive(Clone, Copy)]
    pub(super) enum Node {
        /// Answers view_access_key for PAYER with this nonce, and ft_balance_of
        /// for PAYER on USDC with exactly AMOUNT.
        AccessKeyNonce(u64),
        /// Same, with `balance` as the raw ft_balance_of answer (`None`: an error).
        Funded {
            nonce: u64,
            balance: Option<&'static str>,
        },
        /// Answers every call with HTTP 503.
        Down,
        /// Never answers.
        Hangs,
    }

    /// `"1000000"`: what ft_balance_of answers for a payer holding AMOUNT.
    const EXACTLY_AMOUNT: &str = r#""1000000""#;

    /// The key PAYER signs with. The stub answers only for this key, so a read
    /// of any other access key (the relayer's, say) fails the tests.
    static PAYER_KEY: once_cell::sync::Lazy<SecretKey> =
        once_cell::sync::Lazy::new(|| SecretKey::from_random(KeyType::ED25519));

    pub(super) async fn near_stub(node: Node) -> String {
        let app = Router::new().route(
            "/",
            post(move |Json(req): Json<Value>| async move {
                let (nonce, balance) = match node {
                    Node::AccessKeyNonce(nonce) => (nonce, Some(EXACTLY_AMOUNT)),
                    Node::Funded { nonce, balance } => (nonce, balance),
                    Node::Down => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))),
                    Node::Hangs => {
                        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
                        return (StatusCode::OK, Json(json!({})));
                    }
                };
                let params = &req["params"];
                let balance_of_payer = params["request_type"] == "call_function"
                    && params["account_id"] == USDC
                    && params["method_name"] == "ft_balance_of"
                    && params["args_base64"].as_str().and_then(|args| {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, args)
                            .ok()
                    }) == Some(json!({ "account_id": PAYER }).to_string().into_bytes());
                let body = if params["request_type"] == "view_access_key"
                    && params["account_id"] == PAYER
                    && params["public_key"] == PAYER_KEY.public_key().to_string()
                {
                    json!({"jsonrpc": "2.0", "id": req["id"], "result": {
                        "nonce": nonce,
                        "permission": "FullAccess",
                        "block_height": 1,
                        "block_hash": "11111111111111111111111111111111",
                    }})
                } else if let (true, Some(balance)) = (balance_of_payer, balance) {
                    json!({"jsonrpc": "2.0", "id": req["id"], "result": {
                        "result": balance.as_bytes(),
                        "logs": [],
                        "block_height": 1,
                        "block_hash": "11111111111111111111111111111111",
                    }})
                } else {
                    json!({"jsonrpc": "2.0", "id": req["id"], "error": {
                        "code": -32601, "message": format!("unexpected call {req}"),
                    }})
                };
                (StatusCode::OK, Json(body))
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
        signed_delegate_action_to(USDC)
    }

    /// The same transfer, calling `token` instead of USDC.
    fn signed_delegate_action_to(token: &str) -> SignedDelegateAction {
        let payer: Signer =
            InMemorySigner::from_secret_key(PAYER.parse().unwrap(), PAYER_KEY.clone());
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
            receiver_id: token.parse().unwrap(),
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

    pub(super) fn request() -> VerifyRequest {
        request_paying(USDC)
    }

    /// AMOUNT of `token` to MERCHANT, with a delegate action that calls `token`.
    fn request_paying(token: &str) -> VerifyRequest {
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            borsh::to_vec(&signed_delegate_action_to(token)).unwrap(),
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
                asset: MixedAddress::Near(token.to_string()),
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

    // `verify_accepts_a_delegate_action_with_a_fresh_nonce` is also the
    // boundary of the balance check: its node holds exactly AMOUNT.

    #[tokio::test]
    async fn verify_rejects_a_payer_holding_less_than_the_amount() {
        let provider = provider(Node::Funded {
            nonce: DELEGATE_NONCE - 1,
            balance: Some(r#""999999""#),
        })
        .await;

        let err = provider
            .verify(&request())
            .await
            .expect_err("a payer short of the amount must not verify");
        assert!(
            matches!(&err, FacilitatorLocalError::InsufficientFunds(payer) if *payer == MixedAddress::Near(PAYER.to_string())),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_fails_closed_when_the_balance_cannot_be_read() {
        for balance in [None, Some("null"), Some(r#""lots""#)] {
            let provider = provider(Node::Funded {
                nonce: DELEGATE_NONCE - 1,
                balance,
            })
            .await;

            let err = provider
                .verify(&request())
                .await
                .expect_err("an unread balance must not vouch for a payer");
            assert!(
                matches!(&err, FacilitatorLocalError::ContractCall(_)),
                "{balance:?}: got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn verify_rejects_a_token_that_is_not_the_network_usdc() {
        // Requirements and delegate action agree on the token; it is not USDC.
        let provider = provider(Node::AccessKeyNonce(DELEGATE_NONCE - 1)).await;

        for token in ["usdc.testnet", "usdt.tether-token.testnet"] {
            let err = provider
                .verify(&request_paying(token))
                .await
                .expect_err("only the network's USDC is a NEAR payment");
            assert!(
                matches!(
                    &err,
                    FacilitatorLocalError::UnsupportedAsset(Some(payer), Network::NearTestnet, asset)
                        if *payer == MixedAddress::Near(PAYER.to_string()) && asset == token
                ),
                "{token}: got {err:?}"
            );
        }
    }
}

/// `settle` end to end against a stub node: the payment is the transfer the
/// `broadcast_tx_commit` outcome shows, not the transaction's own status.
#[cfg(test)]
mod settle_confirms_transfer_tests {
    use super::replay_verify_tests::{request, AMOUNT, DELEGATE_NONCE, MERCHANT, PAYER, USDC};
    use super::*;
    use axum::{routing::post, Json, Router};
    use near_crypto::KeyType;
    use near_primitives::views::{BlockHeaderView, BlockView};
    use serde_json::{json, Value};

    const USDC_MAINNET: &str = "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1";
    const RECORDED: &str =
        include_str!("../../tests/fixtures/near/BY1QXFFLoojaNGr4bzqSW6Cmu1wc2G7sPLcHB6wXsSqM.json");

    /// The recorded mainnet settlement, retold between this module's accounts.
    fn recorded() -> Value {
        assert_eq!(AMOUNT, 1_000_000, "the recorded transfer moved 1000000");
        let retold = RECORDED
            .replace(USDC_MAINNET, USDC)
            .replace("uvd-buyer.near", PAYER)
            .replace("402milly.near", MERCHANT);
        serde_json::from_str(&retold).unwrap()
    }

    fn recorded_hash() -> CryptoHash {
        recorded()["transaction"]["hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// The outcome of the receipt the token contract executed.
    fn token_receipt(outcome: &mut Value) -> &mut Value {
        outcome["receipts_outcome"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .map(|receipt| &mut receipt["outcome"])
            .find(|o| o["executor_id"] == USDC)
            .expect("the recorded outcome has a token receipt")
    }

    /// What the stub node answers.
    #[derive(Clone)]
    struct Chain {
        /// The `broadcast_tx_commit` answer.
        outcome: Value,
        /// PAYER's access key nonce (the relayer's is 7).
        payer_nonce: u64,
        /// The raw `ft_balance_of` answer for PAYER.
        payer_balance: &'static str,
        /// Whether `storage_balance_of` finds MERCHANT on USDC.
        merchant_registered: bool,
    }

    /// The recorded transfer, PAYER funded with exactly AMOUNT and a fresh
    /// nonce, MERCHANT registered.
    fn chain(outcome: Value) -> Chain {
        Chain {
            outcome,
            payer_nonce: DELEGATE_NONCE - 1,
            payer_balance: r#""1000000""#,
            merchant_registered: true,
        }
    }

    /// A provider on a stub node, and the count of `broadcast_tx_commit`
    /// calls it got: every transaction the relayer would pay for (the
    /// meta-transaction and a `storage_deposit` alike).
    async fn provider(chain: Chain) -> (NearProvider, Arc<std::sync::atomic::AtomicUsize>) {
        let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = sent.clone();
        let block = serde_json::to_value(BlockView {
            author: "validator.testnet".parse().unwrap(),
            header: BlockHeaderView::default(),
            chunks: vec![],
        })
        .unwrap();
        let app = Router::new().route(
            "/",
            post(move |Json(req): Json<Value>| {
                let (chain, block, counter) = (chain.clone(), block.clone(), counter.clone());
                async move {
                    let params = &req["params"];
                    let view = |result: Vec<u8>| {
                        json!({
                            "result": result,
                            "logs": [],
                            "block_height": 1,
                            "block_hash": "11111111111111111111111111111111",
                        })
                    };
                    let result = match req["method"].as_str() {
                        Some("broadcast_tx_commit") => {
                            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            chain.outcome
                        }
                        Some("block") => block,
                        Some("query") if params["request_type"] == "view_access_key" => {
                            let nonce = if params["account_id"] == PAYER {
                                chain.payer_nonce
                            } else {
                                7
                            };
                            json!({
                                "nonce": nonce,
                                "permission": "FullAccess",
                                "block_height": 1,
                                "block_hash": "11111111111111111111111111111111",
                            })
                        }
                        Some("query")
                            if params["request_type"] == "call_function"
                                && params["account_id"] == USDC
                                && params["method_name"] == "storage_balance_of" =>
                        {
                            view(if chain.merchant_registered {
                                br#"{"total":"1250000000000000000000","available":"0"}"#.to_vec()
                            } else {
                                b"null".to_vec()
                            })
                        }
                        Some("query")
                            if params["request_type"] == "call_function"
                                && params["account_id"] == USDC
                                && params["method_name"] == "ft_balance_of" =>
                        {
                            view(chain.payer_balance.as_bytes().to_vec())
                        }
                        _ => {
                            return Json(json!({"jsonrpc": "2.0", "id": req["id"], "error": {
                                "code": -32601, "message": format!("unexpected call {req}"),
                            }}))
                        }
                    };
                    Json(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = NearProvider::try_new(
            SecretKey::from_random(KeyType::ED25519),
            "facilitator.testnet".to_string(),
            format!("http://{addr}"),
            Network::NearTestnet,
        )
        .unwrap();
        (provider, sent)
    }

    fn sent(counter: &std::sync::atomic::AtomicUsize) -> usize {
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn settle_refuses_a_payer_short_of_the_amount_before_spending() {
        // MERCHANT unregistered: a registration would be a transaction too.
        let (provider, counter) = provider(Chain {
            payer_balance: r#""999999""#,
            merchant_registered: false,
            ..chain(recorded())
        })
        .await;

        let err = provider
            .settle(&request())
            .await
            .expect_err("a payer short of the amount is refused");
        assert!(
            matches!(&err, FacilitatorLocalError::InsufficientFunds(payer) if *payer == MixedAddress::Near(PAYER.to_string())),
            "got {err:?}"
        );
        assert_eq!(sent(&counter), 0, "nothing sent for a refused payer");
    }

    #[tokio::test]
    async fn settle_refuses_a_used_delegate_nonce_before_spending() {
        let (provider, counter) = provider(Chain {
            payer_nonce: DELEGATE_NONCE,
            merchant_registered: false,
            ..chain(recorded())
        })
        .await;

        let err = provider
            .settle(&request())
            .await
            .expect_err("an executed delegate action is refused");
        assert!(
            matches!(&err, FacilitatorLocalError::Other(msg) if msg.contains("already used")),
            "got {err:?}"
        );
        assert_eq!(sent(&counter), 0, "nothing sent for a used nonce");
    }

    #[tokio::test]
    async fn settle_pays_when_the_outcome_shows_the_transfer() {
        let (provider, counter) = provider(chain(recorded())).await;

        let response = provider.settle(&request()).await.expect("a settle verdict");
        assert!(response.success, "got {response:?}");
        assert_eq!(
            response.transaction,
            Some(TransactionHash::Near(recorded_hash().0))
        );
        assert_eq!(sent(&counter), 1, "one meta-transaction, no registration");
    }

    #[tokio::test]
    async fn settle_does_not_pay_an_inner_failure_under_a_successful_transaction() {
        let mut outcome = recorded();
        let ft = token_receipt(&mut outcome);
        ft["status"] = json!({"Failure": {"ActionError": {"index": 0, "kind": {
            "FunctionCallError": {"ExecutionError": "Smart contract panicked: The account doesn't have enough balance"}
        }}}});
        ft["logs"] = json!([]);
        let (provider, _) = provider(chain(outcome)).await;

        let response = provider.settle(&request()).await.expect("a settle verdict");
        assert!(!response.success, "got {response:?}");
        assert!(response.transaction.is_none(), "got {response:?}");
        assert!(
            matches!(
                response.error_reason,
                Some(FacilitatorErrorReason::UnexpectedSettleError)
            ),
            "got {response:?}"
        );
    }

    #[tokio::test]
    async fn settle_without_the_transfer_outcome_is_unconfirmed() {
        let mut outcome = recorded();
        token_receipt(&mut outcome)["status"] = json!("Unknown");
        let (provider, _) = provider(chain(outcome)).await;

        match provider.settle(&request()).await {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)) => {
                assert_eq!(tx, TransactionHash::Near(recorded_hash().0));
                assert_eq!(network, Network::NearTestnet);
            }
            other => panic!("expected the unconfirmed error, got {other:?}"),
        }
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

    #[tokio::test]
    async fn balance_read_gives_up_at_its_timeout() {
        let provider = provider(Node::Hangs).await;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.check_payer_balance(
                &signed_delegate_action().delegate_action,
                alloy::primitives::U256::from(1u64),
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

/// Which failed submissions may still execute. Only a refusal proves it did
/// not; the node's own timeout means the transaction is executing.
#[cfg(test)]
mod submission_outcome_tests {
    use super::*;
    use methods::broadcast_tx_commit::RpcTransactionError;
    use near_jsonrpc_client::errors::{
        JsonRpcError, JsonRpcServerError, JsonRpcServerResponseStatusError,
    };
    use near_jsonrpc_primitives::errors::RpcRequestValidationErrorKind;

    fn server(error: JsonRpcServerError<RpcTransactionError>) -> JsonRpcError<RpcTransactionError> {
        JsonRpcError::ServerError(error)
    }

    #[test]
    fn only_a_refusal_proves_a_submission_never_executed() {
        let refused = [
            server(JsonRpcServerError::HandlerError(
                RpcTransactionError::InvalidTransaction {
                    context: near_primitives::errors::InvalidTxError::Expired,
                },
            )),
            server(JsonRpcServerError::HandlerError(
                RpcTransactionError::DoesNotTrackShard,
            )),
            server(JsonRpcServerError::RequestValidationError(
                RpcRequestValidationErrorKind::ParseError {
                    error_message: "bad params".into(),
                },
            )),
            server(JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::TooManyRequests,
            )),
        ];
        for error in &refused {
            assert!(!submission_may_have_landed(error), "{error:?}");
        }
        let unknown = [
            server(JsonRpcServerError::HandlerError(
                RpcTransactionError::TimeoutError,
            )),
            server(JsonRpcServerError::HandlerError(
                RpcTransactionError::InternalError {
                    debug_info: "limits".into(),
                },
            )),
            server(JsonRpcServerError::InternalError { info: None }),
            server(JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::ServiceUnavailable,
            )),
            server(JsonRpcServerError::ResponseStatusError(
                JsonRpcServerResponseStatusError::TimeoutError,
            )),
        ];
        for error in &unknown {
            assert!(submission_may_have_landed(error), "{error:?}");
        }
    }

    fn payer() -> MixedAddress {
        MixedAddress::Near("payer.testnet".to_string())
    }

    /// The settle answer: a submission with no verdict is an error carrying
    /// the hash, never the `200 success:false` that says nothing was paid.
    #[test]
    fn a_submission_with_no_verdict_is_not_a_failed_settlement() {
        let hash = CryptoHash([7; 32]);
        match settle_outcome(
            Err(FacilitatorLocalError::SettlementUnconfirmed(
                TransactionHash::Near(hash.0),
                Network::NearTestnet,
            )),
            payer(),
            Network::NearTestnet,
        ) {
            Err(FacilitatorLocalError::SettlementUnconfirmed(tx, _)) => {
                assert_eq!(tx, TransactionHash::Near(hash.0))
            }
            other => panic!("expected the unconfirmed error, got {other:?}"),
        }
        let refused = settle_outcome(
            Err(FacilitatorLocalError::ContractCall("refused".into())),
            payer(),
            Network::NearTestnet,
        )
        .expect("a refusal is a settle verdict");
        assert!(!refused.success);
        assert!(refused.transaction.is_none());
        let settled = settle_outcome(Ok(hash), payer(), Network::NearTestnet).unwrap();
        assert!(settled.success);
        assert_eq!(settled.transaction, Some(TransactionHash::Near(hash.0)));
    }

    /// The predicate is wired to the real broadcast: a gateway error is
    /// unconfirmed under the hash of the transaction sent, a provider refusal
    /// is not.
    #[tokio::test]
    async fn the_broadcast_reports_a_lost_answer_under_its_hash() {
        use axum::{response::IntoResponse, routing::post, Router};
        use near_crypto::KeyType;
        for (status, unconfirmed) in [
            (axum::http::StatusCode::BAD_GATEWAY, true),
            (axum::http::StatusCode::TOO_MANY_REQUESTS, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let app = Router::new().route(
                "/",
                post(move || async move { (status, "").into_response() }),
            );
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let provider = NearProvider::try_new(
                SecretKey::from_seed(KeyType::ED25519, "relayer"),
                "relayer.testnet".into(),
                format!("http://{addr}/"),
                Network::NearTestnet,
            )
            .unwrap();
            let signed = Transaction::V0(TransactionV0 {
                signer_id: provider.account_id.clone(),
                public_key: provider.public_key(),
                nonce: 1,
                receiver_id: "payer.testnet".parse().unwrap(),
                block_hash: CryptoHash::default(),
                actions: vec![],
            })
            .sign(&provider.signer);
            let expected = signed.get_hash();
            match (provider.broadcast(signed).await, unconfirmed) {
                (Err(FacilitatorLocalError::SettlementUnconfirmed(tx, network)), true) => {
                    assert_eq!(tx, TransactionHash::Near(expected.0));
                    assert_eq!(network, Network::NearTestnet);
                }
                (Err(FacilitatorLocalError::ContractCall(message)), false) => {
                    assert!(
                        message.starts_with("Failed to submit meta-transaction"),
                        "{message}"
                    )
                }
                (other, _) => panic!("{status}: unexpected {other:?}"),
            }
        }
    }
}
