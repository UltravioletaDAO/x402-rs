//! Permit2-based verification and settlement for the upto payment scheme.
//!
//! This module implements the core verify and settle logic for the `upto` scheme:
//! - `verify_upto()` - Validates a Permit2 payment authorization (off-chain + on-chain simulation)
//! - `settle_upto()` - Settles a payment for the actual amount used (<= authorized max)

use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, WalletProvider};
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use alloy::network::TransactionBuilder as _;
use tracing::{debug, info, instrument, warn};

use crate::chain::evm::{EvmProvider, MetaEvmProvider, MetaTransaction};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};

use super::abi;
use super::errors::UptoError;
use super::types::*;

// ============================================================================
// Public API
// ============================================================================

/// Verify an upto payment authorization.
///
/// Validates off-chain constraints (scheme, amounts, addresses, time) and
/// on-chain constraints (Permit2 allowance, balance, settlement simulation).
#[instrument(skip_all, err)]
pub async fn verify_upto<F>(body: &str, facilitator: &F) -> Result<serde_json::Value, UptoError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    if !super::is_enabled() {
        return Err(UptoError::FeatureDisabled);
    }

    let request: UptoRequest =
        serde_json::from_str(body).map_err(UptoError::DeserializationError)?;

    // Validate scheme
    let accepted = &request.payment_payload.accepted;
    if accepted.scheme != super::UPTO_SCHEME {
        return Err(UptoError::InvalidScheme(accepted.scheme.clone()));
    }

    // Resolve network
    let network_str = &accepted.network;
    let network = Network::from_caip2(network_str)
        .ok_or_else(|| UptoError::UnsupportedNetwork(network_str.clone()))?;

    // Get EVM provider
    let evm_provider = get_evm_provider(facilitator, network)?;
    let permit2_auth = &request.payment_payload.payload.permit_2_authorization;

    // Off-chain validations
    validate_offchain(&request)?;

    // Parse amounts and addresses for on-chain checks
    let payer = parse_address(&permit2_auth.from)?;
    let token = parse_address(&permit2_auth.permitted.token)?;
    let max_amount = parse_amount(&permit2_auth.permitted.amount)?;

    // On-chain: Check Permit2 allowance
    check_permit2_allowance(evm_provider, token, payer, max_amount).await?;

    // On-chain: Check token balance
    check_token_balance(evm_provider, token, payer, max_amount).await?;

    // On-chain: Simulate settlement with max amount (worst case)
    simulate_settlement(evm_provider, &request, max_amount).await?;

    info!(
        payer = %payer,
        network = %network,
        max_amount = %max_amount,
        "Upto payment verification succeeded"
    );

    Ok(serde_json::json!({
        "isValid": true,
        "payer": format!("{:#x}", payer),
    }))
}

/// Settle an upto payment for the actual amount consumed.
///
/// The actual amount is taken from `paymentRequirements.amount` and must be
/// <= the authorized max in `paymentPayload.accepted.amount`.
///
/// If the actual amount is zero, no on-chain transaction is submitted.
#[instrument(skip_all, err)]
pub async fn settle_upto<F>(
    body: &str,
    facilitator: &F,
) -> Result<UptoSettleResponse, UptoError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    if !super::is_enabled() {
        return Err(UptoError::FeatureDisabled);
    }

    let request: UptoRequest =
        serde_json::from_str(body).map_err(UptoError::DeserializationError)?;

    // Validate scheme
    let accepted = &request.payment_payload.accepted;
    if accepted.scheme != super::UPTO_SCHEME {
        return Err(UptoError::InvalidScheme(accepted.scheme.clone()));
    }

    // Resolve network
    let network_str = &accepted.network;
    let network = Network::from_caip2(network_str)
        .ok_or_else(|| UptoError::UnsupportedNetwork(network_str.clone()))?;

    let evm_provider = get_evm_provider(facilitator, network)?;
    let permit2_auth = &request.payment_payload.payload.permit_2_authorization;

    // Off-chain validations
    validate_offchain(&request)?;

    // Parse amounts
    let payer = parse_address(&permit2_auth.from)?;
    let max_amount = parse_amount(&permit2_auth.permitted.amount)?;
    let actual_amount = parse_amount(&request.payment_requirements.amount)?;

    // Validate: actual <= max
    if actual_amount > max_amount {
        return Err(UptoError::AmountExceedsMax {
            actual: actual_amount.to_string(),
            max: max_amount.to_string(),
        });
    }

    // Zero settlement - no on-chain transaction needed
    if actual_amount.is_zero() {
        info!(
            payer = %payer,
            network = %network,
            "Upto zero settlement - no on-chain transaction"
        );
        return Ok(UptoSettleResponse {
            success: true,
            error_reason: None,
            payer: Some(format!("{:#x}", payer)),
            transaction: String::new(),
            network: network_str.clone(),
            amount: "0".to_string(),
        });
    }

    // Execute the on-chain settlement
    let tx_hash = execute_settlement(evm_provider, &request, actual_amount).await?;

    info!(
        payer = %payer,
        network = %network,
        actual_amount = %actual_amount,
        max_amount = %max_amount,
        tx = %tx_hash,
        "Upto settlement succeeded"
    );

    Ok(UptoSettleResponse {
        success: true,
        error_reason: None,
        payer: Some(format!("{:#x}", payer)),
        transaction: format!("{:#x}", tx_hash),
        network: network_str.clone(),
        amount: actual_amount.to_string(),
    })
}

// ============================================================================
// Off-chain validation
// ============================================================================

/// Validate off-chain constraints that don't require RPC calls.
fn validate_offchain(request: &UptoRequest) -> Result<(), UptoError> {
    let accepted = &request.payment_payload.accepted;
    let requirements = &request.payment_requirements;
    let permit2_auth = &request.payment_payload.payload.permit_2_authorization;

    // Scheme must match
    if requirements.scheme != super::UPTO_SCHEME {
        return Err(UptoError::InvalidScheme(requirements.scheme.clone()));
    }

    // Network must match
    if accepted.network != requirements.network {
        return Err(UptoError::InvalidPayload(format!(
            "network mismatch: accepted={}, requirements={}",
            accepted.network, requirements.network
        )));
    }

    // Asset must match
    if !accepted.asset.eq_ignore_ascii_case(&requirements.asset) {
        return Err(UptoError::InvalidPayload(format!(
            "asset mismatch: accepted={}, requirements={}",
            accepted.asset, requirements.asset
        )));
    }

    // Spender must be the UPTO_PERMIT2_PROXY_ADDRESS
    let spender = parse_address(&permit2_auth.spender)?;
    if spender != UPTO_PERMIT2_PROXY_ADDRESS {
        return Err(UptoError::SpenderMismatch {
            expected: format!("{:#x}", UPTO_PERMIT2_PROXY_ADDRESS),
            actual: format!("{:#x}", spender),
        });
    }

    // Witness.to must match pay_to (recipient binding)
    let witness_to = parse_address(&permit2_auth.witness.to)?;
    let pay_to = parse_address(&accepted.pay_to)?;
    if witness_to != pay_to {
        return Err(UptoError::RecipientMismatch {
            expected: format!("{:#x}", pay_to),
            actual: format!("{:#x}", witness_to),
        });
    }

    // Permitted amount must match accepted amount (client authorized the max)
    let permitted = parse_amount(&permit2_auth.permitted.amount)?;
    let accepted_amount = parse_amount(&accepted.amount)?;
    if permitted != accepted_amount {
        return Err(UptoError::InvalidPayload(format!(
            "permitted amount ({}) != accepted amount ({})",
            permitted, accepted_amount
        )));
    }

    // Token in permit must match asset
    let permit_token = parse_address(&permit2_auth.permitted.token)?;
    let asset = parse_address(&accepted.asset)?;
    if permit_token != asset {
        return Err(UptoError::InvalidPayload(format!(
            "permit token ({:#x}) != asset ({:#x})",
            permit_token, asset
        )));
    }

    // Time validity (best-effort: we check against current system time)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Check deadline hasn't passed
    if let Ok(deadline) = permit2_auth.deadline.parse::<u64>() {
        if deadline < now {
            return Err(UptoError::Expired { deadline, now });
        }
    }

    // Check validAfter (with 60s tolerance for clock skew)
    if let Ok(valid_after) = permit2_auth.witness.valid_after.parse::<u64>() {
        if valid_after > now + 60 {
            return Err(UptoError::NotYetValid { valid_after, now });
        }
    }

    debug!("Off-chain validation passed for upto payment");
    Ok(())
}

// ============================================================================
// On-chain checks
// ============================================================================

/// Check that the payer has sufficient Permit2 allowance.
async fn check_permit2_allowance(
    provider: &EvmProvider,
    token: Address,
    payer: Address,
    required: U256,
) -> Result<(), UptoError> {
    let call = abi::allowanceCall {
        owner: payer,
        spender: PERMIT2_ADDRESS,
    };

    let result = eth_call(provider, token, &call).await?;
    let allowance = abi::allowanceCall::abi_decode_returns(&result)
        .map_err(|e| UptoError::ContractCall(format!("decode allowance: {e}")))?;

    if allowance < required {
        return Err(UptoError::InsufficientAllowance {
            has: allowance.to_string(),
            needs: required.to_string(),
        });
    }

    debug!(allowance = %allowance, required = %required, "Permit2 allowance check passed");
    Ok(())
}

/// Check that the payer has sufficient token balance.
async fn check_token_balance(
    provider: &EvmProvider,
    token: Address,
    payer: Address,
    required: U256,
) -> Result<(), UptoError> {
    let call = abi::balanceOfCall { account: payer };

    let result = eth_call(provider, token, &call).await?;
    let balance = abi::balanceOfCall::abi_decode_returns(&result)
        .map_err(|e| UptoError::ContractCall(format!("decode balanceOf: {e}")))?;

    if balance < required {
        return Err(UptoError::InsufficientBalance {
            has: balance.to_string(),
            needs: required.to_string(),
        });
    }

    debug!(balance = %balance, required = %required, "Token balance check passed");
    Ok(())
}

/// Simulate settlement with the specified amount via eth_call.
async fn simulate_settlement(
    provider: &EvmProvider,
    request: &UptoRequest,
    amount: U256,
) -> Result<(), UptoError> {
    let settle_call = build_settle_call(request, amount)?;
    let calldata = settle_call.abi_encode();

    let tx = TransactionRequest::default()
        .with_to(UPTO_PERMIT2_PROXY_ADDRESS)
        .with_input(Bytes::from(calldata));

    // Use the facilitator's signer as the from address for simulation
    // The proxy contract checks msg.sender == witness.facilitator
    let from = provider.inner().default_signer_address();

    let tx = tx.with_from(from);

    provider
        .inner()
        .call(tx)
        .await
        .map_err(|e| {
            warn!(error = %e, "Upto settlement simulation failed");
            UptoError::VerificationFailed(format!("settlement simulation reverted: {e}"))
        })?;

    debug!("Settlement simulation passed");
    Ok(())
}

// ============================================================================
// Settlement execution
// ============================================================================

/// Execute the on-chain settlement for the actual amount.
async fn execute_settlement(
    provider: &EvmProvider,
    request: &UptoRequest,
    actual_amount: U256,
) -> Result<alloy::primitives::B256, UptoError> {
    let settle_call = build_settle_call(request, actual_amount)?;
    let calldata = settle_call.abi_encode();

    let meta_tx = MetaTransaction {
        to: UPTO_PERMIT2_PROXY_ADDRESS,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| UptoError::SettlementFailed(format!("{e}")))?;

    if !receipt.status() {
        return Err(UptoError::SettlementFailed(format!(
            "transaction {} reverted",
            receipt.transaction_hash
        )));
    }

    Ok(receipt.transaction_hash)
}

// ============================================================================
// Helpers
// ============================================================================

/// Build the `settle()` calldata for the X402UptoPermit2Proxy contract.
fn build_settle_call(
    request: &UptoRequest,
    amount: U256,
) -> Result<abi::settleCall, UptoError> {
    let permit2_auth = &request.payment_payload.payload.permit_2_authorization;

    // Parse all fields
    let token = parse_address(&permit2_auth.permitted.token)?;
    let permitted_amount = parse_amount(&permit2_auth.permitted.amount)?;
    let nonce = parse_amount(&permit2_auth.nonce)?;
    let deadline = parse_amount(&permit2_auth.deadline)?;
    let owner = parse_address(&permit2_auth.from)?;
    let witness_to = parse_address(&permit2_auth.witness.to)?;
    let valid_after = parse_amount(&permit2_auth.witness.valid_after)?;

    // Parse facilitator address from witness (if present) or use zero address
    let facilitator_addr = permit2_auth
        .witness
        .facilitator
        .as_ref()
        .map(|f| parse_address(f))
        .transpose()?
        .unwrap_or(Address::ZERO);

    // Parse signature
    let sig_hex = request
        .payment_payload
        .payload
        .signature
        .strip_prefix("0x")
        .unwrap_or(&request.payment_payload.payload.signature);
    let sig_bytes = hex::decode(sig_hex)
        .map_err(|e| UptoError::InvalidPayload(format!("invalid signature hex: {e}")))?;

    Ok(abi::settleCall {
        permit: abi::PermitTransferFrom {
            permitted: abi::TokenPermissions {
                token,
                amount: permitted_amount,
            },
            nonce,
            deadline,
        },
        amount,
        owner,
        witness: abi::Witness {
            to: witness_to,
            facilitator: facilitator_addr,
            validAfter: valid_after,
        },
        signature: Bytes::from(sig_bytes),
    })
}

/// Get the EVM provider for a network from the facilitator's provider map.
fn get_evm_provider<'a, F>(
    facilitator: &'a F,
    network: Network,
) -> Result<&'a EvmProvider, UptoError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    let provider_map = facilitator.provider_map();
    let network_provider = provider_map
        .by_network(&network)
        .ok_or_else(|| UptoError::ProviderNotFound(network.to_string()))?;

    match network_provider {
        NetworkProvider::Evm(provider) => Ok(provider),
        _ => Err(UptoError::NonEvmNetwork),
    }
}

/// Make an eth_call (view function) against a contract.
async fn eth_call(
    provider: &EvmProvider,
    target: Address,
    call: &impl SolCall,
) -> Result<Bytes, UptoError> {
    let calldata = call.abi_encode();

    let tx = TransactionRequest::default()
        .with_to(target)
        .with_input(Bytes::from(calldata));

    let result = provider
        .inner()
        .call(tx)
        .await
        .map_err(|e| UptoError::ContractCall(format!("eth_call failed: {e}")))?;

    Ok(result)
}
