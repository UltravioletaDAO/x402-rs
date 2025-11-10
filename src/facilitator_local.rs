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

use tracing::instrument;
use std::sync::Arc;

use crate::chain::FacilitatorLocalError;
use crate::facilitator::Facilitator;
use crate::provider_cache::ProviderMap;
use crate::types::{
    SettleRequest, SettleResponse, SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse,
};

// Compliance module
use x402_compliance::{ComplianceChecker, TransactionContext, ScreeningDecision};

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
        FacilitatorLocal { provider_map, compliance_checker }
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
        use crate::types::{ExactPaymentPayload, MixedAddress};

        tracing::debug!("Verifying payment for network={}", request.payment_payload.network);

        // Perform compliance screening (OFAC + blacklist) before processing
        match &request.payment_payload.payload {
            ExactPaymentPayload::Evm(evm_payload) => {
                use x402_compliance::extractors::EvmExtractor;

                // Extract payer and payee addresses
                let (payer, payee) = EvmExtractor::extract_addresses(
                    &evm_payload.authorization.from,
                    &evm_payload.authorization.to
                ).map_err(|e| FacilitatorLocalError::Other(format!("Address extraction failed: {}", e)))?;

                // Create transaction context for audit logging
                let context = TransactionContext {
                    amount: evm_payload.authorization.value.to_string(),
                    currency: "USDC".to_string(), // TODO: Extract from network config
                    network: format!("{:?}", request.payment_payload.network),
                    transaction_id: None,
                };

                // Screen both payer and payee
                tracing::debug!("Screening EVM payment: payer={}, payee={}", payer, payee);
                let screening_result = self.compliance_checker
                    .screen_payment(&payer, &payee, &context)
                    .await
                    .map_err(|e| FacilitatorLocalError::Other(format!("Compliance screening failed: {}", e)))?;

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
                        // For now, treat Review as Block (can be configured differently later)
                        return Err(FacilitatorLocalError::BlockedAddress(
                            MixedAddress::Evm(evm_payload.authorization.from),
                            format!("Manual review required: {}", reason),
                        ));
                    }
                    ScreeningDecision::Clear => {
                        tracing::debug!("Payment cleared compliance screening");
                    }
                }
            }
            ExactPaymentPayload::Solana(solana_payload) => {
                use x402_compliance::extractors::SolanaExtractor;

                // Extract Solana addresses from transaction
                match SolanaExtractor::extract_addresses(&solana_payload.transaction) {
                    Ok((payer, payee)) => {
                        tracing::debug!("Extracted Solana addresses: payer={}, payee={}", payer, payee);

                        let context = TransactionContext {
                            amount: "unknown".to_string(), // Solana amount requires parsing
                            currency: "SOL/SPL".to_string(),
                            network: format!("{:?}", request.payment_payload.network),
                            transaction_id: None,
                        };

                        let screening_result = self.compliance_checker
                            .screen_payment(&payer, &payee, &context)
                            .await
                            .map_err(|e| FacilitatorLocalError::Other(format!("Compliance screening failed: {}", e)))?;

                        match screening_result.decision {
                            ScreeningDecision::Block { reason } => {
                                tracing::warn!("Solana payment blocked by compliance: {}", reason);
                                return Err(FacilitatorLocalError::Other(format!(
                                    "Payment blocked: {}",
                                    reason
                                )));
                            }
                            ScreeningDecision::Review { reason } => {
                                tracing::warn!("Solana payment requires manual review: {}", reason);
                                return Err(FacilitatorLocalError::Other(format!(
                                    "Manual review required: {}",
                                    reason
                                )));
                            }
                            ScreeningDecision::Clear => {
                                tracing::debug!("Solana payment cleared compliance screening");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to extract Solana addresses for screening: {}", e);
                        // Fail-open: continue without screening for Solana
                        // TODO: Make this configurable (fail-open vs fail-closed)
                    }
                }
            }
        }

        let network = request.network();
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
                tracing::debug!("Verification complete: Invalid, reason={:?}, payer={:?}", reason, payer);
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
        let network = request.network();
        tracing::debug!("Settlement request received for network={}", network);
        tracing::debug!("Resolving provider for settlement on network={}", network);
        let provider = self
            .provider_map
            .by_network(network)
            .ok_or_else(|| {
                tracing::error!("No provider found for network={}", network);
                FacilitatorLocalError::UnsupportedNetwork(None)
            })?;
        tracing::debug!("Provider resolved, initiating settlement on network={}", network);
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
        Ok(SupportedPaymentKindsResponse { kinds })
    }

    async fn blacklist_info(&self) -> Result<serde_json::Value, Self::Error> {
        Ok(serde_json::json!({
            "total_blocked": self.blacklist.total_blocked(),
            "evm_count": self.blacklist.evm_count(),
            "solana_count": self.blacklist.solana_count(),
            "entries": self.blacklist.entries(),
            "source": "config/blacklist.json",
            "loaded_at_startup": true
        }))
    }
}
