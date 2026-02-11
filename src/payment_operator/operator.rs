//! Escrow Scheme settlement logic
//!
//! This module handles settlement requests with scheme="escrow",
//! supporting the full escrow lifecycle:
//! - authorize: Lock funds in escrow (ERC-3009 signature required)
//! - release: Send escrowed funds to receiver (no signature needed)
//! - refundInEscrow: Return escrowed funds to payer (no signature needed)
//! - charge / refundPostEscrow: Deferred to Phase 3
//!
//! Based on reference implementation:
//! https://github.com/BackTrackCo/x402r-scheme/tree/main/packages/evm/src/escrow/facilitator

use alloy::primitives::{Bytes, FixedBytes, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use alloy::network::TransactionBuilder as _;
use tracing::{debug, info, instrument};

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

    // Validate addresses
    validate_addresses(&query.extra, &addrs)?;

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
    })
}

// ============================================================================
// Request parsing
// ============================================================================

/// Parse escrow scheme request from body, with action routing
fn parse_escrow_request(body: &str) -> Result<ParsedEscrowRequest, OperatorError> {
    let json_value: serde_json::Value = serde_json::from_str(body)?;

    // Verify scheme is "escrow"
    let scheme = json_value
        .get("scheme")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OperatorError::MissingField("scheme".to_string()))?;

    if scheme != ESCROW_SCHEME {
        return Err(OperatorError::InvalidScheme(scheme.to_string()));
    }

    // Read action (default: "authorize" for backward compatibility)
    let action = json_value
        .get("action")
        .and_then(|a| a.as_str())
        .unwrap_or("authorize");

    // Extract network from paymentRequirements
    let network_str = json_value
        .get("paymentRequirements")
        .and_then(|pr| pr.get("network"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| OperatorError::MissingField("paymentRequirements.network".to_string()))?;

    let network = Network::from_caip2(network_str)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(network_str.to_string()))?;

    // Extract extra from paymentRequirements
    let extra_value = json_value
        .get("paymentRequirements")
        .and_then(|pr| pr.get("extra"))
        .ok_or_else(|| OperatorError::MissingField("paymentRequirements.extra".to_string()))?;

    let extra: EscrowExtra = serde_json::from_value(extra_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(format!("Invalid extra: {}", e)))?;

    // Extract payload based on action
    let payload_value = json_value
        .get("payload")
        .ok_or_else(|| OperatorError::MissingField("payload".to_string()))?;

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

// ============================================================================
// Contract execution functions
// ============================================================================

/// Execute authorize on PaymentOperator
///
/// SECURITY: Client-provided addresses (extra) are validated against hardcoded
/// OperatorAddresses to prevent gas drain attacks via arbitrary target addresses.
#[instrument(skip_all, err)]
async fn execute_authorize(
    escrow_payload: &EscrowPayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs)?;

    let payment_info = ContractPaymentInfo::from_escrow_payload(escrow_payload);
    let payment_info_abi = payment_info.to_abi_type();
    let amount = U256::from(escrow_payload.authorization.value);
    let token_collector = addrs.token_collector;
    let collector_data = encode_collector_data(&escrow_payload.signature);

    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %amount,
        token_collector = ?token_collector,
        "Executing authorize on PaymentOperator"
    );

    let target = addrs.payment_operator.ok_or_else(|| {
        OperatorError::UnsupportedNetwork("no deployed PaymentOperator for this network".into())
    })?;

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
#[instrument(skip_all, err)]
async fn execute_release(
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs)?;

    let payment_info = ContractPaymentInfo::from_lifecycle_payload(lifecycle);
    let payment_info_abi = payment_info.to_abi_type();
    let amount = U256::from(lifecycle.amount);

    debug!(
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %amount,
        "Executing release on PaymentOperator"
    );

    let target = addrs.payment_operator.ok_or_else(|| {
        OperatorError::UnsupportedNetwork("no deployed PaymentOperator for this network".into())
    })?;

    let call = OperatorContract::releaseCall {
        paymentInfo: payment_info_abi,
        amount,
    };

    send_operator_tx(provider, target, &call).await
}

/// Execute refundInEscrow on PaymentOperator
///
/// Returns escrowed funds to the payer. No ERC-3009 signature needed.
/// The amount parameter is uint120 in the ABI.
#[instrument(skip_all, err)]
async fn execute_refund_in_escrow(
    lifecycle: &EscrowLifecyclePayload,
    extra: &EscrowExtra,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    validate_addresses(extra, addrs)?;

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

    debug!(
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %lifecycle.amount,
        "Executing refundInEscrow on PaymentOperator"
    );

    let target = addrs.payment_operator.ok_or_else(|| {
        OperatorError::UnsupportedNetwork("no deployed PaymentOperator for this network".into())
    })?;

    let call = OperatorContract::refundInEscrowCall {
        paymentInfo: payment_info_abi,
        amount,
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
/// This prevents gas drain attacks where an attacker submits settlement requests
/// with arbitrary target addresses, causing the facilitator to send transactions
/// to random contracts and burn ETH on reverts.
fn validate_addresses(extra: &EscrowExtra, addrs: &OperatorAddresses) -> Result<(), OperatorError> {
    // Validate operator address
    if let Some(known_operator) = addrs.payment_operator {
        let client_target = extra.authorize_address.unwrap_or(extra.operator_address);
        if client_target != known_operator {
            return Err(OperatorError::PaymentInfoInvalid(format!(
                "operator address mismatch: client={:?}, expected={:?}",
                client_target, known_operator
            )));
        }
    } else {
        return Err(OperatorError::UnsupportedNetwork(
            "no deployed PaymentOperator for this network".into(),
        ));
    }

    // Validate token collector
    if extra.token_collector != addrs.token_collector {
        return Err(OperatorError::PaymentInfoInvalid(format!(
            "token_collector mismatch: client={:?}, expected={:?}",
            extra.token_collector, addrs.token_collector
        )));
    }

    // Validate escrow address
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
