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
//! # An alert, never a refusal
//!
//! A supported network is never taken out of `/supported` by what its RPC
//! answers. A mismatch is logged and counted, and `/health/ready` reports the
//! network `down` with `rpc_chain_id_mismatch` for as long as it lasts (it asks
//! `eth_chainId` on every refresh). Had this check refused, 2.39.0 would have
//! switched both testnets off on deploy, over a number in our own table. A
//! wrong RPC does not settle a payment on the wrong chain either: the token's
//! own domain is checked on-chain, so the payment fails and the alert says why.
//!
//! That includes Arc. Through 2.39.3 an Arc RPC answering for another chain
//! left Arc out of `/supported` until the next deploy. Arc still recovers every
//! signature locally under its configured chain id before anything is
//! estimated or sent (`chain/evm.rs`), so a signature for the other Arc network
//! is refused whatever the RPC answers, and a signature for this one fails the
//! other chain's own domain check.
//!
//! The check runs in the background ([`spawn`]), so a slow RPC delays nothing.
//! An RPC that does not answer is asked again ([`crate::chain::reprobe_delay`])
//! until it does, and the verdict it then gives is logged.
//!
//! Alert tokens, counted by the CloudWatch metric filter in
//! `alerts-network-startup.tf`: `evm_rpc_chain_id_mismatch` (any network) and
//! `arc_rpc_chain_id_unverified` (Arc's RPC did not answer at startup). Native
//! Hedera's startup health check follows the same rule and logs
//! `hedera_health_failed_at_startup` (`src/chain/hedera/mod.rs`).

use std::sync::Arc;
use std::time::Duration;

use alloy::providers::Provider;

use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::chain::NetworkProvider;
use crate::network::Network;
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

fn log_mismatch(network: Network, expected: u64, actual: u64) {
    // One token, no spaces: the metric filter matches it in the message, where
    // the log's colour codes never land.
    tracing::error!(
        %network,
        expected,
        actual,
        "[FAIL] evm_rpc_chain_id_mismatch: the RPC answers for another chain; \
         {network} stays served and its payments fail until the RPC or the declared id is fixed"
    );
}

/// Check every configured EVM network once, in the background, and log each
/// verdict. An RPC that does not answer is asked again until it does. Never
/// removes a network and never fails startup.
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
                    log_mismatch(network, expected, actual);
                }
                Ok(Some((network, Verdict::Unverified))) => {
                    unverified += 1;
                    if matches!(network, Network::Arc | Network::ArcTestnet) {
                        tracing::warn!(
                            %network,
                            "[WARN] arc_rpc_chain_id_unverified: the RPC did not answer \
                             eth_chainId in time; {network} is served and asked again"
                        );
                    } else {
                        tracing::warn!(
                            %network,
                            "[WARN] RPC chain id could not be read at startup; asked again"
                        );
                    }
                    let providers = Arc::clone(&providers);
                    tokio::spawn(async move {
                        if let Some(NetworkProvider::Evm(evm)) = providers.by_network(network) {
                            recheck(evm, timeout, crate::chain::reprobe_delay).await;
                        }
                    });
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

/// Ask an RPC that did not answer at startup again, after `delay(attempt)`
/// each time (every 30-60 s in production), until it gives a verdict; log that
/// verdict and return it with the attempts it took.
async fn recheck(
    evm: &EvmProvider,
    timeout: Duration,
    delay: impl Fn(u32) -> Duration,
) -> (Verdict, u32) {
    let network = evm.chain().network();
    let (verdict, attempts) = crate::chain::reprobe(
        || async {
            match check(evm, timeout).await {
                Verdict::Unverified => {
                    tracing::debug!(%network, "RPC chain id still unanswered");
                    None
                }
                verdict => Some(verdict),
            }
        },
        delay,
    )
    .await;
    match verdict {
        Verdict::Mismatch { expected, actual } => log_mismatch(network, expected, actual),
        _ => tracing::info!(
            %network,
            attempts,
            "[OK] rpc_chain_id_verified: the RPC answers for the chain we sign for"
        ),
    }
    (verdict, attempts)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The loop itself: it keeps asking until the probe gives a verdict, and
    /// waits the schedule's delay before each attempt. Without it, an RPC that
    /// missed the startup check would never be judged.
    #[tokio::test]
    async fn the_reprobe_loop_asks_until_it_gets_a_verdict() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        let delays = std::sync::Mutex::new(Vec::new());
        let (verdict, attempts) = tokio::time::timeout(
            Duration::from_secs(5),
            crate::chain::reprobe(
                || async {
                    let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (n == 3).then_some("answered")
                },
                |attempt| {
                    delays.lock().unwrap().push(attempt);
                    Duration::from_millis(1)
                },
            ),
        )
        .await
        .expect("the loop stopped asking before it got a verdict");
        assert_eq!((verdict, attempts), ("answered", 4));
        assert_eq!(calls.into_inner(), 4);
        assert_eq!(delays.into_inner().unwrap(), [0, 1, 2, 3]);
    }

    /// The EVM re-check end to end: an RPC that cannot answer `eth_chainId`
    /// twice, then answers for the right chain, is judged on the third try; one
    /// that then answers for another chain is judged a mismatch.
    #[tokio::test]
    async fn an_rpc_that_did_not_answer_at_startup_is_judged_when_it_does() {
        for (answer, expected) in [
            (8453u64, Verdict::Matches),
            (
                44787,
                Verdict::Mismatch {
                    expected: 8453,
                    actual: 44787,
                },
            ),
        ] {
            let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let seen = Arc::clone(&calls);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let app = Router::new().route(
                "/",
                post(move |Json(req): Json<Value>| {
                    let seen = Arc::clone(&seen);
                    async move {
                        // Twice an answer that is not a chain id, then a real one.
                        let result = if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                            json!("not-a-chain-id")
                        } else {
                            json!(format!("{answer:#x}"))
                        };
                        Json(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
                    }
                }),
            );
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let base = base_on(&url).await;
            let (verdict, attempts) = tokio::time::timeout(
                Duration::from_secs(10),
                recheck(&base, Duration::from_secs(2), |_| Duration::from_millis(1)),
            )
            .await
            .expect("the re-check stopped asking before it got a verdict");
            assert_eq!((verdict, attempts), (expected, 3));
        }
    }

    /// Bounded: never faster than every 30 s, never slower than every 60 s.
    #[test]
    fn the_reprobe_backs_off_to_a_minute_and_stays_there() {
        let delays: Vec<u64> = (0..5)
            .map(|attempt| crate::chain::reprobe_delay(attempt).as_secs())
            .collect();
        assert_eq!(delays, [30, 60, 60, 60, 60]);
    }
}
