//! Sui blockchain payment verification and settlement.
//!
//! This module implements x402 payment flows for the Sui blockchain using
//! sponsored transactions. Sui provides protocol-level gas sponsorship,
//! allowing the facilitator to pay gas fees while users pay only the
//! stablecoin transfer amount.
//!
//! # Sponsored Transaction Flow
//!
//! 1. Client constructs a USDC transfer transaction
//! 2. Client signs the transaction with their Sui wallet
//! 3. Client sends transaction bytes + signature to facilitator
//! 4. Facilitator verifies the transaction parameters
//! 5. Facilitator adds gas sponsorship and co-signs
//! 6. Facilitator submits the sponsored transaction to Sui network
//!
//! # USDC on Sui
//!
//! Sui USDC uses 6 decimals (same as EVM chains):
//! - Mainnet: `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC`
//! - Testnet: `0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC`

use std::str::FromStr;

use sui_sdk::SuiClientBuilder;
use sui_sdk::rpc_types::SuiTransactionBlockResponseOptions;
use sui_types::base_types::SuiAddress;
use sui_types::crypto::{EncodeDecodeBase64, SuiKeyPair, Signature, SuiSignature, ToFromBytes};
use sui_types::transaction::{TransactionData, Transaction, TransactionDataAPI};
use shared_crypto::intent::{Intent, IntentMessage};
use tracing::{debug, error, info, warn};

use crate::chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::from_env::{
    ENV_RPC_SUI, ENV_RPC_SUI_TESTNET, ENV_SUI_PRIVATE_KEY, ENV_SUI_PRIVATE_KEY_MAINNET,
    ENV_SUI_PRIVATE_KEY_TESTNET,
};
use crate::network::Network;
use crate::types::{
    ExactPaymentPayload, ExactSuiPayload, MixedAddress, Scheme, SettleRequest, SettleResponse,
    SupportedPaymentKind, SupportedPaymentKindExtra, SupportedPaymentKindsResponse, VerifyRequest,
    VerifyResponse, X402Version,
};

/// USDC coin type on Sui mainnet
pub const USDC_COIN_TYPE_MAINNET: &str =
    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

/// USDC coin type on Sui testnet
pub const USDC_COIN_TYPE_TESTNET: &str =
    "0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC";

/// Sui network provider for payment verification and settlement.
///
/// Handles sponsored transactions for gasless USDC payments on Sui.
pub struct SuiProvider {
    /// The network this provider is configured for (Sui mainnet or testnet).
    network: Network,
    /// RPC endpoint URL for Sui network.
    rpc_url: String,
    /// Facilitator's Sui address (pays gas fees).
    signer_address: SuiAddress,
    /// Facilitator's keypair for signing sponsored transactions.
    keypair: SuiKeyPair,
    /// USDC coin type for this network.
    usdc_coin_type: String,
}

impl SuiProvider {
    /// Create a new Sui provider with the given configuration.
    pub fn new(
        network: Network,
        rpc_url: String,
        signer_address: SuiAddress,
        keypair: SuiKeyPair,
    ) -> Self {
        let usdc_coin_type = match network {
            Network::Sui => USDC_COIN_TYPE_MAINNET.to_string(),
            Network::SuiTestnet => USDC_COIN_TYPE_TESTNET.to_string(),
            _ => USDC_COIN_TYPE_TESTNET.to_string(), // Default to testnet
        };

        Self {
            network,
            rpc_url,
            signer_address,
            keypair,
            usdc_coin_type,
        }
    }

    /// Extract the Sui payload from a verify/settle request.
    fn extract_payload<'a>(
        &self,
        request: &'a VerifyRequest,
    ) -> Result<&'a ExactSuiPayload, FacilitatorLocalError> {
        match &request.payment_payload.payload {
            ExactPaymentPayload::Sui(payload) => Ok(payload),
            _ => Err(FacilitatorLocalError::DecodingError(
                "Expected Sui payload".to_string(),
            )),
        }
    }

    /// Parse a Sui address from string.
    fn parse_address(addr: &str) -> Result<SuiAddress, FacilitatorLocalError> {
        SuiAddress::from_str(addr).map_err(|e| {
            FacilitatorLocalError::InvalidAddress(format!("Invalid Sui address '{}': {}", addr, e))
        })
    }

    /// Decode base64 transaction bytes.
    fn decode_transaction_bytes(
        &self,
        tx_bytes_base64: &str,
    ) -> Result<TransactionData, FacilitatorLocalError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let tx_bytes = STANDARD.decode(tx_bytes_base64).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to decode base64 transaction bytes: {}",
                e
            ))
        })?;

        bcs::from_bytes(&tx_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to deserialize BCS transaction data: {}",
                e
            ))
        })
    }

    /// Decode base64 signature.
    fn decode_signature(&self, sig_base64: &str) -> Result<Signature, FacilitatorLocalError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let sig_bytes = STANDARD.decode(sig_base64).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to decode base64 signature: {}",
                e
            ))
        })?;

        Signature::from_bytes(&sig_bytes).map_err(|e| {
            FacilitatorLocalError::DecodingError(format!(
                "Failed to parse Sui signature: {}",
                e
            ))
        })
    }

    /// Verify the sender's signature on the transaction.
    ///
    /// This extracts the public key from the signature and verifies it matches
    /// the expected sender address. Full signature verification is performed
    /// by the Sui network on transaction submission.
    fn verify_signature(
        &self,
        _tx_data: &TransactionData,
        signature: &Signature,
        expected_sender: &SuiAddress,
    ) -> Result<(), FacilitatorLocalError> {
        use sui_types::crypto::PublicKey;

        // Extract the public key from the signature
        // Sui signatures contain: [scheme_flag | signature | public_key]
        let sig_bytes = signature.as_ref();
        if sig_bytes.is_empty() {
            return Err(FacilitatorLocalError::InvalidSignature(
                MixedAddress::Sui(expected_sender.to_string()),
                "Empty signature".to_string(),
            ));
        }

        // Get the public key from the signature and derive the address
        let public_key = PublicKey::try_from_bytes(signature.scheme(), signature.public_key_bytes())
            .map_err(|e| {
                FacilitatorLocalError::InvalidSignature(
                    MixedAddress::Sui(expected_sender.to_string()),
                    format!("Failed to extract public key from signature: {}", e),
                )
            })?;

        let signer_address = SuiAddress::from(&public_key);

        // Verify the signer matches the expected sender
        if signer_address != *expected_sender {
            return Err(FacilitatorLocalError::InvalidSignature(
                MixedAddress::Sui(expected_sender.to_string()),
                format!(
                    "Signature signer {} does not match expected sender {}",
                    signer_address, expected_sender
                ),
            ));
        }

        debug!(
            sender = %expected_sender,
            "Sui transaction signature verified - signer matches sender"
        );

        Ok(())
    }

    /// Verify transaction parameters match payment requirements.
    async fn verify_transaction(
        &self,
        payload: &ExactSuiPayload,
        request: &VerifyRequest,
    ) -> Result<(TransactionData, Signature, SuiAddress), FacilitatorLocalError> {
        let payer_addr = Self::parse_address(&payload.from)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Verify network matches
        if request.payment_payload.network != self.network {
            return Err(FacilitatorLocalError::NetworkMismatch(
                Some(payer),
                self.network,
                request.payment_payload.network,
            ));
        }

        // Verify scheme is exact
        if request.payment_payload.scheme != Scheme::Exact {
            return Err(FacilitatorLocalError::SchemeMismatch(
                Some(payer.clone()),
                Scheme::Exact,
                request.payment_payload.scheme,
            ));
        }

        // Verify recipient matches payment requirements
        let expected_recipient = request.payment_requirements.pay_to.to_string();
        if payload.to.to_lowercase() != expected_recipient.to_lowercase() {
            return Err(FacilitatorLocalError::ReceiverMismatch(
                payer.clone(),
                payload.to.clone(),
                expected_recipient,
            ));
        }

        // Parse amount - Sui USDC uses u64 amounts (fits in 6 decimal precision)
        let payload_amount: u64 = payload.amount.parse().map_err(|e| {
            FacilitatorLocalError::DecodingError(format!("Invalid amount '{}': {}", payload.amount, e))
        })?;

        // Verify amount meets minimum (TokenAmount wraps U256)
        let required_amount = request.payment_requirements.max_amount_required.0;
        let payload_amount_u256 = alloy::primitives::U256::from(payload_amount);
        if payload_amount_u256 < required_amount {
            return Err(FacilitatorLocalError::InsufficientValue(payer.clone()));
        }

        // Decode and verify transaction
        let tx_data = self.decode_transaction_bytes(&payload.transaction_bytes)?;
        let signature = self.decode_signature(&payload.sender_signature)?;

        // Verify sender matches transaction sender
        let tx_sender = tx_data.sender();
        if tx_sender != payer_addr {
            return Err(FacilitatorLocalError::InvalidSignature(
                payer.clone(),
                format!(
                    "Transaction sender {} does not match payload sender {}",
                    tx_sender, payer_addr
                ),
            ));
        }

        // Verify signature
        self.verify_signature(&tx_data, &signature, &payer_addr)?;

        info!(
            network = %self.network,
            from = %payload.from,
            to = %payload.to,
            amount = payload_amount,
            "Sui payment verification passed"
        );

        Ok((tx_data, signature, payer_addr))
    }

    /// Check USDC balance for the sender.
    async fn check_balance(
        &self,
        address: &SuiAddress,
        required_amount: u64,
    ) -> Result<(), FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // Get all USDC coins owned by the address
        let coins = client
            .coin_read_api()
            .get_coins(*address, Some(self.usdc_coin_type.clone()), None, None)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to fetch USDC balance: {}", e))
            })?;

        let total_balance: u64 = coins.data.iter().map(|c| c.balance).sum();

        if total_balance < required_amount {
            return Err(FacilitatorLocalError::InsufficientFunds(
                MixedAddress::Sui(address.to_string()),
            ));
        }

        debug!(
            address = %address,
            balance = total_balance,
            required = required_amount,
            "Sui USDC balance check passed"
        );

        Ok(())
    }

    /// Submit a sponsored transaction to the network.
    ///
    /// Sui sponsored transactions require TWO signatures:
    /// 1. sender_signature - User's signature authorizing the transfer
    /// 2. sponsor_signature - Facilitator's signature authorizing gas payment
    async fn submit_sponsored_transaction(
        &self,
        tx_data: TransactionData,
        sender_signature: Signature,
        sender: SuiAddress,
    ) -> Result<String, FacilitatorLocalError> {
        let client = SuiClientBuilder::default()
            .build(&self.rpc_url)
            .await
            .map_err(|e| {
                FacilitatorLocalError::ContractCall(format!("Failed to connect to Sui RPC: {}", e))
            })?;

        // For sponsored transactions, we need BOTH signatures:
        // 1. sender_signature - already provided by the user
        // 2. sponsor_signature - facilitator signs as gas owner

        // Sign the transaction data with facilitator's key (gas sponsor)
        let intent_msg = IntentMessage::new(Intent::sui_transaction(), tx_data.clone());
        let sponsor_signature = Signature::new_secure(&intent_msg, &self.keypair);

        debug!(
            sender = %sender,
            sponsor = %self.signer_address,
            "Creating sponsored transaction with dual signatures"
        );

        // Create the transaction with BOTH signatures
        // Order matters: sender signature first, then sponsor signature
        // Convert Signature to GenericSignature for the Transaction constructor
        let sender_sig = sui_types::signature::GenericSignature::Signature(sender_signature);
        let sponsor_sig = sui_types::signature::GenericSignature::Signature(sponsor_signature);

        let transaction = Transaction::from_generic_sig_data(
            tx_data,
            vec![sender_sig, sponsor_sig],
        );

        // Execute the transaction
        let response = client
            .quorum_driver_api()
            .execute_transaction_block(
                transaction,
                SuiTransactionBlockResponseOptions::full_content(),
                None,
            )
            .await
            .map_err(|e| {
                FacilitatorLocalError::Other(format!(
                    "Failed to execute Sui transaction: {}",
                    e
                ))
            })?;

        let digest = response.digest.to_string();

        info!(
            digest = %digest,
            sender = %sender,
            sponsor = %self.signer_address,
            "Sui sponsored transaction executed successfully with dual signatures"
        );

        Ok(digest)
    }
}

impl NetworkProviderOps for SuiProvider {
    fn signer_address(&self) -> MixedAddress {
        MixedAddress::Sui(self.signer_address.to_string())
    }

    fn network(&self) -> Network {
        self.network
    }
}

impl FromEnvByNetworkBuild for SuiProvider {
    async fn from_env(network: Network) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        // Determine RPC URL based on network
        let rpc_env = match network {
            Network::Sui => ENV_RPC_SUI,
            Network::SuiTestnet => ENV_RPC_SUI_TESTNET,
            _ => {
                return Err(format!("Network {:?} is not a Sui network", network).into());
            }
        };

        let rpc_url = match std::env::var(rpc_env) {
            Ok(url) => url,
            Err(_) => {
                // Use public RPC endpoints as fallback
                match network {
                    Network::Sui => "https://fullnode.mainnet.sui.io:443".to_string(),
                    Network::SuiTestnet => "https://fullnode.testnet.sui.io:443".to_string(),
                    _ => unreachable!(),
                }
            }
        };

        // Determine private key based on network (mainnet vs testnet)
        let is_testnet = network.is_testnet();
        let private_key_env = if is_testnet {
            ENV_SUI_PRIVATE_KEY_TESTNET
        } else {
            ENV_SUI_PRIVATE_KEY_MAINNET
        };

        // Try network-specific key first, then fall back to generic key
        let private_key_str = match std::env::var(private_key_env)
            .or_else(|_| std::env::var(ENV_SUI_PRIVATE_KEY))
        {
            Ok(key) => key,
            Err(_) => {
                warn!(
                    network = %network,
                    "No Sui private key found for network, skipping provider initialization"
                );
                return Ok(None);
            }
        };

        // Parse the private key
        // Sui private keys can be in different formats:
        // - Base64 encoded raw key
        // - Bech32 encoded (suiprivkey1...)
        let keypair = parse_sui_private_key(&private_key_str)?;
        let signer_address = SuiAddress::from(&keypair.public());

        info!(
            network = %network,
            rpc_url = %rpc_url,
            signer = %signer_address,
            "Sui provider initialized"
        );

        Ok(Some(Self::new(network, rpc_url, signer_address, keypair)))
    }
}

/// Parse a Sui private key from various formats.
///
/// Supported formats:
/// - Bech32 encoded (suiprivkey1...)
/// - Base64 encoded with scheme prefix byte
fn parse_sui_private_key(key_str: &str) -> Result<SuiKeyPair, Box<dyn std::error::Error>> {
    let trimmed = key_str.trim();

    // Try Bech32 format first (suiprivkey1...)
    if trimmed.starts_with("suiprivkey") {
        return SuiKeyPair::decode(trimmed)
            .map_err(|e| format!("Failed to decode Sui bech32 private key: {}", e).into());
    }

    // Try base64 encoded format - SuiKeyPair::decode handles this too
    // The base64 format includes a scheme prefix byte
    SuiKeyPair::decode_base64(trimmed)
        .map_err(|e| format!("Failed to decode base64 Sui private key: {}", e).into())
}

impl Facilitator for SuiProvider {
    type Error = FacilitatorLocalError;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        let payload = self.extract_payload(request)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Full verification including signature and transaction structure
        let (_tx_data, _signature, payer_addr) = self.verify_transaction(payload, request).await?;

        // Check balance
        let required_amount: u64 = payload.amount.parse().unwrap_or(0);
        self.check_balance(&payer_addr, required_amount).await?;

        info!(
            network = %self.network,
            payer = %payer,
            "Sui payment verified successfully"
        );

        Ok(VerifyResponse::valid(payer))
    }

    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        let payload = self.extract_payload(request)?;
        let payer = MixedAddress::Sui(payload.from.clone());

        // Verify the transaction
        let (tx_data, signature, sender) = self.verify_transaction(payload, request).await?;

        // Check balance before settlement
        let required_amount: u64 = payload.amount.parse().unwrap_or(0);
        self.check_balance(&sender, required_amount).await?;

        // Submit the sponsored transaction
        match self.submit_sponsored_transaction(tx_data, signature, sender).await {
            Ok(digest) => {
                info!(
                    network = %self.network,
                    payer = %payer,
                    digest = %digest,
                    "Sui payment settled successfully"
                );

                Ok(SettleResponse {
                    success: true,
                    error_reason: None,
                    payer,
                    transaction: Some(crate::types::TransactionHash::Sui(digest)),
                    network: self.network,
                })
            }
            Err(e) => {
                error!(
                    network = %self.network,
                    payer = %payer,
                    error = %e,
                    "Sui settlement failed"
                );

                Ok(SettleResponse {
                    success: false,
                    error_reason: Some(crate::types::FacilitatorErrorReason::FreeForm(
                        format!("Settlement failed: {}", e),
                    )),
                    payer,
                    transaction: None,
                    network: self.network,
                })
            }
        }
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        // Return supported payment kinds for this Sui network
        let network_string = match self.network {
            Network::Sui => "sui",
            Network::SuiTestnet => "sui-testnet",
            _ => unreachable!(),
        };

        let kinds = vec![SupportedPaymentKind {
            x402_version: X402Version::V1,
            scheme: Scheme::Exact,
            network: network_string.to_string(),
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: Some(self.signer_address()),
                tokens: None, // TODO: Add supported tokens list
            }),
        }];

        Ok(SupportedPaymentKindsResponse { kinds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sui_address_parsing() {
        let valid_address = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let result = SuiProvider::parse_address(valid_address);
        assert!(result.is_ok());

        let invalid_address = "not-a-valid-address";
        let result = SuiProvider::parse_address(invalid_address);
        assert!(result.is_err());
    }

    #[test]
    fn test_usdc_coin_types() {
        assert!(USDC_COIN_TYPE_MAINNET.contains("usdc::USDC"));
        assert!(USDC_COIN_TYPE_TESTNET.contains("usdc::USDC"));
    }
}
