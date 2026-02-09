//! Escrow Scheme settlement logic
//!
//! This module handles settlement requests with scheme="escrow",
//! calling PaymentOperator.authorize() to place funds in escrow.
//!
//! Based on reference implementation:
//! https://github.com/BackTrackCo/x402r-scheme/tree/main/packages/evm/src/escrow/facilitator

use alloy::primitives::{Bytes, B256, U256};
use alloy::sol_types::SolCall;
use tracing::{debug, info, instrument};

use crate::chain::evm::{EvmProvider, MetaEvmProvider, MetaTransaction};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{EvmAddress, MixedAddress, SettleResponse, TransactionHash};

use super::abi::OperatorContract;
use super::addresses::OperatorAddresses;
use super::errors::OperatorError;
use super::types::{ContractPaymentInfo, EscrowExtra, EscrowPayload};

/// Escrow scheme identifier
pub const ESCROW_SCHEME: &str = "escrow";

/// Main escrow settlement function
///
/// This is called from handlers.rs when scheme="escrow" is detected.
/// It calls PaymentOperator.authorize() to place funds in escrow.
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

    // Parse the request
    let (network, escrow_payload, extra) = parse_escrow_request(body)?;

    info!(
        network = %network,
        payer = ?escrow_payload.authorization.from,
        receiver = ?escrow_payload.payment_info.receiver,
        amount = %escrow_payload.authorization.value,
        "Processing escrow scheme settlement (authorize)"
    );

    // Verify network is supported
    let addrs = OperatorAddresses::for_network(network)
        .ok_or_else(|| OperatorError::unsupported_network(&network))?;

    // Get provider
    let provider_map = facilitator.provider_map();
    let network_provider = provider_map
        .by_network(&network)
        .ok_or_else(|| OperatorError::ProviderNotFound(network.to_string()))?;

    // Extract EVM provider
    let evm_provider = match network_provider {
        NetworkProvider::Evm(provider) => provider,
        _ => return Err(OperatorError::NonEvmNetwork),
    };

    // Execute authorize
    let tx_hash = execute_authorize(&escrow_payload, &extra, &addrs, evm_provider).await?;

    info!(
        tx_hash = ?tx_hash,
        "Escrow authorize transaction submitted"
    );

    // Convert B256 to [u8; 32] for TransactionHash::Evm
    let tx_hash_bytes: [u8; 32] = tx_hash.into();

    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(escrow_payload.authorization.from)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network,
        proof_of_payment: None,
    })
}

/// Parse escrow scheme request from body
fn parse_escrow_request(
    body: &str,
) -> Result<(Network, EscrowPayload, EscrowExtra), OperatorError> {
    let json_value: serde_json::Value = serde_json::from_str(body)?;

    // Verify scheme is "escrow"
    let scheme = json_value
        .get("scheme")
        .and_then(|s| s.as_str())
        .ok_or_else(|| OperatorError::MissingField("scheme".to_string()))?;

    if scheme != ESCROW_SCHEME {
        return Err(OperatorError::InvalidScheme(scheme.to_string()));
    }

    // Extract network from paymentRequirements
    let network_str = json_value
        .get("paymentRequirements")
        .and_then(|pr| pr.get("network"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| OperatorError::MissingField("paymentRequirements.network".to_string()))?;

    let network = Network::from_caip2(network_str)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(network_str.to_string()))?;

    // Extract escrow payload
    let payload_value = json_value
        .get("payload")
        .ok_or_else(|| OperatorError::MissingField("payload".to_string()))?;

    let escrow_payload: EscrowPayload = serde_json::from_value(payload_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(e.to_string()))?;

    // Extract extra from paymentRequirements
    let extra_value = json_value
        .get("paymentRequirements")
        .and_then(|pr| pr.get("extra"))
        .ok_or_else(|| OperatorError::MissingField("paymentRequirements.extra".to_string()))?;

    let extra: EscrowExtra = serde_json::from_value(extra_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(format!("Invalid extra: {}", e)))?;

    Ok((network, escrow_payload, extra))
}

/// Execute authorize on PaymentOperator
///
/// This is the ONLY action the facilitator performs for escrow scheme.
/// Other actions (charge, release, refunds) are handled by other systems.
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
    // SECURITY: Validate client-provided addresses match known deployments.
    // Without this check, an attacker could specify arbitrary target addresses,
    // causing the facilitator to send transactions to random contracts and burn gas.
    validate_addresses(extra, addrs)?;

    // Build PaymentInfo from escrow payload
    let payment_info = ContractPaymentInfo::from_escrow_payload(escrow_payload);
    let payment_info_abi = payment_info.to_abi_type();

    let amount = U256::from(escrow_payload.authorization.value);

    // Use hardcoded token collector address (validated above)
    let token_collector = addrs.token_collector;

    // collectorData is the raw ERC-3009 signature bytes.
    // The TokenCollector passes this through _handleERC6492Signature()
    // and then to USDC.receiveWithAuthorization() as the signature parameter.
    let collector_data = encode_collector_data(&escrow_payload.signature);

    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        receiver = ?payment_info.receiver,
        amount = %amount,
        token_collector = ?token_collector,
        "Executing authorize on PaymentOperator"
    );

    // Use hardcoded operator address (validated above)
    let target = addrs.payment_operator.ok_or_else(|| {
        OperatorError::UnsupportedNetwork("no deployed PaymentOperator for this network".into())
    })?;

    let call = OperatorContract::authorizeCall {
        paymentInfo: payment_info_abi,
        amount,
        tokenCollector: token_collector,
        collectorData: collector_data,
    };

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
///
/// The collectorData is the raw ERC-3009 signature bytes. The TokenCollector
/// passes this through `_handleERC6492Signature()` and then to
/// `USDC.receiveWithAuthorization()` as the signature parameter.
fn encode_collector_data(signature: &Bytes) -> Bytes {
    // Pass the signature directly - the TokenCollector handles it
    signature.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_escrow_request() {
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

        let (network, payload, extra) = parse_escrow_request(body).unwrap();

        assert_eq!(network, Network::BaseSepolia);
        assert_eq!(payload.authorization.value, 1_000_000);
        assert_eq!(
            extra.operator_address,
            alloy::primitives::address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70")
        );
    }

    #[test]
    fn test_encode_collector_data() {
        let signature = Bytes::from(vec![0xab, 0xcd, 0xef]);
        let encoded = encode_collector_data(&signature);

        // Raw signature bytes passed directly to TokenCollector
        assert_eq!(encoded, signature);
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
}
