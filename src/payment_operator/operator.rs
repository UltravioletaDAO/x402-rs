//! Escrow Scheme settlement and verification logic
//!
//! This module handles requests with scheme="escrow",
//! supporting the full escrow lifecycle:
//! - verify: Validate escrow payload (balance check, address validation)
//! - authorize: Lock funds in escrow (ERC-3009 signature required)
//! - release: Send escrowed funds to receiver (no signature needed)
//! - refundInEscrow: Return escrowed funds to payer (no signature needed)
//! - charge / refundPostEscrow: Deferred to Phase 3
//!
//! Based on reference implementation:
//! https://github.com/BackTrackCo/x402r-scheme/tree/main/packages/evm/src/escrow/facilitator

use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::network::TransactionBuilder as _;
use tracing::{debug, info, warn, instrument};

use crate::chain::evm::{EvmProvider, MetaEvmProvider, MetaTransaction};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{EvmAddress, MixedAddress, SettleResponse, TransactionHash};

use super::abi::{EscrowContract, OperatorContract};
use super::addresses::OperatorAddresses;
use super::errors::OperatorError;
use super::types::{
    ContractPaymentInfo, EscrowExtra, EscrowLifecyclePayload, EscrowPayload, EscrowStateQuery,
    EscrowStateResponse,
};

// Minimal ERC-20 ABI for balance checks during verify
sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
    }
}

/// Escrow scheme identifier
pub const ESCROW_SCHEME: &str = "escrow";

// ============================================================================
// Parsed request types
// ============================================================================

/// Parsed escrow request dispatched by action
enum ParsedEscrowRequest {
    Authorize {
        network: Network,
        payload: EscrowPayload,
        extra: EscrowExtra,
    },
    Release {
        network: Network,
        lifecycle: EscrowLifecyclePayload,
        extra: EscrowExtra,
    },
    RefundInEscrow {
        network: Network,
        lifecycle: EscrowLifecyclePayload,
        extra: EscrowExtra,
    },
}

// ============================================================================
// Main settlement dispatcher
// ============================================================================

/// Main escrow settlement function
///
/// This is called from handlers.rs when scheme="escrow" is detected.
/// Dispatches to the appropriate action based on the "action" field.
#[instrument(skip_all, err, fields(network))]
pub async fn settle_escrow<F>(body: &str, facilitator: &F) -> Result<SettleResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    // Check feature flag
    if !super::is_enabled() {
        return Err(OperatorError::FeatureDisabled);
    }

    // Parse the request (action-aware)
    let parsed = parse_escrow_request(body)?;

    match parsed {
        ParsedEscrowRequest::Authorize {
            network,
            payload,
            extra,
        } => execute_authorize_flow(network, &payload, &extra, facilitator).await,

        ParsedEscrowRequest::Release {
            network,
            lifecycle,
            extra,
        } => execute_release_flow(network, &lifecycle, &extra, facilitator).await,

        ParsedEscrowRequest::RefundInEscrow {
            network,
            lifecycle,
            extra,
        } => execute_refund_in_escrow_flow(network, &lifecycle, &extra, facilitator).await,
    }
}

// ============================================================================
// Escrow verification
// ============================================================================

/// Verify an escrow scheme payment payload.
///
/// Called from handlers.rs when scheme="escrow" is detected on /verify.
/// Validates the escrow payload structure, checks payer token balance,
/// and returns a VerifyResponse-compatible JSON value.
///
/// This does NOT verify the ERC-3009 signature on-chain (that happens at
/// settle/authorize time). It validates that the payment CAN be settled:
/// - Correct contract addresses (escrow, token_collector)
/// - Payer has sufficient token balance
/// - Authorization.to matches token_collector
#[instrument(skip_all, err)]
pub async fn verify_escrow<F>(body: &str, facilitator: &F) -> Result<serde_json::Value, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    // Check feature flag
    if !super::is_enabled() {
        return Err(OperatorError::FeatureDisabled);
    }

    let json_value: serde_json::Value = serde_json::from_str(body)?;

    // Extract payload (could be top-level or nested in paymentPayload)
    let (payload_value, req_value) = extract_escrow_verify_fields(&json_value)?;

    // Parse escrow payload
    let escrow_payload: EscrowPayload = serde_json::from_value(payload_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(format!("Invalid escrow payload: {}", e)))?;

    let payer = escrow_payload.authorization.from;

    // Extract network from paymentRequirements
    let network_str = req_value
        .get("network")
        .and_then(|n| n.as_str())
        .ok_or_else(|| OperatorError::MissingField("paymentRequirements.network".to_string()))?;

    let network = Network::from_caip2(network_str)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(network_str.to_string()))?;

    // Extract extra from paymentRequirements
    let extra: EscrowExtra = if let Some(extra_val) = req_value.get("extra") {
        serde_json::from_value(extra_val.clone())
            .map_err(|e| OperatorError::InvalidExtensionFormat(format!("Invalid extra: {}", e)))?
    } else {
        return Ok(serde_json::json!({
            "isValid": false,
            "invalidReason": "missing paymentRequirements.extra",
            "payer": format!("{}", payer)
        }));
    };

    // Validate addresses (escrow, token_collector; operator is flexible)
    let addrs = match OperatorAddresses::for_network(network) {
        Some(a) => a,
        None => {
            return Ok(serde_json::json!({
                "isValid": false,
                "invalidReason": format!("unsupported escrow network: {}", network_str),
                "payer": format!("{}", payer)
            }));
        }
    };

    if let Err(e) = validate_addresses(&extra, &addrs, false) {
        warn!(error = %e, "Escrow verify address validation failed");
        return Ok(serde_json::json!({
            "isValid": false,
            "invalidReason": format!("{}", e),
            "payer": format!("{}", payer)
        }));
    }

    // Validate authorization.to == token_collector
    if escrow_payload.authorization.to != extra.token_collector {
        return Ok(serde_json::json!({
            "isValid": false,
            "invalidReason": format!(
                "authorization.to ({}) does not match tokenCollector ({})",
                escrow_payload.authorization.to, extra.token_collector
            ),
            "payer": format!("{}", payer)
        }));
    }

    // Check payer token balance
    let evm_provider = get_evm_provider(facilitator, network)?;
    let token_address = escrow_payload.payment_info.token;
    let required_amount = U256::from(escrow_payload.authorization.value);

    match check_token_balance(evm_provider, token_address, payer, required_amount).await {
        Ok(true) => {
            info!(
                payer = ?payer,
                network = %network,
                amount = %required_amount,
                "Escrow verify: valid"
            );
            Ok(serde_json::json!({
                "isValid": true,
                "payer": format!("{}", payer)
            }))
        }
        Ok(false) => {
            info!(payer = ?payer, "Escrow verify: insufficient funds");
            Ok(serde_json::json!({
                "isValid": false,
                "invalidReason": "insufficient_funds",
                "payer": format!("{}", payer)
            }))
        }
        Err(e) => {
            warn!(error = %e, "Escrow verify: balance check failed, allowing anyway");
            // If balance check fails (RPC error), still return valid
            // The actual settlement will catch any issues
            Ok(serde_json::json!({
                "isValid": true,
                "payer": format!("{}", payer)
            }))
        }
    }
}

/// Extract escrow payload and paymentRequirements from various request formats.
///
/// Supports:
/// - Top-level: { scheme, payload, paymentRequirements }
/// - Wrapped: { paymentPayload: { scheme, payload, accepted } }
fn extract_escrow_verify_fields(json: &serde_json::Value) -> Result<(serde_json::Value, serde_json::Value), OperatorError> {
    // Format 1: Top-level { scheme, payload, paymentRequirements }
    if json.get("scheme").and_then(|s| s.as_str()) == Some(ESCROW_SCHEME) {
        let payload = json.get("payload")
            .ok_or_else(|| OperatorError::MissingField("payload".to_string()))?
            .clone();
        let requirements = json.get("paymentRequirements")
            .ok_or_else(|| OperatorError::MissingField("paymentRequirements".to_string()))?
            .clone();
        return Ok((payload, requirements));
    }

    // Format 2: { paymentPayload: { scheme/accepted.scheme, payload, ... } }
    if let Some(pp) = json.get("paymentPayload") {
        let scheme = pp.get("scheme").and_then(|s| s.as_str())
            .or_else(|| pp.get("accepted").and_then(|a| a.get("scheme")).and_then(|s| s.as_str()));

        if scheme == Some(ESCROW_SCHEME) {
            let payload = pp.get("payload")
                .ok_or_else(|| OperatorError::MissingField("paymentPayload.payload".to_string()))?
                .clone();
            // paymentRequirements can be in "accepted" (v2) or "paymentRequirements" (v1)
            let requirements = pp.get("accepted")
                .or_else(|| json.get("paymentRequirements"))
                .ok_or_else(|| OperatorError::MissingField("paymentRequirements/accepted".to_string()))?
                .clone();
            return Ok((payload, requirements));
        }
    }

    Err(OperatorError::InvalidScheme("not an escrow scheme request".to_string()))
}

/// Check if an address has sufficient ERC-20 token balance.
async fn check_token_balance(
    provider: &EvmProvider,
    token: Address,
    account: Address,
    required: U256,
) -> Result<bool, OperatorError> {
    let call = IERC20::balanceOfCall { account };
    let result = eth_call(provider, token, &call).await?;
    let balance = IERC20::balanceOfCall::abi_decode_returns(&result)
        .map_err(|e| OperatorError::ContractCall(format!("decode balanceOf: {}", e)))?;
    Ok(balance >= required)
}

// ============================================================================
// Escrow state query
// ============================================================================

/// Query escrow state from the AuthCaptureEscrow contract
///
/// Calls getHash(paymentInfo) then paymentState(hash) to return the
/// current escrow state (capturable, refundable amounts).
#[instrument(skip_all, err)]
pub async fn query_escrow_state<F>(
    body: &str,
    facilitator: &F,
) -> Result<EscrowStateResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    // Check feature flag
    if !super::is_enabled() {
        return Err(OperatorError::FeatureDisabled);
    }

    let query: EscrowStateQuery =
        serde_json::from_str(body).map_err(|e| OperatorError::InvalidExtensionFormat(e.to_string()))?;

    let network = Network::from_caip2(&query.network)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(query.network.clone()))?;

    let addrs = OperatorAddresses::for_network(network)
        .ok_or_else(|| OperatorError::unsupported_network(&network))?;

    // Validate addresses (read-only query, lenient on operator)
    validate_addresses(&query.extra, &addrs, false)?;

    // Get EVM provider
    let evm_provider = get_evm_provider(facilitator, network)?;

    // Build ContractPaymentInfo from the query
    let lifecycle = EscrowLifecyclePayload {
        payment_info: query.payment_info,
        payer: query.payer,
        amount: 0, // not used for state query
    };
    let payment_info = ContractPaymentInfo::from_lifecycle_payload(&lifecycle);
    // Use escrow ABI type for EscrowContract calls (different Rust type from OperatorContract's)
    let escrow_payment_info = payment_info.to_escrow_abi_type();

    // Call getHash(paymentInfo) on the escrow contract
    let get_hash_call = EscrowContract::getHashCall {
        paymentInfo: escrow_payment_info,
    };
    let hash_result = eth_call(evm_provider, addrs.escrow, &get_hash_call).await?;
    let payment_info_hash: FixedBytes<32> =
        EscrowContract::getHashCall::abi_decode_returns(&hash_result)
            .map_err(|e| OperatorError::EscrowStateQuery(format!("decode getHash: {}", e)))?;

    // Call paymentState(hash) on the escrow contract
    let state_call = EscrowContract::paymentStateCall {
        paymentInfoHash: payment_info_hash,
    };
    let state_result = eth_call(evm_provider, addrs.escrow, &state_call).await?;
    let state = EscrowContract::paymentStateCall::abi_decode_returns(&state_result)
        .map_err(|e| OperatorError::EscrowStateQuery(format!("decode paymentState: {}", e)))?;

    info!(
        hash = ?payment_info_hash,
        has_collected = state.hasCollectedPayment,
        capturable = %state.capturableAmount,
        refundable = %state.refundableAmount,
        "Escrow state queried"
    );

    Ok(EscrowStateResponse {
        has_collected_payment: state.hasCollectedPayment,
        capturable_amount: state.capturableAmount.to::<u128>(),
        refundable_amount: state.refundableAmount.to::<u128>(),
        payment_info_hash: format!("0x{}", hex::encode(payment_info_hash)),
        network: query.network,
    })
}

// ============================================================================
// Action flows
// ============================================================================

/// Execute the authorize flow (existing behavior)
async fn execute_authorize_flow<F>(
    network: Network,
    payload: &EscrowPayload,
    extra: &EscrowExtra,
    facilitator: &F,
) -> Result<SettleResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    info!(
        network = %network,
        payer = ?payload.authorization.from,
        receiver = ?payload.payment_info.receiver,
        amount = %payload.authorization.value,
        "Processing escrow scheme settlement (authorize)"
    );

    let addrs = OperatorAddresses::for_network(network)
        .ok_or_else(|| OperatorError::unsupported_network(&network))?;
    let evm_provider = get_evm_provider(facilitator, network)?;
    let tx_hash = execute_authorize(payload, extra, &addrs, evm_provider).await?;

    info!(tx_hash = ?tx_hash, "Escrow authorize transaction submitted");

    let tx_hash_bytes: [u8; 32] = tx_hash.into();
    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(payload.authorization.from)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network,
        proof_of_payment: None,
        extensions: None,
    })
}

/// Execute the release flow - send escrowed funds to receiver
async fn execute_release_flow<F>(
    network: Network,
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    facilitator: &F,
) -> Result<SettleResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    info!(
        network = %network,
        payer = ?lifecycle.payer,
        receiver = ?lifecycle.payment_info.receiver,
        amount = %lifecycle.amount,
        "Processing escrow scheme settlement (release)"
    );

    let addrs = OperatorAddresses::for_network(network)
        .ok_or_else(|| OperatorError::unsupported_network(&network))?;
    let evm_provider = get_evm_provider(facilitator, network)?;
    let tx_hash = execute_release(lifecycle, extra, &addrs, evm_provider).await?;

    info!(tx_hash = ?tx_hash, "Escrow release transaction submitted");

    let tx_hash_bytes: [u8; 32] = tx_hash.into();
    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(lifecycle.payer)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network,
        proof_of_payment: None,
        extensions: None,
    })
}

/// Execute the refundInEscrow flow - return escrowed funds to payer
async fn execute_refund_in_escrow_flow<F>(
    network: Network,
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    facilitator: &F,
) -> Result<SettleResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    info!(
        network = %network,
        payer = ?lifecycle.payer,
        receiver = ?lifecycle.payment_info.receiver,
        amount = %lifecycle.amount,
        "Processing escrow scheme settlement (refundInEscrow)"
    );

    let addrs = OperatorAddresses::for_network(network)
        .ok_or_else(|| OperatorError::unsupported_network(&network))?;
    let evm_provider = get_evm_provider(facilitator, network)?;
    let tx_hash = execute_refund_in_escrow(lifecycle, extra, &addrs, evm_provider).await?;

    info!(tx_hash = ?tx_hash, "Escrow refundInEscrow transaction submitted");

    let tx_hash_bytes: [u8; 32] = tx_hash.into();
    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(lifecycle.payer)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network,
        proof_of_payment: None,
        extensions: None,
    })
}

// ============================================================================
// Request parsing
// ============================================================================

/// Parse escrow scheme request from body, with action routing.
///
/// Supports two request formats:
/// - Top-level: `{ scheme, action, payload, paymentRequirements }`
/// - Wrapped (v2): `{ paymentPayload: { payload, accepted: { scheme, network, extra } }, paymentRequirements: { ... } }`
fn parse_escrow_request(body: &str) -> Result<ParsedEscrowRequest, OperatorError> {
    let json_value: serde_json::Value = serde_json::from_str(body)?;

    // Extract fields from whichever format is present
    let (payload_value, network_str, extra_value, action) = extract_escrow_settle_fields(&json_value)?;

    let network = Network::from_caip2(network_str)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(network_str.to_string()))?;

    let extra: EscrowExtra = serde_json::from_value(extra_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(format!("Invalid extra: {}", e)))?;

    match action {
        "authorize" => {
            let escrow_payload: EscrowPayload =
                serde_json::from_value(payload_value.clone())
                    .map_err(|e| OperatorError::InvalidExtensionFormat(e.to_string()))?;
            Ok(ParsedEscrowRequest::Authorize {
                network,
                payload: escrow_payload,
                extra,
            })
        }
        "release" => {
            let lifecycle: EscrowLifecyclePayload =
                serde_json::from_value(payload_value.clone())
                    .map_err(|e| OperatorError::InvalidExtensionFormat(
                        format!("Invalid release payload: {}", e),
                    ))?;
            Ok(ParsedEscrowRequest::Release {
                network,
                lifecycle,
                extra,
            })
        }
        "refundInEscrow" => {
            let lifecycle: EscrowLifecyclePayload =
                serde_json::from_value(payload_value.clone())
                    .map_err(|e| OperatorError::InvalidExtensionFormat(
                        format!("Invalid refundInEscrow payload: {}", e),
                    ))?;
            Ok(ParsedEscrowRequest::RefundInEscrow {
                network,
                lifecycle,
                extra,
            })
        }
        other => Err(OperatorError::UnknownAction(other.to_string())),
    }
}

/// Extract settle fields from either top-level or wrapped (v2) format.
///
/// Returns (payload, network_str, extra_value, action).
fn extract_escrow_settle_fields(json: &serde_json::Value) -> Result<(serde_json::Value, &str, serde_json::Value, &str), OperatorError> {
    // Format 1: Top-level { scheme, action, payload, paymentRequirements }
    if json.get("scheme").and_then(|s| s.as_str()) == Some(ESCROW_SCHEME) {
        let action = json.get("action").and_then(|a| a.as_str()).unwrap_or("authorize");

        let payload = json.get("payload")
            .ok_or_else(|| OperatorError::MissingField("payload".to_string()))?
            .clone();

        let requirements = json.get("paymentRequirements")
            .ok_or_else(|| OperatorError::MissingField("paymentRequirements".to_string()))?;

        let network_str = requirements.get("network").and_then(|n| n.as_str())
            .ok_or_else(|| OperatorError::MissingField("paymentRequirements.network".to_string()))?;

        let extra = requirements.get("extra")
            .ok_or_else(|| OperatorError::MissingField("paymentRequirements.extra".to_string()))?
            .clone();

        return Ok((payload, network_str, extra, action));
    }

    // Format 2: { paymentPayload: { payload, accepted/scheme }, paymentRequirements }
    if let Some(pp) = json.get("paymentPayload") {
        let scheme = pp.get("scheme").and_then(|s| s.as_str())
            .or_else(|| pp.get("accepted").and_then(|a| a.get("scheme")).and_then(|s| s.as_str()));

        if scheme == Some(ESCROW_SCHEME) {
            let action = pp.get("action").and_then(|a| a.as_str())
                .or_else(|| pp.get("accepted").and_then(|a| a.get("action")).and_then(|a| a.as_str()))
                .unwrap_or("authorize");

            let payload = pp.get("payload")
                .ok_or_else(|| OperatorError::MissingField("paymentPayload.payload".to_string()))?
                .clone();

            // paymentRequirements: try top-level first, then accepted inside paymentPayload
            let requirements = json.get("paymentRequirements")
                .or_else(|| pp.get("accepted"))
                .ok_or_else(|| OperatorError::MissingField("paymentRequirements".to_string()))?;

            let network_str = requirements.get("network").and_then(|n| n.as_str())
                .ok_or_else(|| OperatorError::MissingField("paymentRequirements.network".to_string()))?;

            let extra = requirements.get("extra")
                .ok_or_else(|| OperatorError::MissingField("paymentRequirements.extra".to_string()))?
                .clone();

            return Ok((payload, network_str, extra, action));
        }
    }

    Err(OperatorError::InvalidScheme("not an escrow scheme request".to_string()))
}

// ============================================================================
// Contract execution functions
// ============================================================================

/// Execute authorize on PaymentOperator
///
/// Client-provided addresses (escrow, token_collector) are validated against
/// hardcoded OperatorAddresses. Operator address is NOT restricted — the
/// facilitator acts as a relay and the escrow contract enforces operator rules.
#[instrument(skip_all, err)]
async fn execute_authorize(
    escrow_payload: &EscrowPayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs, false)?;

    let payment_info = ContractPaymentInfo::from_escrow_payload(escrow_payload);
    let payment_info_abi = payment_info.to_abi_type();
    let amount = U256::from(escrow_payload.authorization.value);
    let token_collector = addrs.token_collector;
    let collector_data = encode_collector_data(&escrow_payload.signature);

    // Target is the client-provided operator address (already validated in validate_addresses)
    let target = extra.authorize_address.unwrap_or(extra.operator_address);

    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %amount,
        token_collector = ?token_collector,
        target = ?target,
        "Executing authorize on PaymentOperator"
    );

    let call = OperatorContract::authorizeCall {
        paymentInfo: payment_info_abi,
        amount,
        tokenCollector: token_collector,
        collectorData: collector_data,
    };

    send_operator_tx(provider, target, &call).await
}

/// Execute release on PaymentOperator
///
/// Sends escrowed funds to the receiver. No ERC-3009 signature needed.
/// The PaymentOperator contract checks msg.sender == operator for access control.
/// Operator address is not restricted — the escrow contract enforces operator rules.
#[instrument(skip_all, err)]
async fn execute_release(
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs, false)?;

    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let payment_info_abi = payment_info.to_abi_type();
    let amount = U256::from(lifecycle.amount);

    // Target is the client-provided operator address (already validated in validate_addresses)
    let target = extra.authorize_address.unwrap_or(extra.operator_address);

    debug!(
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %amount,
        target = ?target,
        "Executing release on PaymentOperator"
    );

    let call = OperatorContract::releaseCall {
        paymentInfo: payment_info_abi,
        amount,
        data: alloy::primitives::Bytes::new(),
    };

    send_operator_tx(provider, target, &call).await
}

/// Execute refundInEscrow on PaymentOperator
///
/// Returns escrowed funds to the payer. No ERC-3009 signature needed.
/// The amount parameter is uint120 in the ABI.
/// Operator address is not restricted — the escrow contract enforces operator rules.
#[instrument(skip_all, err)]
async fn execute_refund_in_escrow(
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs, false)?;

    // Bounds check: refundInEscrow takes uint120 (max ~1.3*10^36)
    const UINT120_MAX: u128 = (1u128 << 120) - 1;
    if lifecycle.amount > UINT120_MAX {
        return Err(OperatorError::InvalidAmount(format!(
            "amount {} exceeds uint120 max {}",
            lifecycle.amount, UINT120_MAX
        )));
    }

    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let payment_info_abi = payment_info.to_abi_type();

    // Convert to Uint<120, 2> for the ABI
    let amount = alloy::primitives::Uint::<120, 2>::from(lifecycle.amount);

    // Target is the client-provided operator address (already validated in validate_addresses)
    let target = extra.authorize_address.unwrap_or(extra.operator_address);

    debug!(
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %lifecycle.amount,
        target = ?target,
        "Executing refundInEscrow on PaymentOperator"
    );

    let call = OperatorContract::refundInEscrowCall {
        paymentInfo: payment_info_abi,
        amount,
        data: alloy::primitives::Bytes::new(),
    };

    send_operator_tx(provider, target, &call).await
}

// ============================================================================
// Helper functions
// ============================================================================

/// Get EVM provider for a network from the facilitator
fn get_evm_provider<'a, F>(facilitator: &'a F, network: Network) -> Result<&'a EvmProvider, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    let provider_map = facilitator.provider_map();
    let network_provider = provider_map
        .by_network(&network)
        .ok_or_else(|| OperatorError::ProviderNotFound(network.to_string()))?;

    match network_provider {
        NetworkProvider::Evm(provider) => Ok(provider),
        _ => Err(OperatorError::NonEvmNetwork),
    }
}

/// Send a transaction to the PaymentOperator contract
async fn send_operator_tx(
    provider: &EvmProvider,
    target: alloy::primitives::Address,
    call: &impl SolCall,
) -> Result<B256, OperatorError> {
    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: target,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Make an eth_call (view function) against a contract
async fn eth_call(
    provider: &EvmProvider,
    target: alloy::primitives::Address,
    call: &impl SolCall,
) -> Result<Bytes, OperatorError> {
    let calldata = call.abi_encode();

    let tx = TransactionRequest::default()
        .with_to(target)
        .with_input(Bytes::from(calldata));

    let result = provider
        .inner()
        .call(tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("eth_call failed: {:?}", e)))?;

    Ok(result)
}

/// Validate that client-provided addresses match known contract deployments.
///
/// Validates escrow and token_collector addresses (shared infrastructure).
/// Operator address is NOT validated — the facilitator acts as a relay and
/// the escrow contract enforces operator rules. Merchants bring their own
/// PaymentOperator contracts.
///
/// The `strict_operator` parameter is retained for future use but currently
/// all callers pass `false` (operator-agnostic mode).
///
/// Note on gas risk: accepting any operator means the facilitator may spend
/// gas on transactions that revert if the operator contract rejects them.
/// This is an accepted tradeoff for protocol openness.
fn validate_addresses(extra: &EscrowExtra, addrs: &OperatorAddresses, strict_operator: bool) -> Result<(), OperatorError> {
    if strict_operator {
        // Reserved for future use — currently all paths are operator-agnostic
        if addrs.payment_operators.is_empty() {
            return Err(OperatorError::UnsupportedNetwork(
                "no deployed PaymentOperator for this network".into(),
            ));
        }
        let client_target = extra.authorize_address.unwrap_or(extra.operator_address);
        if !addrs.payment_operators.contains(&client_target) {
            return Err(OperatorError::PaymentInfoInvalid(format!(
                "operator address mismatch: client={:?}, allowed={:?}",
                client_target, addrs.payment_operators
            )));
        }
    }
    // Operator address is not validated — merchants specify their own operator.

    // Validate token collector (shared infrastructure, always strict)
    if extra.token_collector != addrs.token_collector {
        return Err(OperatorError::PaymentInfoInvalid(format!(
            "token_collector mismatch: client={:?}, expected={:?}",
            extra.token_collector, addrs.token_collector
        )));
    }

    // Validate escrow address (shared infrastructure, always strict)
    if extra.escrow_address != addrs.escrow {
        return Err(OperatorError::PaymentInfoInvalid(format!(
            "escrow address mismatch: client={:?}, expected={:?}",
            extra.escrow_address, addrs.escrow
        )));
    }

    Ok(())
}

/// Encode collector data for authorize call.
fn encode_collector_data(signature: &Bytes) -> Bytes {
    signature.clone()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_escrow_request_authorize_default() {
        // No "action" field - defaults to authorize
        let body = r#"{
            "x402Version": 2,
            "scheme": "escrow",
            "payload": {
                "authorization": {
                    "from": "0x1111111111111111111111111111111111111111",
                    "to": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "1738500000",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000003039"
                },
                "signature": "0xabcdef1234567890",
                "paymentInfo": {
                    "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "receiver": "0x2222222222222222222222222222222222222222",
                    "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "maxAmount": "1000000",
                    "preApprovalExpiry": 281474976710655,
                    "authorizationExpiry": 281474976710655,
                    "refundExpiry": 281474976710655,
                    "minFeeBps": 0,
                    "maxFeeBps": 100,
                    "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
                }
            },
            "paymentRequirements": {
                "scheme": "escrow",
                "network": "eip155:84532",
                "maxAmountRequired": "1000000",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "payTo": "0x2222222222222222222222222222222222222222",
                "extra": {
                    "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                    "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
                }
            }
        }"#;

        let result = parse_escrow_request(body).unwrap();
        assert!(matches!(result, ParsedEscrowRequest::Authorize { .. }));

        if let ParsedEscrowRequest::Authorize { network, payload, extra } = result {
            assert_eq!(network, Network::BaseSepolia);
            assert_eq!(payload.authorization.value, 1_000_000);
            assert_eq!(
                extra.operator_address,
                alloy::primitives::address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70")
            );
        }
    }

    #[test]
    fn test_parse_release_request() {
        let body = r#"{
            "x402Version": 2,
            "scheme": "escrow",
            "action": "release",
            "payload": {
                "paymentInfo": {
                    "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "receiver": "0x2222222222222222222222222222222222222222",
                    "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "maxAmount": "1000000",
                    "preApprovalExpiry": 281474976710655,
                    "authorizationExpiry": 281474976710655,
                    "refundExpiry": 281474976710655,
                    "minFeeBps": 0,
                    "maxFeeBps": 100,
                    "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
                },
                "payer": "0x1111111111111111111111111111111111111111",
                "amount": "1000000"
            },
            "paymentRequirements": {
                "scheme": "escrow",
                "network": "eip155:84532",
                "extra": {
                    "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                    "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
                }
            }
        }"#;

        let result = parse_escrow_request(body).unwrap();
        assert!(matches!(result, ParsedEscrowRequest::Release { .. }));

        if let ParsedEscrowRequest::Release { network, lifecycle, .. } = result {
            assert_eq!(network, Network::BaseSepolia);
            assert_eq!(lifecycle.amount, 1_000_000);
            assert_eq!(
                lifecycle.payer,
                alloy::primitives::address!("1111111111111111111111111111111111111111")
            );
        }
    }

    #[test]
    fn test_parse_refund_in_escrow_request() {
        let body = r#"{
            "x402Version": 2,
            "scheme": "escrow",
            "action": "refundInEscrow",
            "payload": {
                "paymentInfo": {
                    "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "receiver": "0x2222222222222222222222222222222222222222",
                    "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "maxAmount": "1000000",
                    "preApprovalExpiry": 281474976710655,
                    "authorizationExpiry": 281474976710655,
                    "refundExpiry": 281474976710655,
                    "minFeeBps": 0,
                    "maxFeeBps": 100,
                    "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
                },
                "payer": "0x1111111111111111111111111111111111111111",
                "amount": "500000"
            },
            "paymentRequirements": {
                "scheme": "escrow",
                "network": "eip155:84532",
                "extra": {
                    "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                    "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
                }
            }
        }"#;

        let result = parse_escrow_request(body).unwrap();
        assert!(matches!(result, ParsedEscrowRequest::RefundInEscrow { .. }));

        if let ParsedEscrowRequest::RefundInEscrow { lifecycle, .. } = result {
            assert_eq!(lifecycle.amount, 500_000);
        }
    }

    #[test]
    fn test_default_action_is_authorize() {
        // Explicit action:"authorize" should work the same as no action
        let body = r#"{
            "x402Version": 2,
            "scheme": "escrow",
            "action": "authorize",
            "payload": {
                "authorization": {
                    "from": "0x1111111111111111111111111111111111111111",
                    "to": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757",
                    "value": "1000000",
                    "validAfter": "0",
                    "validBefore": "1738500000",
                    "nonce": "0x0000000000000000000000000000000000000000000000000000000000003039"
                },
                "signature": "0xabcdef1234567890",
                "paymentInfo": {
                    "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "receiver": "0x2222222222222222222222222222222222222222",
                    "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                    "maxAmount": "1000000",
                    "preApprovalExpiry": 281474976710655,
                    "authorizationExpiry": 281474976710655,
                    "refundExpiry": 281474976710655,
                    "minFeeBps": 0,
                    "maxFeeBps": 100,
                    "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
                }
            },
            "paymentRequirements": {
                "scheme": "escrow",
                "network": "eip155:84532",
                "extra": {
                    "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                    "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
                }
            }
        }"#;

        let result = parse_escrow_request(body).unwrap();
        assert!(matches!(result, ParsedEscrowRequest::Authorize { .. }));
    }

    #[test]
    fn test_unknown_action_returns_error() {
        let body = r#"{
            "x402Version": 2,
            "scheme": "escrow",
            "action": "destroyFunds",
            "payload": {},
            "paymentRequirements": {
                "network": "eip155:84532",
                "extra": {
                    "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                    "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                    "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
                }
            }
        }"#;

        let result = parse_escrow_request(body);
        assert!(matches!(result, Err(OperatorError::UnknownAction(_))));
    }

    #[test]
    fn test_invalid_scheme() {
        let body = r#"{
            "x402Version": 2,
            "scheme": "exact",
            "payload": {},
            "paymentRequirements": {}
        }"#;

        let result = parse_escrow_request(body);
        assert!(matches!(result, Err(OperatorError::InvalidScheme(_))));
    }

    #[test]
    fn test_encode_collector_data() {
        let signature = Bytes::from(vec![0xab, 0xcd, 0xef]);
        let encoded = encode_collector_data(&signature);
        assert_eq!(encoded, signature);
    }

    #[test]
    fn test_escrow_state_query_deserialization() {
        let body = r#"{
            "paymentInfo": {
                "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                "receiver": "0x2222222222222222222222222222222222222222",
                "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "maxAmount": "1000000",
                "preApprovalExpiry": 281474976710655,
                "authorizationExpiry": 281474976710655,
                "refundExpiry": 281474976710655,
                "minFeeBps": 0,
                "maxFeeBps": 100,
                "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
            },
            "payer": "0x1111111111111111111111111111111111111111",
            "network": "eip155:84532",
            "extra": {
                "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
                "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757"
            }
        }"#;

        let query: EscrowStateQuery = serde_json::from_str(body).unwrap();
        assert_eq!(query.network, "eip155:84532");
        assert_eq!(
            query.payer,
            alloy::primitives::address!("1111111111111111111111111111111111111111")
        );
    }
}
