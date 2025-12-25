//! x402r Escrow/Refund Extension Support
//!
//! Implements the x402r proposal for trustless refunds via escrow contracts.
//! This module enables payments to be routed through escrow proxies that hold
//! funds until release or refund conditions are met.
//!
//! See: <https://github.com/coinbase/x402/issues/864>
//!
//! # Architecture
//!
//! The x402r system uses:
//! - **DepositRelayFactory**: Deploys deterministic proxy contracts per merchant
//! - **DepositRelay**: Stateless implementation that proxies use via delegatecall
//! - **Escrow**: Shared contract that holds funds and manages disputes
//!
//! # Flow
//!
//! 1. Merchant registers with escrow and factory deploys a proxy
//! 2. Server marks routes as refundable, setting `payTo` to proxy address
//! 3. Client signs EIP-3009 authorization to the proxy address
//! 4. Facilitator detects `refund` extension and routes to escrow settlement
//! 5. Proxy receives tokens and forwards them to escrow with deposit record
//!
//! # Feature Flag
//!
//! Set `ENABLE_ESCROW=true` to enable escrow settlement support.

use alloy::primitives::{address, keccak256, Address, Bytes, FixedBytes, B256, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use crate::chain::evm::{EvmProvider, MetaEvmProvider, MetaTransaction};
use crate::chain::{FacilitatorLocalError, NetworkProvider};
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{EvmAddress, ExactPaymentPayload, MixedAddress, SettleResponse, TransactionHash};
use crate::types_v2::{
    PaymentPayloadV2, PaymentRequirementsV2, ResourceInfo, SettleRequestV2,
    X402rPayload, X402rPaymentPayloadNested,
};

// ============================================================================
// Contract Bindings
// ============================================================================

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    DepositRelay,
    "abi/DepositRelay.json"
);

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    DepositRelayFactory,
    "abi/DepositRelayFactory.json"
);

// ============================================================================
// Contract Addresses
// ============================================================================

/// CreateX universal deployer (same address on all EVM chains)
pub const CREATEX_ADDRESS: Address = address!("ba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed");

/// Contract addresses for Base Mainnet
pub mod base_mainnet {
    use super::*;

    pub const FACTORY: Address = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
    pub const ESCROW: Address = address!("C409e6da89E54253fbA86C1CE3E553d24E03f6bC");
    pub const IMPLEMENTATION: Address = address!("55eEC2951Da58118ebf32fD925A9bBB13096e828");
}

/// Contract addresses for Base Sepolia
pub mod base_sepolia {
    use super::*;

    pub const FACTORY: Address = address!("f981D813842eE78d18ef8ac825eef8e2C8A8BaC2");
    pub const ESCROW: Address = address!("F7F2Bc463d79Bd3E5Cb693944B422c39114De058");
    pub const IMPLEMENTATION: Address = address!("740785D15a77caCeE72De645f1bAeed880E2E99B");
}

/// Get factory address for a given network
pub fn factory_for_network(network: Network) -> Option<Address> {
    match network {
        Network::Base => Some(base_mainnet::FACTORY),
        Network::BaseSepolia => Some(base_sepolia::FACTORY),
        _ => None,
    }
}

/// Get escrow address for a given network
pub fn escrow_for_network(network: Network) -> Option<Address> {
    match network {
        Network::Base => Some(base_mainnet::ESCROW),
        Network::BaseSepolia => Some(base_sepolia::ESCROW),
        _ => None,
    }
}

// ============================================================================
// Types
// ============================================================================

/// x402r refund extension data from payment payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundExtension {
    /// Refund info containing factory and proxy mappings
    pub info: RefundInfo,
}

/// Refund extension info structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundInfo {
    /// Escrow factory contract address
    pub factory_address: Address,

    /// Map of proxy addresses to merchant payout addresses
    /// Key: proxy address (where client sends payment)
    /// Value: merchant payout address (final recipient after escrow)
    /// Note: x402r spec uses "merchantPayouts" as the field name
    pub merchant_payouts: HashMap<Address, Address>,
}

/// Raw escrow settle request matching Ali's SDK format
///
/// Ali's format has resource/accepted INSIDE paymentPayload, not at top level:
/// ```json
/// {
///   "x402Version": 2,
///   "paymentPayload": {
///     "x402Version": 2,
///     "resource": {...},
///     "accepted": {...},
///     "payload": { "authorization": {...}, "signature": "..." },
///     "extensions": { "refund": {...} }
///   },
///   "paymentRequirements": {...}
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowSettleRequestRaw {
    pub x402_version: u8,
    pub payment_payload: X402rPaymentPayloadNested,
    #[serde(default)]
    pub payment_requirements: Option<PaymentRequirementsV2>,
}

/// Parsed escrow settlement request
#[derive(Debug, Clone)]
pub struct EscrowSettleRequest {
    /// Original payment payload (for extracting signature data)
    pub payment_payload_v2: PaymentPayloadV2,

    /// Proxy address from payTo field
    pub proxy_address: Address,

    /// Merchant payout address from proxies map
    pub merchant_payout: Address,

    /// Factory address from extension
    pub factory_address: Address,

    /// Network for provider lookup
    pub network: Network,

    /// Payer address
    pub payer: Address,

    /// Payment amount
    pub amount: U256,

    /// EIP-3009 valid after timestamp
    pub valid_after: U256,

    /// EIP-3009 valid before timestamp
    pub valid_before: U256,

    /// EIP-3009 nonce
    pub nonce: FixedBytes<32>,

    /// Signature v component
    pub sig_v: u8,

    /// Signature r component
    pub sig_r: FixedBytes<32>,

    /// Signature s component
    pub sig_s: FixedBytes<32>,
}

// ============================================================================
// Errors
// ============================================================================

/// Errors that can occur during escrow operations
#[derive(Debug, Error)]
pub enum EscrowError {
    #[error("Escrow feature is disabled. Set ENABLE_ESCROW=true to enable.")]
    FeatureDisabled,

    #[error("Escrow refund extension requires x402 v2 protocol")]
    V1NotSupported,

    #[error("Missing 'refund' extension in payment payload")]
    MissingRefundExtension,

    #[error("Invalid refund extension format: {0}")]
    InvalidExtensionFormat(String),

    #[error("Invalid proxy address: expected {expected}, computed {computed}")]
    InvalidProxyAddress { expected: Address, computed: Address },

    #[error("Proxy {0} not found in refund extension proxies map")]
    UnknownProxy(Address),

    #[error("Network {0} does not support escrow (factory not deployed)")]
    UnsupportedNetwork(String),

    #[error("Only EVM networks support escrow settlement")]
    NonEvmNetwork,

    #[error("payTo address must be an EVM address for escrow")]
    NonEvmPayTo,

    #[error("Payer address must be an EVM address")]
    NonEvmPayer,

    #[error("Invalid EVM authorization payload")]
    InvalidEvmPayload,

    #[error("On-chain proxy verification failed: {0}")]
    ProxyVerificationFailed(String),

    #[error("Contract call failed: {0}")]
    ContractCall(String),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Provider not found for network: {0}")]
    ProviderNotFound(String),
}

impl From<EscrowError> for FacilitatorLocalError {
    fn from(err: EscrowError) -> Self {
        FacilitatorLocalError::Other(err.to_string())
    }
}

// ============================================================================
// Feature Flag
// ============================================================================

/// Check if escrow feature is enabled via environment variable
pub fn is_escrow_enabled() -> bool {
    env::var("ENABLE_ESCROW")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

// ============================================================================
// CREATE3 Address Computation (DEPRECATED - use on-chain verification instead)
// ============================================================================

/// Compute deterministic proxy address using CREATE3
///
/// **DEPRECATED**: Per Ali's recommendation, prefer querying the factory contract
/// via `getMerchantFromRelay()` instead of computing addresses locally.
/// Local computation is kept for reference/testing but is NOT used in settlement.
///
/// The address is computed as:
/// 1. salt = keccak256(factory || merchant)
/// 2. guardedSalt = keccak256(factory || salt) (CreateX salt guarding)
/// 3. proxy = CREATE3(CreateX, guardedSalt)
///
/// This matches the Solidity implementation in DepositRelayFactory.sol
#[deprecated(note = "Use verify_proxy_onchain() instead - query factory contract directly")]
pub fn compute_proxy_address(factory: Address, merchant_payout: Address) -> Address {
    // Step 1: Compute raw salt = keccak256(factory || merchant)
    // Per DepositRelayFactory.sol: salt = keccak256(abi.encodePacked(address(this), merchantPayout))
    let mut salt_input = Vec::with_capacity(40);
    salt_input.extend_from_slice(factory.as_slice());
    salt_input.extend_from_slice(merchant_payout.as_slice());
    let raw_salt = keccak256(&salt_input);

    debug!(
        factory = ?factory,
        merchant = ?merchant_payout,
        raw_salt = ?raw_salt,
        "Computing proxy address"
    );

    // Step 2: CreateX salt guarding - XOR with msg.sender (factory)
    // guardedSalt = keccak256(abi.encodePacked(msg.sender, salt))
    let mut guarded_input = Vec::with_capacity(52);
    guarded_input.extend_from_slice(factory.as_slice());
    guarded_input.extend_from_slice(raw_salt.as_slice());
    let guarded_salt = keccak256(&guarded_input);

    debug!(guarded_salt = ?guarded_salt, "Computed guarded salt");

    // Step 3: Compute CREATE3 address
    compute_create3_address(CREATEX_ADDRESS, guarded_salt)
}

/// Compute CREATE3 address from CreateX deployer
///
/// CREATE3 works by:
/// 1. CREATE2 deploys a minimal proxy at deterministic address
/// 2. That proxy uses CREATE (nonce=1) to deploy the actual contract
fn compute_create3_address(createx: Address, salt: B256) -> Address {
    // CREATE3 proxy init code hash (constant for CreateX)
    // This is keccak256 of the minimal CREATE3 proxy bytecode
    const CREATE3_PROXY_INITCODE_HASH: B256 =
        alloy::primitives::b256!("21c35dbe1b344a2488cf3321d6ce542f8e9f305544ff09e4993a62319a497c1f");

    // Step 1: Compute CREATE2 proxy address
    // proxy_addr = keccak256(0xff || createx || salt || initCodeHash)[12:]
    let mut create2_input = Vec::with_capacity(85);
    create2_input.push(0xff);
    create2_input.extend_from_slice(createx.as_slice());
    create2_input.extend_from_slice(salt.as_slice());
    create2_input.extend_from_slice(CREATE3_PROXY_INITCODE_HASH.as_slice());

    let proxy_hash = keccak256(&create2_input);
    let proxy_address = Address::from_slice(&proxy_hash[12..]);

    debug!(proxy_address = ?proxy_address, "Computed intermediate proxy address");

    // Step 2: Compute CREATE address from proxy (nonce = 1)
    // For nonce=1: RLP([address, 1]) = 0xd6 0x94 <20-byte-address> 0x01
    let mut rlp = Vec::with_capacity(23);
    rlp.push(0xd6);
    rlp.push(0x94);
    rlp.extend_from_slice(proxy_address.as_slice());
    rlp.push(0x01);

    let final_hash = keccak256(&rlp);
    let final_address = Address::from_slice(&final_hash[12..]);

    debug!(final_address = ?final_address, "Computed final CREATE3 address");

    final_address
}

// ============================================================================
// Parsing
// ============================================================================

/// Parse escrow settlement request from raw JSON body
///
/// Supports two formats:
/// 1. Ali's SDK format: resource/accepted nested inside paymentPayload
/// 2. Standard v2 format: resource/accepted at top level
#[instrument(skip(body), err)]
pub fn parse_escrow_request(body: &str) -> Result<EscrowSettleRequest, EscrowError> {
    // Try Ali's nested format first (resource/accepted inside paymentPayload)
    if let Ok(raw_req) = serde_json::from_str::<EscrowSettleRequestRaw>(body) {
        debug!("Parsed escrow request using nested format (Ali's SDK)");
        return parse_from_nested_format(raw_req);
    }

    // Fall back to standard v2 format (resource/accepted at top level)
    debug!("Trying standard v2 format for escrow request");
    let settle_req: SettleRequestV2 =
        serde_json::from_str(body).map_err(|e| EscrowError::Json(e))?;

    parse_from_v2_format(settle_req)
}

/// Parse escrow request from Ali's nested format
fn parse_from_nested_format(raw_req: EscrowSettleRequestRaw) -> Result<EscrowSettleRequest, EscrowError> {
    use alloy::hex;
    use std::str::FromStr;

    let inner = &raw_req.payment_payload;

    // Extract refund extension
    let refund_ext_value = inner
        .extensions
        .get("refund")
        .ok_or(EscrowError::MissingRefundExtension)?;

    let refund_ext: RefundExtension = serde_json::from_value(refund_ext_value.clone())
        .map_err(|e| EscrowError::InvalidExtensionFormat(e.to_string()))?;

    // Get proxy address from payTo (in accepted)
    let proxy_address: Address = match &inner.accepted.pay_to {
        MixedAddress::Evm(addr) => addr.0,
        _ => return Err(EscrowError::NonEvmPayTo),
    };

    // Lookup merchant from merchantPayouts map
    let merchant_payout = *refund_ext
        .info
        .merchant_payouts
        .get(&proxy_address)
        .ok_or(EscrowError::UnknownProxy(proxy_address))?;

    // Parse network from CAIP-2
    let network = Network::from_caip2(&inner.accepted.network.to_string())
        .ok_or_else(|| EscrowError::UnsupportedNetwork(inner.accepted.network.to_string()))?;

    // Parse authorization fields from X402rPayload strings
    let auth = &inner.payload.authorization;

    let payer = Address::from_str(&auth.from)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;
    let amount = U256::from_str(&auth.value)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;
    let valid_after = U256::from_str(&auth.valid_after)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;
    let valid_before = U256::from_str(&auth.valid_before)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;

    // Parse nonce (32 bytes hex)
    let nonce_str = auth.nonce.trim_start_matches("0x");
    let nonce_bytes = hex::decode(nonce_str)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;
    let nonce_array: [u8; 32] = nonce_bytes
        .try_into()
        .map_err(|_| EscrowError::InvalidEvmPayload)?;
    let nonce = FixedBytes(nonce_array);

    // Parse signature
    let sig_str = inner.payload.signature.trim_start_matches("0x");
    let sig_bytes = hex::decode(sig_str)
        .map_err(|_| EscrowError::InvalidEvmPayload)?;

    if sig_bytes.len() < 65 {
        return Err(EscrowError::InvalidEvmPayload);
    }

    let sig_r = FixedBytes::from_slice(&sig_bytes[0..32]);
    let sig_s = FixedBytes::from_slice(&sig_bytes[32..64]);
    let sig_v = sig_bytes[64];

    // Build a PaymentPayloadV2 for compatibility
    // (needed for some downstream functions)
    let resource = inner.resource.clone().unwrap_or_else(|| ResourceInfo {
        url: url::Url::parse("https://x402r.escrow/resource").unwrap(),
        description: "x402r escrow payment".to_string(),
        mime_type: "application/json".to_string(),
    });

    // Convert X402rPayload to ExactEvmPayload
    let evm_authorization = crate::types::ExactEvmPayloadAuthorization {
        from: EvmAddress(payer),
        to: EvmAddress(Address::from_str(&auth.to).map_err(|_| EscrowError::InvalidEvmPayload)?),
        value: crate::types::TokenAmount(amount),
        valid_after: crate::timestamp::UnixTimestamp(valid_after.try_into().unwrap_or(0)),
        valid_before: crate::timestamp::UnixTimestamp(valid_before.try_into().unwrap_or(u64::MAX)),
        nonce: crate::types::HexEncodedNonce(nonce_array),
    };
    let evm_payload = crate::types::ExactEvmPayload {
        authorization: evm_authorization,
        signature: crate::types::EvmSignature(sig_bytes),
    };

    let payment_payload_v2 = PaymentPayloadV2 {
        x402_version: 2,
        resource,
        accepted: inner.accepted.clone(),
        payload: ExactPaymentPayload::Evm(evm_payload),
        extensions: inner.extensions.clone(),
    };

    Ok(EscrowSettleRequest {
        payment_payload_v2,
        proxy_address,
        merchant_payout,
        factory_address: refund_ext.info.factory_address,
        network,
        payer,
        amount,
        valid_after,
        valid_before,
        nonce,
        sig_v,
        sig_r,
        sig_s,
    })
}

/// Parse escrow request from standard v2 format
fn parse_from_v2_format(settle_req: SettleRequestV2) -> Result<EscrowSettleRequest, EscrowError> {
    let payment_payload = settle_req.payment_payload;

    // Extract refund extension
    let refund_ext_value = payment_payload
        .extensions
        .get("refund")
        .ok_or(EscrowError::MissingRefundExtension)?;

    let refund_ext: RefundExtension = serde_json::from_value(refund_ext_value.clone())
        .map_err(|e| EscrowError::InvalidExtensionFormat(e.to_string()))?;

    // Get proxy address from payTo
    let proxy_address: Address = match &payment_payload.accepted.pay_to {
        MixedAddress::Evm(addr) => addr.0,
        _ => return Err(EscrowError::NonEvmPayTo),
    };

    // Lookup merchant from merchantPayouts map
    let merchant_payout = *refund_ext
        .info
        .merchant_payouts
        .get(&proxy_address)
        .ok_or(EscrowError::UnknownProxy(proxy_address))?;

    // Parse network from CAIP-2
    let network = Network::from_caip2(&payment_payload.accepted.network.to_string())
        .ok_or_else(|| EscrowError::UnsupportedNetwork(payment_payload.accepted.network.to_string()))?;

    // Extract EVM-specific payload data
    let (payer, amount, valid_after, valid_before, nonce, sig_v, sig_r, sig_s) =
        extract_evm_payload(&payment_payload)?;

    Ok(EscrowSettleRequest {
        payment_payload_v2: payment_payload,
        proxy_address,
        merchant_payout,
        factory_address: refund_ext.info.factory_address,
        network,
        payer,
        amount,
        valid_after,
        valid_before,
        nonce,
        sig_v,
        sig_r,
        sig_s,
    })
}

/// Extract EVM-specific data from payment payload
fn extract_evm_payload(
    payload: &PaymentPayloadV2,
) -> Result<(Address, U256, U256, U256, FixedBytes<32>, u8, FixedBytes<32>, FixedBytes<32>), EscrowError>
{
    // Get the EVM payload (signature + authorization)
    let evm_payload = match &payload.payload {
        crate::types::ExactPaymentPayload::Evm(evm_payload) => evm_payload,
        _ => return Err(EscrowError::InvalidEvmPayload),
    };

    let auth = &evm_payload.authorization;

    // Get payer address
    let payer: Address = auth.from.0;

    // Get amount
    let amount: U256 = auth.value.into();

    // Get timestamps
    let valid_after: U256 = auth.valid_after.into();
    let valid_before: U256 = auth.valid_before.into();

    // Get nonce
    let nonce: FixedBytes<32> = FixedBytes(auth.nonce.0);

    // Parse signature components from the evm_payload
    let signature = &evm_payload.signature;
    let sig_bytes = signature.0.as_slice();

    if sig_bytes.len() < 65 {
        return Err(EscrowError::InvalidEvmPayload);
    }

    // Standard signature format: r (32 bytes) || s (32 bytes) || v (1 byte)
    let sig_r = FixedBytes::from_slice(&sig_bytes[0..32]);
    let sig_s = FixedBytes::from_slice(&sig_bytes[32..64]);
    let sig_v = sig_bytes[64];

    Ok((payer, amount, valid_after, valid_before, nonce, sig_v, sig_r, sig_s))
}

// ============================================================================
// Verification
// ============================================================================

/// Verify proxy address matches deterministic computation
///
/// **DEPRECATED**: Per Ali's recommendation, prefer `verify_proxy_onchain()` which
/// queries the factory contract directly. Local CREATE3 computation is error-prone.
#[deprecated(note = "Use verify_proxy_onchain() instead")]
#[instrument(skip_all, err)]
#[allow(dead_code)]
pub fn verify_proxy_deterministic(request: &EscrowSettleRequest) -> Result<(), EscrowError> {
    let computed = compute_proxy_address(request.factory_address, request.merchant_payout);

    if computed != request.proxy_address {
        warn!(
            expected = ?computed,
            actual = ?request.proxy_address,
            factory = ?request.factory_address,
            merchant = ?request.merchant_payout,
            "Proxy address mismatch - possible attack or misconfiguration"
        );
        return Err(EscrowError::InvalidProxyAddress {
            expected: computed,
            computed: request.proxy_address,
        });
    }

    debug!(
        proxy = ?request.proxy_address,
        "Proxy address verified via deterministic computation"
    );

    Ok(())
}

/// Verify proxy is registered with factory (on-chain check)
#[instrument(skip(provider), err)]
pub async fn verify_proxy_onchain(
    request: &EscrowSettleRequest,
    provider: &EvmProvider,
) -> Result<(), EscrowError> {
    let factory_address = factory_for_network(request.network)
        .ok_or_else(|| EscrowError::UnsupportedNetwork(request.network.to_string()))?;

    // Create factory contract instance using the inner provider
    let factory = DepositRelayFactory::new(factory_address, provider.inner());

    // Call getMerchantFromRelay to verify the proxy is registered
    let call_result = factory
        .getMerchantFromRelay(request.proxy_address)
        .call()
        .await;

    let registered_merchant: Address = match call_result {
        Ok(result) => result,
        Err(e) => {
            return Err(EscrowError::ProxyVerificationFailed(format!(
                "Failed to query factory: {:?}",
                e
            )));
        }
    };

    // Check if merchant matches
    if registered_merchant == Address::ZERO {
        return Err(EscrowError::ProxyVerificationFailed(format!(
            "Proxy {} is not deployed",
            request.proxy_address
        )));
    }

    if registered_merchant != request.merchant_payout {
        return Err(EscrowError::ProxyVerificationFailed(format!(
            "Proxy merchant mismatch: expected {}, got {}",
            request.merchant_payout, registered_merchant
        )));
    }

    debug!(
        proxy = ?request.proxy_address,
        merchant = ?request.merchant_payout,
        "Proxy verified on-chain"
    );

    Ok(())
}

// ============================================================================
// Settlement
// ============================================================================

/// Main escrow settlement function
///
/// This is called from handlers.rs when a refund extension is detected.
/// It routes the payment through the escrow proxy instead of direct transfer.
///
/// The `facilitator` parameter must implement `HasProviderMap` to provide
/// access to the network-specific providers needed for settlement.
#[instrument(skip_all, err, fields(network))]
pub async fn settle_with_escrow<F>(body: &str, facilitator: &F) -> Result<SettleResponse, EscrowError>
where
    F: HasProviderMap,
    F::Map: ProviderMap<Value = NetworkProvider>,
{
    // Get the provider map from the facilitator
    let provider_map = facilitator.provider_map();

    // Check feature flag
    if !is_escrow_enabled() {
        return Err(EscrowError::FeatureDisabled);
    }

    // Parse request
    let request = parse_escrow_request(body)?;

    info!(
        proxy = ?request.proxy_address,
        merchant = ?request.merchant_payout,
        factory = ?request.factory_address,
        network = %request.network,
        amount = %request.amount,
        payer = ?request.payer,
        "Processing x402r escrow settlement"
    );

    // Verify factory is supported on this network
    if factory_for_network(request.network).is_none() {
        return Err(EscrowError::UnsupportedNetwork(request.network.to_string()));
    }

    // Get provider
    let network_provider = provider_map
        .by_network(&request.network)
        .ok_or_else(|| EscrowError::ProviderNotFound(request.network.to_string()))?;

    // Extract EVM provider
    let evm_provider = match network_provider {
        NetworkProvider::Evm(provider) => provider,
        _ => return Err(EscrowError::NonEvmNetwork),
    };

    // Verify proxy on-chain (PRIMARY verification - query the factory contract)
    // Per Ali's recommendation: Don't compute CREATE3 locally, get it from the factory.
    // This is more reliable and avoids potential math errors.
    verify_proxy_onchain(&request, evm_provider).await?;

    // Skip deterministic verification - on-chain check is authoritative
    // The factory's getMerchantFromRelay() confirms:
    // 1. The proxy is deployed
    // 2. The merchant mapping is correct
    debug!(
        proxy = ?request.proxy_address,
        merchant = ?request.merchant_payout,
        "Proxy verified via factory contract (skipping local CREATE3 computation)"
    );

    // Execute deposit on proxy contract
    let tx_hash = execute_escrow_deposit(&request, evm_provider).await?;

    info!(
        tx_hash = ?tx_hash,
        proxy = ?request.proxy_address,
        "Escrow settlement transaction submitted"
    );

    // Convert B256 to [u8; 32] for TransactionHash::Evm
    let tx_hash_bytes: [u8; 32] = tx_hash.into();

    Ok(SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Evm(EvmAddress(request.payer)),
        transaction: Some(TransactionHash::Evm(tx_hash_bytes)),
        network: request.network,
    })
}

/// Execute deposit on the escrow proxy contract
#[instrument(skip(provider), err)]
async fn execute_escrow_deposit(
    request: &EscrowSettleRequest,
    provider: &EvmProvider,
) -> Result<B256, EscrowError> {
    // Build the executeDeposit call data using the generated bindings
    let call = DepositRelay::executeDepositCall {
        fromUser: request.payer,
        amount: request.amount,
        validAfter: request.valid_after,
        validBefore: request.valid_before,
        nonce: request.nonce,
        v: request.sig_v,
        r: request.sig_r,
        s: request.sig_s,
    };

    // ABI-encode the call
    let calldata = call.abi_encode();

    debug!(
        proxy = ?request.proxy_address,
        payer = ?request.payer,
        amount = %request.amount,
        calldata_len = calldata.len(),
        "Calling executeDeposit on proxy"
    );

    // Create meta transaction
    let meta_tx = MetaTransaction {
        to: request.proxy_address,
        calldata: Bytes::from(calldata),
        confirmations: 1,
    };

    // Send transaction using provider's send_transaction
    let receipt = provider
        .send_transaction(meta_tx)
        .await
        .map_err(|e| EscrowError::ContractCall(format!("{:?}", e)))?;

    Ok(receipt.transaction_hash)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_proxy_address_deterministic() {
        let factory = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
        let merchant = address!("1234567890123456789012345678901234567890");

        let addr1 = compute_proxy_address(factory, merchant);
        let addr2 = compute_proxy_address(factory, merchant);

        assert_eq!(addr1, addr2, "Address computation should be deterministic");
    }

    #[test]
    fn test_compute_proxy_address_different_inputs() {
        let factory = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
        let merchant1 = address!("1111111111111111111111111111111111111111");
        let merchant2 = address!("2222222222222222222222222222222222222222");

        let addr1 = compute_proxy_address(factory, merchant1);
        let addr2 = compute_proxy_address(factory, merchant2);

        assert_ne!(addr1, addr2, "Different merchants should produce different addresses");
    }

    #[test]
    fn test_factory_for_network() {
        assert_eq!(
            factory_for_network(Network::Base),
            Some(base_mainnet::FACTORY)
        );
        assert_eq!(
            factory_for_network(Network::BaseSepolia),
            Some(base_sepolia::FACTORY)
        );
        assert_eq!(factory_for_network(Network::Avalanche), None);
    }

    #[test]
    fn test_is_escrow_enabled() {
        // Default should be disabled
        env::remove_var("ENABLE_ESCROW");
        assert!(!is_escrow_enabled());

        // Test enabling
        env::set_var("ENABLE_ESCROW", "true");
        assert!(is_escrow_enabled());

        env::set_var("ENABLE_ESCROW", "TRUE");
        assert!(is_escrow_enabled());

        env::set_var("ENABLE_ESCROW", "1");
        assert!(is_escrow_enabled());

        // Test disabling
        env::set_var("ENABLE_ESCROW", "false");
        assert!(!is_escrow_enabled());

        env::set_var("ENABLE_ESCROW", "0");
        assert!(!is_escrow_enabled());

        // Cleanup
        env::remove_var("ENABLE_ESCROW");
    }

    #[test]
    fn test_parse_refund_extension() {
        let json = r#"{
            "info": {
                "factoryAddress": "0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814",
                "merchantPayouts": {
                    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                }
            }
        }"#;

        let ext: RefundExtension = serde_json::from_str(json).unwrap();
        assert_eq!(
            ext.info.factory_address,
            address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814")
        );
        assert_eq!(ext.info.merchant_payouts.len(), 1);
    }

    #[test]
    fn test_create3_address_computation() {
        // Test that CREATE3 computation follows the expected pattern
        let deployer = CREATEX_ADDRESS;
        let salt = B256::from_slice(&[1u8; 32]);

        let addr = compute_create3_address(deployer, salt);

        // Address should be 20 bytes and non-zero
        assert_ne!(addr, Address::ZERO);
    }
}
