//! PaymentOperator settlement logic
//!
//! This module handles settlement requests with the "operator" extension,
//! routing them to the appropriate PaymentOperator contract functions.

use alloy::hex;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::sol_types::SolCall;
use tracing::{debug, info, instrument};

use crate::chain::evm::{EvmProvider, MetaEvmProvider, MetaTransaction};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{EvmAddress, MixedAddress, SettleResponse, TransactionHash};

use super::abi::{OperatorContract, PaymentInfo as PaymentInfoContract};
use super::addresses::OperatorAddresses;
use super::errors::OperatorError;
use super::types::{EscrowState, OperatorAction, OperatorExtension};

/// Main PaymentOperator settlement function
///
/// This is called from handlers.rs when an "operator" extension is detected.
/// It routes the payment through the PaymentOperator contract based on the action.
#[instrument(skip_all, err, fields(network))]
pub async fn settle_with_operator<F>(
    body: &str,
    facilitator: &F,
) -> Result<SettleResponse, OperatorError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    // Check feature flag
    if !super::is_enabled() {
        return Err(OperatorError::FeatureDisabled);
    }

    // Parse the request
    let (network, extension) = parse_operator_request(body)?;

    info!(
        action = ?extension.action,
        network = %network,
        operator = ?extension.payment_info.operator,
        payer = ?extension.payment_info.payer,
        receiver = ?extension.payment_info.receiver,
        amount = %extension.amount,
        "Processing PaymentOperator settlement"
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

    // Execute the action
    let tx_hash = execute_operator_action(&extension, &addrs, evm_provider).await?;

    info!(
        tx_hash = ?tx_hash,
        action = ?extension.action,
        "PaymentOperator settlement transaction submitted"
    );

    // Convert B256 to [u8; 32] for TransactionHash::Evm
    let tx_hash_bytes: [u8; 32] = tx_hash.into();

    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(extension.payment_info.payer)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network,
        proof_of_payment: None,
    })
}

/// Parse operator extension from request body
fn parse_operator_request(body: &str) -> Result<(Network, OperatorExtension), OperatorError> {
    let json_value: serde_json::Value = serde_json::from_str(body)?;

    // Extract network from accepted field
    let network_str = json_value
        .get("paymentPayload")
        .and_then(|pp| pp.get("accepted"))
        .and_then(|acc| acc.get("network"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| OperatorError::MissingField("paymentPayload.accepted.network".to_string()))?;

    let network = Network::from_caip2(network_str)
        .ok_or_else(|| OperatorError::UnsupportedNetwork(network_str.to_string()))?;

    // Extract operator extension
    let operator_ext_value = json_value
        .get("paymentPayload")
        .and_then(|pp| pp.get("extensions"))
        .and_then(|ext| ext.get("operator"))
        .ok_or(OperatorError::MissingOperatorExtension)?;

    let extension: OperatorExtension = serde_json::from_value(operator_ext_value.clone())
        .map_err(|e| OperatorError::InvalidExtensionFormat(e.to_string()))?;

    Ok((network, extension))
}

/// Execute the appropriate operator action
async fn execute_operator_action(
    extension: &OperatorExtension,
    addrs: &OperatorAddresses,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    let payment_info = extension.payment_info.to_contract_type();
    let amount = U256::from(extension.amount);

    // Get token collector and data (default to ERC3009 collector with empty data)
    let token_collector = extension
        .token_collector
        .unwrap_or(addrs.erc3009_collector);

    let collector_data = extension
        .collector_data
        .as_ref()
        .map(|s| {
            if s.starts_with("0x") {
                hex::decode(&s[2..]).unwrap_or_default()
            } else {
                hex::decode(s).unwrap_or_default()
            }
        })
        .unwrap_or_default();

    match extension.action {
        OperatorAction::Authorize => {
            execute_authorize(
                &payment_info,
                amount,
                token_collector,
                &collector_data,
                provider,
            )
            .await
        }
        OperatorAction::Charge => {
            execute_charge(
                &payment_info,
                amount,
                token_collector,
                &collector_data,
                provider,
            )
            .await
        }
        OperatorAction::Release => {
            execute_release(&payment_info, amount, provider).await
        }
        OperatorAction::RefundInEscrow => {
            execute_refund_in_escrow(&payment_info, extension.amount, provider).await
        }
        OperatorAction::RefundPostEscrow => {
            execute_refund_post_escrow(
                &payment_info,
                amount,
                token_collector,
                &collector_data,
                provider,
            )
            .await
        }
    }
}

/// Execute authorize action on PaymentOperator
#[instrument(skip_all, err)]
async fn execute_authorize(
    payment_info: &PaymentInfoContract,
    amount: U256,
    token_collector: Address,
    collector_data: &[u8],
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        amount = %amount,
        "Executing authorize on PaymentOperator"
    );

    let call = OperatorContract::authorizeCall {
        paymentInfo: payment_info.clone(),
        amount,
        tokenCollector: token_collector,
        collectorData: Bytes::from(collector_data.to_vec()),
    };

    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: payment_info.operator,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Execute charge action on PaymentOperator
#[instrument(skip_all, err)]
async fn execute_charge(
    payment_info: &PaymentInfoContract,
    amount: U256,
    token_collector: Address,
    collector_data: &[u8],
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        amount = %amount,
        "Executing charge on PaymentOperator"
    );

    let call = OperatorContract::chargeCall {
        paymentInfo: payment_info.clone(),
        amount,
        tokenCollector: token_collector,
        collectorData: Bytes::from(collector_data.to_vec()),
    };

    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: payment_info.operator,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Execute release action on PaymentOperator
#[instrument(skip_all, err)]
async fn execute_release(
    payment_info: &PaymentInfoContract,
    amount: U256,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    debug!(
        operator = ?payment_info.operator,
        receiver = ?payment_info.receiver,
        amount = %amount,
        "Executing release on PaymentOperator"
    );

    let call = OperatorContract::releaseCall {
        paymentInfo: payment_info.clone(),
        amount,
    };

    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: payment_info.operator,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Execute refundInEscrow action on PaymentOperator
#[instrument(skip_all, err)]
async fn execute_refund_in_escrow(
    payment_info: &PaymentInfoContract,
    amount: u128,
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    use alloy::primitives::Uint;

    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        amount = %amount,
        "Executing refundInEscrow on PaymentOperator"
    );

    // Convert u128 to Uint<120, 2> for the contract call
    let amount_uint120: Uint<120, 2> = Uint::from(amount);

    let call = OperatorContract::refundInEscrowCall {
        paymentInfo: payment_info.clone(),
        amount: amount_uint120,
    };

    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: payment_info.operator,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Execute refundPostEscrow action on PaymentOperator
#[instrument(skip_all, err)]
async fn execute_refund_post_escrow(
    payment_info: &PaymentInfoContract,
    amount: U256,
    token_collector: Address,
    collector_data: &[u8],
    provider: &EvmProvider,
) -> Result<B256, OperatorError> {
    debug!(
        operator = ?payment_info.operator,
        payer = ?payment_info.payer,
        amount = %amount,
        "Executing refundPostEscrow on PaymentOperator"
    );

    let call = OperatorContract::refundPostEscrowCall {
        paymentInfo: payment_info.clone(),
        amount,
        tokenCollector: token_collector,
        collectorData: Bytes::from(collector_data.to_vec()),
    };

    let calldata = call.abi_encode();

    let meta_tx = MetaTransaction {
        to: payment_info.operator,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

/// Query escrow state for a payment
///
/// Note: This function is not yet implemented as it requires eth_call support
/// for view functions. It's a placeholder for future implementation.
#[instrument(skip_all, err)]
#[allow(dead_code)]
pub async fn query_escrow_state(
    _payment_info: &PaymentInfoContract,
    escrow_address: Address,
    _provider: &EvmProvider,
) -> Result<EscrowState, OperatorError> {
    debug!(
        escrow = ?escrow_address,
        "Querying escrow state"
    );

    // Note: For view calls, we'd need to use eth_call instead of send_transaction
    // For now, this function is a placeholder - actual implementation would need
    // the provider to support eth_call for view functions
    Err(OperatorError::EscrowStateQuery("View calls not yet implemented".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_parse_operator_request() {
        let body = r#"{
            "x402Version": 2,
            "paymentPayload": {
                "accepted": {
                    "network": "eip155:84532"
                },
                "extensions": {
                    "operator": {
                        "action": "authorize",
                        "paymentInfo": {
                            "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                            "payer": "0x1111111111111111111111111111111111111111",
                            "receiver": "0x2222222222222222222222222222222222222222",
                            "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                            "maxAmount": "1000000",
                            "preApprovalExpiry": "1738400000",
                            "authorizationExpiry": "1738500000",
                            "refundExpiry": "1738600000",
                            "minFeeBps": 0,
                            "maxFeeBps": 100,
                            "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                            "salt": "12345"
                        },
                        "amount": "500000"
                    }
                }
            }
        }"#;

        let (network, extension) = parse_operator_request(body).unwrap();

        assert_eq!(network, Network::BaseSepolia);
        assert_eq!(extension.action, OperatorAction::Authorize);
        assert_eq!(extension.amount, 500000);
        assert_eq!(
            extension.payment_info.operator,
            address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70")
        );
    }
}
