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

use crate::blocklist::SharedBlacklist;
use crate::chain::FacilitatorLocalError;
use crate::facilitator::Facilitator;
use crate::provider_cache::ProviderMap;
use crate::types::{
    SettleRequest, SettleResponse, SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse,
};

/// A concrete [`Facilitator`] implementation that verifies and settles x402 payments
/// using a network-aware provider cache.
///
/// This type is generic over the [`ProviderMap`] implementation used to access EVM providers,
/// which enables testing or customization beyond the default [`ProviderCache`].
pub struct FacilitatorLocal<A> {
    provider_map: A,
    blacklist: SharedBlacklist,
}

impl<A> FacilitatorLocal<A> {
    /// Creates a new [`FacilitatorLocal`] with the given provider cache and blacklist.
    ///
    /// The provider cache is used to resolve the appropriate EVM provider for each payment's target network.
    /// The blacklist is used to block addresses from using the facilitator.
    pub fn new(provider_map: A, blacklist: SharedBlacklist) -> Self {
        FacilitatorLocal { provider_map, blacklist }
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

        // Check blacklist before processing
        match &request.payment_payload.payload {
            ExactPaymentPayload::Evm(evm_payload) => {
                let from_address = format!("{:?}", evm_payload.authorization.from);
                tracing::debug!("Checking blacklist for EVM address={}", from_address);
                if let Some(reason) = self.blacklist.is_evm_blocked(&from_address) {
                    tracing::warn!("Blocked EVM address={}, reason={}", from_address, reason);
                    return Err(FacilitatorLocalError::BlockedAddress(
                        MixedAddress::Evm(evm_payload.authorization.from),
                        reason,
                    ));
                }
            }
            ExactPaymentPayload::Solana(_solana_payload) => {
                // For Solana, we would need to parse the transaction to extract the signer
                // This is more complex and may require decoding the base64 transaction
                // For now, we'll skip Solana blacklist checking in verify()
                // TODO: Implement Solana address extraction and blacklist check
                tracing::debug!("Skipping blacklist check for Solana (not implemented)");
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
