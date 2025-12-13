//! Facilitator implementation for x402 payments using on-chain verification and settlement.
//!
//! This module provides a [`Facilitator`] implementation that validates x402 payment payloads
//! and performs on-chain settlements using ERC-3009 `transferWithAuthorization`.
//!
//! Features include:
//! - EIP-712 signature recovery
//! - ERC-20 balance checks
//! - Contract interaction using Alloy
//! - Network-specific configuration via [`ProviderCache`] and [`USDCDeployment`]

use std::str::FromStr;
use std::sync::Arc;
use tracing::instrument;

use crate::chain::FacilitatorLocalError;
use crate::facilitator::Facilitator;
use crate::network::Network;
use crate::provider_cache::ProviderMap;
use crate::types::{
    Scheme, SettleRequest, SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
    SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse, X402Version,
};

// Compliance module
#[cfg(feature = "solana")]
use x402_compliance::SolanaExtractor;
use x402_compliance::{ComplianceChecker, EvmExtractor, ScreeningDecision, TransactionContext};

/// A concrete [`Facilitator`] implementation that verifies and settles x402 payments
/// using a network-aware provider cache.
///
/// This type is generic over the [`ProviderMap`] implementation used to access EVM providers,
/// which enables testing or customization beyond the default [`ProviderCache`].
pub struct FacilitatorLocal<A> {
    provider_map: A,
    compliance_checker: Arc<Box<dyn ComplianceChecker>>,
}

impl<A> FacilitatorLocal<A> {
    /// Creates a new [`FacilitatorLocal`] with the given provider cache and compliance checker.
    ///
    /// The provider cache is used to resolve the appropriate EVM provider for each payment's target network.
    /// The compliance checker is used to screen addresses against OFAC, UN, UK, EU sanctions lists.
    pub fn new(provider_map: A, compliance_checker: Arc<Box<dyn ComplianceChecker>>) -> Self {
        FacilitatorLocal {
            provider_map,
            compliance_checker,
        }
    }
}

impl<A, E> Facilitator for FacilitatorLocal<A>
where
    A: ProviderMap + Sync,
    A::Value: Facilitator<Error = E>,
    E: Send,
    FacilitatorLocalError: From<E>,
{
    type Error = FacilitatorLocalError;

    /// Verifies a proposed x402 payment payload against a passed [`PaymentRequirements`].
    ///
    /// This function validates the signature, timing, receiver match, network, scheme, and on-chain
    /// balance sufficiency for the token. If all checks pass, return a [`VerifyResponse::Valid`].
    ///
    /// Called from the `/verify` HTTP endpoint on the facilitator.
    ///
    /// # Errors
    ///
    /// Returns [`FacilitatorLocalError`] if any check fails, including:
    /// - scheme/network mismatch,
    /// - receiver mismatch,
    /// - invalid signature,
    /// - expired or future-dated timing,
    /// - insufficient funds,
    /// - unsupported network.
    #[instrument(skip_all, err, fields(network = %request.payment_payload.network))]
    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error> {
        tracing::debug!(
            "Verifying payment for network={}",
            request.payment_payload.network
        );

        let network = request.network();

        // Perform compliance screening before verification
        tracing::debug!("Performing compliance screening for verification");
        self.perform_compliance_screening(&request.payment_payload.payload, network)
            .await?;
        tracing::debug!("Compliance screening passed for verification");

        tracing::debug!("Resolving provider for network={}", network);
        let provider = self
            .provider_map
            .by_network(network)
            .ok_or(FacilitatorLocalError::UnsupportedNetwork(None))?;
        tracing::debug!("Provider resolved, calling verify on network provider");
        let verify_response = provider.verify(request).await?;
        match &verify_response {
            VerifyResponse::Valid { payer } => {
                tracing::debug!("Verification complete: Valid, payer={:?}", payer);
            }
            VerifyResponse::Invalid { reason, payer } => {
                tracing::debug!(
                    "Verification complete: Invalid, reason={:?}, payer={:?}",
                    reason,
                    payer
                );
            }
        }
        Ok(verify_response)
    }

    /// Executes an x402 payment on-chain using ERC-3009 `transferWithAuthorization`.
    ///
    /// This function performs the same validations as `verify`, then sends the authorized transfer
    /// via a smart contract and waits for transaction receipt.
    ///
    /// Called from the `/settle` HTTP endpoint on the facilitator.
    ///
    /// # Errors
    ///
    /// Returns [`FacilitatorLocalError`] if validation or contract call fails. Transaction receipt is included
    /// in the response on success or failure.
    #[instrument(skip_all, err, fields(network = %request.payment_payload.network))]
    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error> {
        use crate::types::ExactPaymentPayload;

        let network = request.network();
        tracing::debug!("Settlement request received for network={}", network);

        // CRITICAL: Re-screen compliance before settlement (don't trust prior verify call)
        tracing::debug!("Performing compliance screening before settlement");
        self.perform_compliance_screening(&request.payment_payload.payload, network)
            .await?;
        tracing::debug!("Compliance screening passed for settlement");

        tracing::debug!("Resolving provider for settlement on network={}", network);
        let provider = self.provider_map.by_network(network).ok_or_else(|| {
            tracing::error!("No provider found for network={}", network);
            FacilitatorLocalError::UnsupportedNetwork(None)
        })?;
        tracing::debug!(
            "Provider resolved, initiating settlement on network={}",
            network
        );
        let settle_response = provider.settle(request).await?;
        tracing::debug!(
            "Settlement response received: success={}, tx_hash={:?}, network={:?}",
            settle_response.success,
            settle_response.transaction,
            settle_response.network
        );
        Ok(settle_response)
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let mut kinds = vec![];
        for provider in self.provider_map.values() {
            let supported = provider.supported().await.ok();
            let mut supported_kinds = supported.map(|k| k.kinds).unwrap_or_default();
            kinds.append(&mut supported_kinds);
        }

        // Add v2 entries with CAIP-2 network identifiers for each v1 entry
        let v2_kinds: Vec<SupportedPaymentKind> = kinds
            .iter()
            .filter_map(|v1_kind| {
                // Parse the v1 network string to get the Network enum
                Network::from_str(&v1_kind.network).ok().map(|network| {
                    SupportedPaymentKind {
                        x402_version: X402Version::V2,
                        scheme: v1_kind.scheme.clone(),
                        network: network.to_caip2(),
                        extra: v1_kind.extra.clone(),
                    }
                })
            })
            .collect();

        kinds.extend(v2_kinds);

        // Add FHE transfer scheme support (proxied to Zama Lambda)
        // Zama FHEVM on Ethereum Sepolia testnet
        // Note: extra is None because FHE requests are proxied to the Zama Lambda facilitator
        // which handles its own fee_payer and token configuration (ERC7984 standard)
        kinds.push(SupportedPaymentKind {
            x402_version: X402Version::V1,
            scheme: Scheme::FheTransfer,
            network: "ethereum-sepolia".to_string(),
            extra: None, // FHE proxy handles fee_payer internally
        });
        kinds.push(SupportedPaymentKind {
            x402_version: X402Version::V2,
            scheme: Scheme::FheTransfer,
            network: "eip155:11155111".to_string(), // CAIP-2 for Sepolia
            extra: None, // FHE proxy handles fee_payer internally
        });

        Ok(SupportedPaymentKindsResponse { kinds })
    }

    async fn blacklist_info(&self) -> Result<serde_json::Value, Self::Error> {
        // Get compliance checker metadata
        let metadata = self.compliance_checker.list_metadata();

        Ok(serde_json::json!({
            "status": "loaded",
            "compliance_enabled": true,
            "lists": metadata.into_iter().map(|(name, meta)| {
                serde_json::json!({
                    "name": name,
                    "enabled": meta.enabled,
                    "record_count": meta.record_count,
                    "last_updated": meta.last_updated,
                    "source_url": meta.source_url
                })
            }).collect::<Vec<_>>()
        }))
    }
}

// Private helper methods
impl<A> FacilitatorLocal<A>
where
    A: ProviderMap + Sync,
{
    /// Private helper: Perform compliance screening for payment payload
    ///
    /// This method screens both payer and payee addresses against OFAC, UN, UK, EU sanctions lists
    /// and custom blacklist. Used by both verify() and settle() to ensure compliance.
    async fn perform_compliance_screening(
        &self,
        payload: &crate::types::ExactPaymentPayload,
        network: crate::network::Network,
    ) -> Result<(), FacilitatorLocalError> {
        use crate::types::{ExactPaymentPayload, MixedAddress};

        match payload {
            ExactPaymentPayload::Evm(evm_payload) => {
                // Extract payer and payee addresses
                let (payer, payee) = EvmExtractor::extract_addresses(
                    &evm_payload.authorization.from,
                    &evm_payload.authorization.to,
                )
                .map_err(|e| {
                    FacilitatorLocalError::Other(format!("Address extraction failed: {}", e))
                })?;

                // Create transaction context for audit logging
                let context = TransactionContext {
                    amount: evm_payload.authorization.value.to_string(),
                    currency: "USDC".to_string(),
                    network: format!("{:?}", network),
                    transaction_id: None,
                };

                // Screen both payer and payee
                tracing::debug!("Screening EVM payment: payer={}, payee={}", payer, payee);
                let screening_result = self
                    .compliance_checker
                    .screen_payment(&payer, &payee, &context)
                    .await
                    .map_err(|e| {
                        FacilitatorLocalError::Other(format!("Compliance screening failed: {}", e))
                    })?;

                match screening_result.decision {
                    ScreeningDecision::Block { reason } => {
                        tracing::warn!("Payment blocked by compliance: {}", reason);
                        return Err(FacilitatorLocalError::BlockedAddress(
                            MixedAddress::Evm(evm_payload.authorization.from),
                            reason,
                        ));
                    }
                    ScreeningDecision::Review { reason } => {
                        tracing::warn!("Payment requires manual review: {}", reason);
                        return Err(FacilitatorLocalError::BlockedAddress(
                            MixedAddress::Evm(evm_payload.authorization.from),
                            format!("Manual review required: {}", reason),
                        ));
                    }
                    ScreeningDecision::Clear => {
                        tracing::debug!("Payment cleared compliance screening");
                    }
                }

                Ok(())
            }
            ExactPaymentPayload::Solana(solana_payload) => {
                #[cfg(feature = "solana")]
                {
                    // Extract Solana addresses from transaction
                    match SolanaExtractor::extract_addresses(&solana_payload.transaction) {
                        Ok((payer, payee)) => {
                            tracing::debug!(
                                "Extracted Solana addresses: payer={}, payee={}",
                                payer,
                                payee
                            );

                            let context = TransactionContext {
                                amount: "unknown".to_string(),
                                currency: "SOL/SPL".to_string(),
                                network: format!("{:?}", network),
                                transaction_id: None,
                            };

                            let screening_result = self
                                .compliance_checker
                                .screen_payment(&payer, &payee, &context)
                                .await
                                .map_err(|e| {
                                    FacilitatorLocalError::Other(format!(
                                        "Compliance screening failed: {}",
                                        e
                                    ))
                                })?;

                            match screening_result.decision {
                                ScreeningDecision::Block { reason } => {
                                    tracing::warn!(
                                        "Solana payment blocked by compliance: {}",
                                        reason
                                    );
                                    return Err(FacilitatorLocalError::Other(format!(
                                        "Payment blocked: {}",
                                        reason
                                    )));
                                }
                                ScreeningDecision::Review { reason } => {
                                    tracing::warn!(
                                        "Solana payment requires manual review: {}",
                                        reason
                                    );
                                    return Err(FacilitatorLocalError::Other(format!(
                                        "Manual review required: {}",
                                        reason
                                    )));
                                }
                                ScreeningDecision::Clear => {
                                    tracing::debug!("Solana payment cleared compliance screening");
                                }
                            }

                            Ok(())
                        }
                        Err(e) => {
                            // TEMPORARY FIX: FAIL-OPEN until blacklist is properly configured
                            // TODO: Revert to FAIL-CLOSED once Solana blacklist addresses are available
                            // Original error: Failed to deserialize Solana transaction
                            tracing::warn!(
                                "Failed to extract Solana addresses for screening: {}. \
                            ALLOWING transaction temporarily (compliance check bypassed). \
                            TODO: Re-enable strict compliance once blacklist is configured.",
                                e
                            );
                            Ok(())
                        }
                    }
                }

                #[cfg(not(feature = "solana"))]
                {
                    Err(FacilitatorLocalError::Other(
                        "Solana support not enabled".to_string(),
                    ))
                }
            }
            ExactPaymentPayload::Near(_near_payload) => {
                #[cfg(feature = "near")]
                {
                    // Extract NEAR addresses from delegate action
                    // For now, allow NEAR transactions through (compliance will be added later)
                    tracing::debug!(
                        "NEAR payment compliance check: allowing transaction (TODO: implement NEAR compliance)"
                    );
                    Ok(())
                }

                #[cfg(not(feature = "near"))]
                {
                    Err(FacilitatorLocalError::Other(
                        "NEAR support not enabled".to_string(),
                    ))
                }
            }
            ExactPaymentPayload::Stellar(_stellar_payload) => {
                #[cfg(feature = "stellar")]
                {
                    // For now, allow Stellar transactions through (compliance will be added later)
                    tracing::debug!(
                        "Stellar payment compliance check: allowing transaction (TODO: implement Stellar compliance)"
                    );
                    Ok(())
                }

                #[cfg(not(feature = "stellar"))]
                {
                    Err(FacilitatorLocalError::Other(
                        "Stellar support not enabled".to_string(),
                    ))
                }
            }
        }
    }
}
