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
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    EscrowSupportedInfo, EvmAddress, Scheme, SettleRequest, SettleResponse, SupportedPaymentKind,
    SupportedPaymentKindExtra, SupportedPaymentKindsResponse, VerifyRequest, VerifyResponse,
    X402Version,
};

// Compliance module
#[cfg(feature = "solana")]
use x402_compliance::SolanaExtractor;
use x402_compliance::{ComplianceChecker, EvmExtractor, ScreeningDecision, TransactionContext};

/// The same payment kind, named the other way round, or `None` when the chain
/// resolves to neither form.
///
/// The x402 version tracks the naming form, exactly as it always has for
/// `exact`: a v1 chain name goes out as x402 v1, a CAIP-2 id as x402 v2.
fn network_form_counterpart(kind: &SupportedPaymentKind) -> Option<SupportedPaymentKind> {
    let (x402_version, network) = match Network::from_str(&kind.network) {
        Ok(network) => (X402Version::V2, network.to_caip2()),
        Err(_) => (
            X402Version::V1,
            Network::from_caip2(&kind.network)?.to_string(),
        ),
    };
    Some(SupportedPaymentKind {
        x402_version,
        scheme: kind.scheme,
        network,
        extra: kind.extra.clone(),
    })
}

/// Everything a client can tell apart about one advertised kind.
///
/// `extra` is part of the identity on purpose: the `escrow` scheme publishes
/// one entry per deployed PaymentOperator on the same network, and those
/// entries differ in nothing else. Keying without it would collapse them.
fn kind_identity(kind: &SupportedPaymentKind) -> String {
    format!(
        "{}|{}|{}|{}",
        kind.x402_version.as_u8(),
        kind.scheme,
        kind.network,
        serde_json::to_string(&kind.extra).unwrap_or_default(),
    )
}

/// Advertise every kind under BOTH ways of naming its chain.
///
/// `/supported` publishes each chain twice: by its v1 name (`base`) and by its
/// CAIP-2 id (`eip155:8453`). Until 2026-09-03 only `exact` got both, because
/// the mirroring ran as its own pass BEFORE `escrow`, `commerce` and `upto`
/// were pushed. Measured against production on 2026-09-02: `exact` 38 and 38,
/// and zero v1 entries across the other three schemes. A client that
/// discovers schemes by reading the v1 entries -- which is every integration
/// written before CAIP-2 existed -- concluded this facilitator has no escrow
/// at all. The advertisement was not incomplete, it was wrong.
///
/// So the mirroring is ONE pass over the finished list and every scheme goes
/// through it by construction. A scheme added later is advertised under both
/// forms without its author having to know that both forms exist.
///
/// A kind whose network resolves to neither form is left exactly as it is:
/// inventing an identifier would be worse than publishing only one.
fn advertise_under_both_network_forms(
    mut kinds: Vec<SupportedPaymentKind>,
) -> Vec<SupportedPaymentKind> {
    let mut seen: std::collections::HashSet<String> = kinds.iter().map(kind_identity).collect();

    let mirrored: Vec<SupportedPaymentKind> = kinds
        .iter()
        .filter_map(network_form_counterpart)
        .filter(|counterpart| seen.insert(kind_identity(counterpart)))
        .collect();

    kinds.extend(mirrored);
    kinds
}

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

    /// Returns a reference to the underlying provider map.
    ///
    /// This is used by the escrow module to access network-specific providers
    /// for x402r escrow settlement.
    pub fn provider_map(&self) -> &A {
        &self.provider_map
    }
}

// Implement HasProviderMap to allow escrow module to access providers
impl<A: ProviderMap> HasProviderMap for FacilitatorLocal<A> {
    type Map = A;

    fn provider_map(&self) -> &Self::Map {
        &self.provider_map
    }
}

// Delegate ProviderMap so the WS-E attestation task can look up providers
// straight from the shared `Arc<FacilitatorLocal<_>>` axum state.
impl<A: ProviderMap> ProviderMap for FacilitatorLocal<A> {
    type Value = A::Value;

    fn by_network<N: std::borrow::Borrow<crate::network::Network>>(
        &self,
        network: N,
    ) -> Option<&Self::Value> {
        self.provider_map.by_network(network)
    }

    fn values(&self) -> impl Iterator<Item = &Self::Value> + Send {
        self.provider_map.values()
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

        // NOTE: the pass that names every chain BOTH ways runs once, at the
        // very END of this function. It used to run here, which is precisely
        // why only `exact` ever got both forms -- everything pushed below was
        // pushed after the mirror had already been taken. Push one entry per
        // scheme here and let `advertise_under_both_network_forms` finish it.

        // Add FHE transfer scheme support (proxied to Zama Lambda)
        // Zama FHEVM on Ethereum Sepolia testnet
        // Note: extra is None because FHE requests are proxied to the Zama Lambda facilitator
        // which handles its own fee_payer and token configuration (ERC7984 standard)
        // Only the v1 form is pushed: the mirroring pass derives
        // `eip155:11155111` from the Network enum, so the CAIP-2 id is never
        // typed out here to drift away from `Network::to_caip2`.
        kinds.push(SupportedPaymentKind {
            x402_version: X402Version::V1,
            scheme: Scheme::FheTransfer,
            network: "ethereum-sepolia".to_string(),
            extra: None, // FHE proxy handles fee_payer internally
        });

        // Add x402r escrow/commerce scheme support (PaymentOperator-based escrow)
        // Dynamically advertise all networks with deployed PaymentOperator contracts
        // Only if ENABLE_PAYMENT_OPERATOR=true
        //
        // "escrow" scheme: uses per-chain legacy addresses (backward compat with existing operators)
        // "commerce" scheme: uses x402r CREATE3 canonical addresses (interop with @x402r/helpers)
        if crate::payment_operator::is_enabled() {
            use crate::payment_operator::addresses::{create3, OperatorAddresses, ESCROW_NETWORKS};

            for &network in ESCROW_NETWORKS {
                if let Some(addrs) = OperatorAddresses::for_network(network) {
                    // Escrow scheme: per-chain legacy addresses, one entry per deployed operator
                    for &operator in &addrs.payment_operators {
                        let escrow_extra = SupportedPaymentKindExtra {
                            fee_payer: None,
                            tokens: None,
                            escrow: Some(EscrowSupportedInfo {
                                escrow_address: EvmAddress(addrs.escrow),
                                operator_address: EvmAddress(operator),
                                token_collector: EvmAddress(addrs.token_collector),
                            }),
                        };

                        kinds.push(SupportedPaymentKind {
                            x402_version: X402Version::V2,
                            scheme: Scheme::Escrow,
                            network: network.to_caip2(),
                            extra: Some(escrow_extra),
                        });
                    }

                    // Commerce scheme: CREATE3 canonical addresses (no operator — merchant-specific)
                    let commerce_extra = SupportedPaymentKindExtra {
                        fee_payer: None,
                        tokens: None,
                        escrow: Some(EscrowSupportedInfo {
                            escrow_address: EvmAddress(create3::ESCROW),
                            operator_address: EvmAddress(create3::FACTORY_PAYMENT_OPERATOR),
                            token_collector: EvmAddress(create3::TOKEN_COLLECTOR),
                        }),
                    };

                    kinds.push(SupportedPaymentKind {
                        x402_version: X402Version::V2,
                        scheme: Scheme::Commerce,
                        network: network.to_caip2(),
                        extra: Some(commerce_extra),
                    });
                }
            }
        }

        // Add upto scheme support (Permit2-based variable amount settlement)
        // Only if ENABLE_UPTO=true
        //
        // ONLY on chains where the proxy actually has code. The address is the
        // same everywhere (deterministic CREATE2), and this used to be read as
        // "therefore it works everywhere" — so `upto` was advertised on every
        // EVM network carrying `exact`, including five where the deployment was
        // never replayed. Advertising an unsettleable scheme is worse than not
        // offering it: the client only finds out after signing a Permit2
        // authorization, and a signed authorization does not un-sign itself.
        if crate::upto::is_enabled() {
            // Collect the EVM networks carrying `exact`, resolved through the
            // Network enum rather than by matching an `eip155:` prefix. The
            // prefix test only ever saw the mirrored CAIP-2 entries, so this
            // list was silently coupled to the mirror running FIRST; now that
            // it runs last, reading the v1 entries is the only thing that
            // works -- and it is the thing that was always meant.
            let mut evm_networks: Vec<String> = kinds
                .iter()
                .filter(|k| k.scheme == Scheme::Exact)
                .filter_map(|k| {
                    Network::from_str(&k.network)
                        .ok()
                        .or_else(|| Network::from_caip2(&k.network))
                })
                .filter(|network| crate::upto::types::is_proxy_deployed_on(*network))
                .map(|network| network.to_caip2())
                .collect();
            evm_networks.sort();
            evm_networks.dedup();

            for network_caip2 in evm_networks {
                kinds.push(SupportedPaymentKind {
                    x402_version: X402Version::V2,
                    scheme: Scheme::Upto,
                    network: network_caip2,
                    extra: None, // Upto doesn't need extra (Permit2 domain is always "Permit2")
                });
            }
        }

        // Every scheme, under both ways of naming its chain. Once, here, over
        // the finished list -- see the function's own doc comment for what
        // made that placement load-bearing.
        let kinds = advertise_under_both_network_forms(kinds);

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
            #[cfg(feature = "algorand")]
            ExactPaymentPayload::Algorand(_algorand_payload) => {
                // For now, allow Algorand transactions through (compliance will be added later)
                tracing::debug!(
                    "Algorand payment compliance check: allowing transaction (TODO: implement Algorand compliance)"
                );
                Ok(())
            }
            #[cfg(feature = "sui")]
            ExactPaymentPayload::Sui(_sui_payload) => {
                // For now, allow Sui transactions through (compliance will be added later)
                tracing::debug!(
                    "Sui payment compliance check: allowing transaction (TODO: implement Sui compliance)"
                );
                Ok(())
            }
            ExactPaymentPayload::SolanaSettlementAccount(_sa_payload) => {
                // Settlement account payloads contain an already-submitted transaction signature.
                // Compliance screening will be done when verifying the on-chain transaction.
                tracing::debug!(
                    "Settlement account compliance check: will verify on-chain transaction"
                );
                Ok(())
            }
            #[cfg(feature = "xrpl")]
            ExactPaymentPayload::Xrpl(_xrpl_payload) => {
                // XRPL pre-signed transactions: no compliance screening at this layer.
                tracing::debug!("XRPL payment compliance check: allowing transaction");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod supported_advertisement_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn kind(x402_version: X402Version, scheme: Scheme, network: &str) -> SupportedPaymentKind {
        SupportedPaymentKind {
            x402_version,
            scheme,
            network: network.to_string(),
            extra: None,
        }
    }

    fn escrow_kind(network: &str, operator_byte: u8) -> SupportedPaymentKind {
        SupportedPaymentKind {
            x402_version: X402Version::V2,
            scheme: Scheme::Escrow,
            network: network.to_string(),
            extra: Some(SupportedPaymentKindExtra {
                fee_payer: None,
                tokens: None,
                escrow: Some(EscrowSupportedInfo {
                    escrow_address: EvmAddress(alloy::primitives::Address::repeat_byte(1)),
                    operator_address: EvmAddress(alloy::primitives::Address::repeat_byte(
                        operator_byte,
                    )),
                    token_collector: EvmAddress(alloy::primitives::Address::repeat_byte(3)),
                }),
            }),
        }
    }

    /// The shape `/supported` actually published on 2026-09-02: `exact` under
    /// both ways of naming the chain, `escrow` / `commerce` / `upto` under the
    /// CAIP-2 form alone.
    fn shape_measured_before_the_fix() -> Vec<SupportedPaymentKind> {
        vec![
            kind(X402Version::V1, Scheme::Exact, "base"),
            kind(X402Version::V2, Scheme::Exact, "eip155:8453"),
            kind(X402Version::V1, Scheme::Exact, "polygon"),
            kind(X402Version::V2, Scheme::Exact, "eip155:137"),
            escrow_kind("eip155:8453", 2),
            kind(X402Version::V2, Scheme::Commerce, "eip155:8453"),
            kind(X402Version::V2, Scheme::Upto, "eip155:8453"),
        ]
    }

    /// Which ways of naming a chain each (scheme, chain) pair is published
    /// under: `(v1 name seen, CAIP-2 id seen)`.
    fn forms_seen(kinds: &[SupportedPaymentKind]) -> HashMap<(String, Network), (bool, bool)> {
        let mut seen: HashMap<(String, Network), (bool, bool)> = HashMap::new();
        for k in kinds {
            let (network, is_caip2) = match Network::from_str(&k.network) {
                Ok(network) => (network, false),
                Err(_) => match Network::from_caip2(&k.network) {
                    Some(network) => (network, true),
                    None => continue,
                },
            };
            let entry = seen.entry((k.scheme.to_string(), network)).or_default();
            if is_caip2 {
                entry.1 = true;
            } else {
                entry.0 = true;
            }
        }
        seen
    }

    /// The defect, stated as an invariant: a scheme published under only one
    /// of the two ways of naming a chain, while another scheme on that same
    /// chain gets both, is an advertisement that lies by omission. Measured
    /// before the fix: `exact` 38 v1 entries, `escrow` / `commerce` / `upto`
    /// zero between them, so a v1 client read "this facilitator has no
    /// escrow".
    #[test]
    fn no_scheme_is_advertised_under_fewer_forms_than_another() {
        let published = advertise_under_both_network_forms(shape_measured_before_the_fix());
        let seen = forms_seen(&published);

        assert!(!seen.is_empty(), "the fixture published nothing");
        for ((scheme, network), (v1_name, caip2)) in &seen {
            assert!(
                *v1_name && *caip2,
                "{scheme} on {network} is advertised under one form only \
                 (v1 name: {v1_name}, CAIP-2: {caip2})"
            );
        }
    }

    /// The v1 half is the half that was missing, so name it outright rather
    /// than leaving it implied by the invariant above.
    #[test]
    fn escrow_commerce_and_upto_gain_their_v1_names() {
        let published = advertise_under_both_network_forms(shape_measured_before_the_fix());

        for scheme in [Scheme::Escrow, Scheme::Commerce, Scheme::Upto] {
            let v1_entries: Vec<&SupportedPaymentKind> = published
                .iter()
                .filter(|k| k.scheme == scheme && k.network == "base")
                .collect();
            assert_eq!(
                v1_entries.len(),
                1,
                "{scheme} must be discoverable as `base`, not only as `eip155:8453`"
            );
            assert_eq!(v1_entries[0].x402_version, X402Version::V1);
        }
    }

    /// `escrow` publishes one entry per deployed PaymentOperator on the same
    /// chain, differing in nothing but `extra`. A dedup key that ignored
    /// `extra` would silently drop every operator but the first.
    #[test]
    fn operators_that_differ_only_in_extra_all_survive() {
        let published = advertise_under_both_network_forms(vec![
            escrow_kind("eip155:8453", 2),
            escrow_kind("eip155:8453", 9),
        ]);

        let v1_escrows: Vec<&SupportedPaymentKind> = published
            .iter()
            .filter(|k| k.scheme == Scheme::Escrow && k.network == "base")
            .collect();
        assert_eq!(v1_escrows.len(), 2, "one v1 entry per deployed operator");

        let operators: HashSet<String> = v1_escrows
            .iter()
            .map(|k| {
                k.extra
                    .as_ref()
                    .and_then(|e| e.escrow.as_ref())
                    .map(|e| e.operator_address.0.to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(operators.len(), 2, "the two operators must stay distinct");
    }

    /// A scheme that already pushed both forms by hand must not gain a third
    /// copy. This is what lets the pass be applied to the whole list without
    /// auditing who pushed what.
    #[test]
    fn a_kind_already_published_under_both_forms_is_not_duplicated() {
        let published = advertise_under_both_network_forms(vec![
            kind(X402Version::V1, Scheme::FheTransfer, "ethereum-sepolia"),
            kind(X402Version::V2, Scheme::FheTransfer, "eip155:11155111"),
        ]);
        assert_eq!(published.len(), 2);
    }

    /// A network the enum does not know is left exactly as it was. Inventing
    /// an identifier for it would be worse than publishing only one form.
    #[test]
    fn an_unresolvable_network_is_left_alone() {
        let published = advertise_under_both_network_forms(vec![kind(
            X402Version::V1,
            Scheme::Exact,
            "not-a-network",
        )]);
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].network, "not-a-network");
    }

    /// The mirror derives the CAIP-2 id from the enum, never from a string
    /// typed at the push site, so the two can never drift apart.
    #[test]
    fn the_caip2_twin_comes_from_the_network_enum() {
        let published = advertise_under_both_network_forms(vec![kind(
            X402Version::V1,
            Scheme::FheTransfer,
            "ethereum-sepolia",
        )]);
        let twin = published
            .iter()
            .find(|k| k.x402_version == X402Version::V2)
            .expect("the CAIP-2 twin must be generated");
        assert_eq!(twin.network, Network::EthereumSepolia.to_caip2());
    }
}
