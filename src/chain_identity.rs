//! Does each EVM RPC answer for the chain we sign for?
//!
//! # Why this exists
//!
//! The chain id lives in two places: the one `EvmChain` declares, which the
//! EIP-712 domain commits to, and the one the RPC answers, which the
//! transaction filler signs with. Through 2.39.0 two testnets were served with
//! the declared id wrong -- `celo-sepolia` said 44787 (Alfajores) while its RPC
//! answered 11142220, `hyperevm-testnet` said 333 while its RPC answered 998 --
//! and nothing compared the two. Only Arc did, in `EvmProvider::try_new`.
//!
//! # An alert, never a refusal -- except Arc's mismatch
//!
//! For every EVM network but Arc a mismatch is logged and counted, and the
//! network stays in `/supported`. Had this check refused, 2.39.0 would have
//! switched both testnets off on deploy, over a number in our own table. A
//! wrong RPC does not settle a payment on the wrong chain either: the token's
//! own domain is checked on-chain, so the payment fails and the alert says why.
//! That check runs once per task start, in the background ([`spawn`]), so a
//! slow RPC delays nothing.
//!
//! Arc is admitted by [`admit`] before it is served: its local signature
//! recovery commits to the configured chain id while the transaction filler
//! asks the RPC, so an Arc RPC that answers for another chain leaves Arc out of
//! `/supported`. An Arc RPC that does not answer in time is served and alerted.
//! Neither outcome is an error: until 2.39.2 both were, and an error there
//! stopped the whole process, for every network, over one network's probe.
//!
//! Alert tokens, counted by the CloudWatch metric filter in
//! `alerts-network-startup.tf`: `evm_rpc_chain_id_mismatch` (any network) and
//! `arc_rpc_chain_id_unverified` (Arc served without an answer). Native
//! Hedera's startup health check follows the same rule and logs
//! `hedera_health_failed_at_startup` (`src/chain/hedera/mod.rs`).

use std::sync::Arc;
use std::time::Duration;

use alloy::providers::Provider;

use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::chain::NetworkProvider;
use crate::provider_cache::{ProviderCache, ProviderMap};

/// What one RPC said about its chain id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Matches,
    Mismatch {
        expected: u64,
        actual: u64,
    },
    /// No answer within the timeout, or an error. Not a mismatch.
    Unverified,
}

/// Ask `evm`'s RPC for its chain id and compare it with the declared one.
pub async fn check(evm: &EvmProvider, timeout: Duration) -> Verdict {
    let expected = evm.chain().chain_id;
    match tokio::time::timeout(timeout, evm.inner().get_chain_id()).await {
        Ok(Ok(actual)) if actual == expected => Verdict::Matches,
        Ok(Ok(actual)) => Verdict::Mismatch { expected, actual },
        _ => Verdict::Unverified,
    }
}

/// Admit a network whose serving depends on its RPC's chain id (Arc).
///
/// `None` when the RPC answers for another chain: the network is left out of
/// `/supported`. `Some` when it matches, and also when it does not answer in
/// time -- served, with an alert. Never an error, so one network's probe can
/// never stop the process.
pub async fn admit(provider: EvmProvider, timeout: Duration) -> Option<EvmProvider> {
    let network = provider.chain().network();
    match check(&provider, timeout).await {
        Verdict::Matches => Some(provider),
        Verdict::Mismatch { expected, actual } => {
            tracing::error!(
                %network,
                expected,
                actual,
                "[FAIL] evm_rpc_chain_id_mismatch: the RPC answers for another chain; \
                 {network} is not served"
            );
            None
        }
        Verdict::Unverified => {
            tracing::warn!(
                %network,
                "[WARN] arc_rpc_chain_id_unverified: the RPC did not answer eth_chainId \
                 in time; {network} is served without the check"
            );
            Some(provider)
        }
    }
}

/// Check every configured EVM network once, in the background, and log each
/// verdict. Never removes a network and never fails startup.
pub fn spawn(providers: Arc<ProviderCache>) {
    let networks: Vec<_> = providers
        .values()
        .filter_map(|provider| match provider {
            NetworkProvider::Evm(evm) => Some(evm.chain().network()),
            _ => None,
        })
        .collect();
    if networks.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let timeout = crate::chain::rpc_http_timeout();
        let mut checks = tokio::task::JoinSet::new();
        for network in networks {
            let providers = Arc::clone(&providers);
            checks.spawn(async move {
                match providers.by_network(network) {
                    Some(NetworkProvider::Evm(evm)) => Some((network, check(evm, timeout).await)),
                    _ => None,
                }
            });
        }
        let (mut matched, mut mismatched, mut unverified) = (0, 0, 0);
        while let Some(joined) = checks.join_next().await {
            match joined {
                Ok(Some((network, Verdict::Matches))) => {
                    matched += 1;
                    tracing::debug!(%network, "RPC chain id matches");
                }
                Ok(Some((network, Verdict::Mismatch { expected, actual }))) => {
                    mismatched += 1;
                    // One token, no spaces: the metric filter matches it in the
                    // message, where the log's colour codes never land.
                    tracing::warn!(
                        %network,
                        expected,
                        actual,
                        "[WARN] evm_rpc_chain_id_mismatch: the RPC answers for another chain; \
                         the network stays served and its payments will fail on-chain"
                    );
                }
                Ok(Some((network, Verdict::Unverified))) => {
                    unverified += 1;
                    tracing::warn!(%network, "[WARN] RPC chain id could not be read at startup");
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(?error, "[WARN] chain id check task failed"),
            }
        }
        tracing::info!(
            matched,
            mismatched,
            unverified,
            "EVM RPC chain id check finished"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Network;
    use alloy::network::EthereumWallet;
    use alloy::signers::local::PrivateKeySigner;
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};

    /// An RPC that answers `eth_chainId` with `chain_id` and nothing else.
    async fn rpc_answering(chain_id: u64) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/",
            post(move |Json(req): Json<Value>| async move {
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": req.get("id").cloned().unwrap_or(json!(1)),
                    "result": format!("{chain_id:#x}"),
                }))
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/")
    }

    async fn base_on(url: &str) -> EvmProvider {
        EvmProvider::try_new(
            EthereumWallet::from(PrivateKeySigner::random()),
            url,
            true,
            Network::Base,
        )
        .await
        .expect("a mismatched RPC must not stop the provider from being built")
    }

    /// The 2.39.0 shape: the RPC answers for one chain, the table says another.
    /// The provider is still built -- the network stays served -- and the
    /// check names both ids.
    #[tokio::test]
    async fn a_wrong_chain_id_is_reported_and_the_network_is_kept() {
        let url = rpc_answering(44787).await;
        let base = base_on(&url).await;
        assert_eq!(
            check(&base, Duration::from_secs(5)).await,
            Verdict::Mismatch {
                expected: 8453,
                actual: 44787
            }
        );
    }

    #[tokio::test]
    async fn the_right_chain_id_matches() {
        let url = rpc_answering(8453).await;
        let base = base_on(&url).await;
        assert_eq!(check(&base, Duration::from_secs(5)).await, Verdict::Matches);
    }

    /// Nothing listening: not a mismatch, and not a reason to refuse.
    #[tokio::test]
    async fn an_rpc_that_does_not_answer_is_unverified_not_a_mismatch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        let base = base_on(&url).await;
        assert_eq!(
            check(&base, Duration::from_secs(2)).await,
            Verdict::Unverified
        );
    }
}
