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

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::instrument;

use crate::blocklist::SharedBlacklist;
use crate::chain::{FacilitatorLocalError, NetworkProvider, NetworkProviderOps};
use crate::facilitator::Facilitator;
use crate::network::Network;
use crate::provider_cache::ProviderCache;
use crate::provider_cache::ProviderMap;
use crate::types::{
    MixedAddress, Scheme, SettleRequest, SettleResponse, SupportedPaymentKind,
    SupportedPaymentKindExtra, SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse,
    X402Version,
};

/// A concrete [`Facilitator`] implementation that verifies and settles x402 payments
/// using a network-aware provider cache.
///
/// This type is generic over the [`ProviderMap`] implementation used to access EVM providers,
/// which enables testing or customization beyond the default [`ProviderCache`].
#[derive(Clone)]
pub struct FacilitatorLocal {
    pub provider_cache: Arc<ProviderCache>,
    pub blacklist: SharedBlacklist,
}

impl FacilitatorLocal {
    /// Creates a new [`FacilitatorLocal`] with the given provider cache and blacklist.
    ///
    /// The provider cache is used to resolve the appropriate EVM provider for each payment's target network.
    /// The blacklist is checked to prevent processing payments from blocked addresses.
    pub fn new(provider_cache: ProviderCache, blacklist: SharedBlacklist) -> Self {
        FacilitatorLocal {
            provider_cache: Arc::new(provider_cache),
            blacklist,
        }
    }

    /// Helper method to check if an address is blacklisted.
    /// Returns an error with descriptive message if the address is blocked.
    fn check_address(&self, addr: &MixedAddress, role: &str) -> Result<(), FacilitatorLocalError> {
        match addr {
            MixedAddress::Evm(evm_addr) => {
                if let Some(reason) = self.blacklist.is_evm_blocked(&format!("{}", evm_addr)) {
                    tracing::warn!("Blocked EVM address ({}) attempted payment: {} - Reason: {}", role, evm_addr, reason);
                    return Err(FacilitatorLocalError::BlockedAddress(addr.clone(), format!("{}: {}", role, reason)));
                }
            }
            MixedAddress::Solana(pubkey) => {
                if let Some(reason) = self.blacklist.is_solana_blocked(&pubkey.to_string()) {
                    tracing::warn!("Blocked Solana address ({}) attempted payment: {} - Reason: {}", role, pubkey, reason);
                    return Err(FacilitatorLocalError::BlockedAddress(addr.clone(), format!("{}: {}", role, reason)));
                }
            }
            MixedAddress::Offchain(_) => {
                // Offchain addresses are not checked against blacklist
            }
        }
        Ok(())
    }

    pub fn kinds(&self) -> Vec<SupportedPaymentKind> {
        self.provider_cache
            .into_iter()
            .map(|(network, provider)| match provider {
                NetworkProvider::Evm(_) => SupportedPaymentKind {
                    x402_version: X402Version::V1,
                    scheme: Scheme::Exact,
                    network: network.to_string(),
                    extra: None,
                },
                NetworkProvider::Solana(provider) => SupportedPaymentKind {
                    x402_version: X402Version::V1,
                    scheme: Scheme::Exact,
                    network: network.to_string(),
                    extra: Some(SupportedPaymentKindExtra {
                        fee_payer: provider.signer_address(),
                    }),
                },
            })
            .collect()
    }

    pub fn health(&self) -> Vec<HealthStatus> {
        self.provider_cache
            .into_iter()
            .map(|(network, provider)| match provider {
                NetworkProvider::Evm(_) => HealthStatus {
                    network: *network,
                    address: provider.signer_address(),
                },
                NetworkProvider::Solana(provider) => HealthStatus {
                    network: *network,
                    address: provider.signer_address(),
                },
            })
            .collect()
    }
}

impl Facilitator for FacilitatorLocal {
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
        let network = request.network();
        let provider = self
            .provider_cache
            .by_network(network)
            .ok_or(FacilitatorLocalError::UnsupportedNetwork(None))?;

        // Verify the payment through the provider
        let response = provider.verify(request).await?;

        // Check if payer (sender) is blacklisted
        let payer = match &response {
            VerifyResponse::Valid { payer } => payer,
            VerifyResponse::Invalid { payer: Some(payer), .. } => payer,
            VerifyResponse::Invalid { payer: None, .. } => return Ok(response), // No payer to check
        };

        // Check sender address against blacklist
        self.check_address(payer, "Blocked sender")?;

        // Check receiver address against blacklist
        let receiver = &request.payment_requirements.pay_to;
        self.check_address(receiver, "Blocked recipient")?;

        Ok(response)
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
        let provider = self
            .provider_cache
            .by_network(network)
            .ok_or(FacilitatorLocalError::UnsupportedNetwork(None))?;

        // Settle the payment through the provider
        let response = provider.settle(request).await?;

        // Check if sender or receiver is blacklisted (verify after settlement for logging/audit purposes)
        // Note: The blacklist check should ideally happen during verification phase,
        // but we check here too as a safety measure in case settle is called directly

        // Check sender (payer)
        if let Err(e) = self.check_address(&response.payer, "Blocked sender") {
            tracing::error!("Blacklisted sender completed settlement: {} - Error: {}", response.payer, e);
            // We don't return error here since transaction is already on-chain,
            // but we log it for audit purposes
        }

        // Check receiver
        let receiver = &request.payment_requirements.pay_to;
        if let Err(e) = self.check_address(receiver, "Blocked recipient") {
            tracing::error!("Blacklisted recipient completed settlement: {} - Error: {}", receiver, e);
            // We don't return error here since transaction is already on-chain,
            // but we log it for audit purposes
        }

        Ok(response)
    }

    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error> {
        let kinds = self.kinds();
        Ok(SupportedPaymentKindsResponse { kinds })
    }

    async fn blacklist_info(&self) -> Result<crate::types::BlacklistInfoResponse, Self::Error> {
        // Convert internal BlacklistEntry to API BlacklistEntry
        let entries = self.blacklist.entries()
            .iter()
            .map(|e| crate::types::BlacklistEntry {
                account_type: e.account_type.clone(),
                wallet: e.wallet.clone(),
                reason: e.reason.clone(),
            })
            .collect();

        Ok(crate::types::BlacklistInfoResponse {
            total_blocked: self.blacklist.total_blocked(),
            evm_count: self.blacklist.evm_count(),
            solana_count: self.blacklist.solana_count(),
            entries,
            source: "config/blacklist.json".to_string(),
            loaded_at_startup: self.blacklist.total_blocked() > 0,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    pub network: Network,
    pub address: MixedAddress,
}
