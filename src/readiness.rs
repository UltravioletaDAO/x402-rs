//! `GET /health/ready`: whether this task can actually settle, chain by chain.
//!
//! # Why this exists
//!
//! `GET /health` is a constant. It answers `{"status":"healthy"}` whatever
//! happens to the chains, and it has to: the ALB target groups probe it every
//! few seconds, and a load-balancer check that turned red on a chain problem
//! would have ECS replace healthy tasks in a loop while the chain stayed broken.
//!
//! What it cannot do is tell an operator anything. On 2026-09-14 every Base
//! settle failed for hours -- the mainnet signer could no longer cover a
//! transaction's gas reservation -- while `/health` answered 200 in 0.2 s.
//! This route is the other half: liveness stays on `/health`, the truth about
//! settling lives here.
//!
//! # Rules this module keeps
//!
//! 1. **It is not an amplifier.** A probe costs three or four RPC calls per EVM
//!    network. The result is cached for [`ReadinessConfig::ttl`] and refreshed by
//!    one background task at a time, which callers wait on but do not own: a
//!    caller that hangs up mid-probe does not cancel it, and the next caller
//!    joins it or reads the cache it wrote. However hard the route is hit, and
//!    however its callers disconnect, a task sends at most one round of probes
//!    per TTL. The route also sits behind the per-IP governor of the other
//!    on-chain reads. The body says when it was measured.
//! 2. **It never leaks.** No RPC URL (ours carry API keys), no key, no signer
//!    address, no balance. A chain is a name, a bounded status token and an
//!    estimate of how many settles its signers can still admit.
//! 3. **It can go red.** `503` whenever a mainnet cannot settle -- its RPC does
//!    not answer, or a signer is below [`ReadinessConfig::min_settles`]. A route
//!    that can only say "fine" is the one we already had.
//! 4. **It is not for load balancers.** See the first paragraph.
//! 5. **It lists every configured network, whatever state it is in.** A network
//!    is never taken out of `/supported` by its health: this route is where the
//!    health goes. Until 2.39.3 a native Hedera ledger whose probe failed at
//!    startup vanished from both, which hid the problem it was reporting.
//!
//! EVM and native Hedera chains are probed; an EVM RPC is also asked for its
//! chain id on every refresh, so a wrong one reads `rpc_chain_id_mismatch`
//! instead of green. Every other configured family is listed under `unchecked`
//! rather than folded into a green answer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::providers::Provider;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::chain::{NetworkProvider, NetworkProviderOps};
use crate::network::Network;
use crate::provider_cache::ProviderMap;

/// Gas one EIP-3009 settle reserves against the signer's balance.
///
/// Measured: 103,244 and 103,252 `gasUsed` on two Base mainnet settles on
/// 2026-09-14, and the send path sets the limit to the estimate times 5/4
/// (`chain/evm.rs`), so ~129k. The node checks `gasLimit * maxFeePerGas`, not
/// the gas finally used, and that reservation is what refuses a settle.
pub const SETTLE_GAS_BUDGET: u128 = 130_000;

/// Defaults for [`ReadinessConfig`].
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_MIN_SETTLES: u64 = 10;
pub const DEFAULT_WARN_SETTLES: u64 = 100;

/// Tunables, all optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessConfig {
    /// How long a probe result is served before the next caller refreshes it.
    /// `HEALTH_READY_TTL_SECS`, 5-3600.
    pub ttl: Duration,
    /// Bound on one network's probe. A node that accepts the connection and
    /// never answers is a down chain, not a hung route.
    /// `HEALTH_READY_PROBE_TIMEOUT_MS`, 250-30000.
    pub probe_timeout: Duration,
    /// Below this many settles a signer is `down`. `HEALTH_READY_MIN_SETTLES`,
    /// 1-100000: a 0 would switch the gas red off without saying so.
    pub min_settles: u64,
    /// Below this many it is `degraded`. `HEALTH_READY_WARN_SETTLES`, never
    /// below `min_settles`.
    pub warn_settles: u64,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            min_settles: DEFAULT_MIN_SETTLES,
            warn_settles: DEFAULT_WARN_SETTLES,
        }
    }
}

impl ReadinessConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let ttl_secs = env_u64("HEALTH_READY_TTL_SECS", defaults.ttl.as_secs(), 5..=3600);
        let timeout_ms = env_u64(
            "HEALTH_READY_PROBE_TIMEOUT_MS",
            defaults.probe_timeout.as_millis() as u64,
            250..=30_000,
        );
        let min_settles = env_u64(
            "HEALTH_READY_MIN_SETTLES",
            defaults.min_settles,
            1..=100_000,
        );
        let warn_settles = env_u64(
            "HEALTH_READY_WARN_SETTLES",
            defaults.warn_settles,
            0..=u64::MAX,
        )
        .max(min_settles);
        Self {
            ttl: Duration::from_secs(ttl_secs),
            probe_timeout: Duration::from_millis(timeout_ms),
            min_settles,
            warn_settles,
        }
    }
}

fn env_u64(var: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> u64 {
    let Ok(raw) = std::env::var(var) else {
        return default;
    };
    match raw.trim().parse::<u64>() {
        Ok(value) if range.contains(&value) => value,
        _ => {
            tracing::warn!(
                variable = var,
                default,
                "[WARN] readiness setting out of range or not a number; keeping the default"
            );
            default
        }
    }
}

/// Ordered worst-last, so the worst of several is their `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Degraded,
    Down,
}

/// One signer, with nothing that identifies it beyond its position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignerReport {
    pub index: usize,
    pub status: Status,
    /// Whether the balance clears [`ReadinessConfig::min_settles`].
    pub gas_ok: bool,
    /// Settles the balance can still admit at the current fee cap. `None` when
    /// the chain prices gas at zero, where the figure is unbounded.
    pub settles_remaining: Option<u64>,
}

/// One chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkReport {
    pub network: String,
    /// The CAIP-2 id, so a caller holding either spelling finds the chain
    /// (native Hedera appears in `/supported` under this one only).
    pub caip2: String,
    pub mainnet: bool,
    pub status: Status,
    /// Bounded token, present unless `status` is `ok`: `rpc_unreachable`,
    /// `rpc_timeout`, `rpc_chain_id_mismatch`, `signer_gas_critical`,
    /// `signer_gas_low`, and for native Hedera also `signer_key_mismatch` and
    /// `store_unavailable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// `ok`, `unreachable`, `timeout` or `wrong_chain`.
    pub rpc: &'static str,
    pub signers: Vec<SignerReport>,
}

/// How many settles `balance_wei` admits when each reserves
/// [`SETTLE_GAS_BUDGET`] at `fee_cap_wei`.
pub fn settles_remaining(balance_wei: u128, fee_cap_wei: u128) -> Option<u64> {
    let per_settle = SETTLE_GAS_BUDGET.saturating_mul(fee_cap_wei);
    if per_settle == 0 {
        return None;
    }
    Some(u64::try_from(balance_wei / per_settle).unwrap_or(u64::MAX))
}

pub fn grade_signer(
    index: usize,
    balance_wei: u128,
    fee_cap_wei: u128,
    config: &ReadinessConfig,
) -> SignerReport {
    let remaining = settles_remaining(balance_wei, fee_cap_wei);
    let status = match remaining {
        None => Status::Ok,
        Some(n) if n < config.min_settles => Status::Down,
        Some(n) if n < config.warn_settles => Status::Degraded,
        Some(_) => Status::Ok,
    };
    SignerReport {
        index,
        status,
        gas_ok: status != Status::Down,
        settles_remaining: remaining,
    }
}

fn graded_network(network: Network, signers: Vec<SignerReport>) -> NetworkReport {
    let status = signers.iter().map(|s| s.status).max().unwrap_or(Status::Ok);
    let reason = match status {
        Status::Ok => None,
        Status::Degraded => Some("signer_gas_low"),
        Status::Down => Some("signer_gas_critical"),
    };
    NetworkReport {
        network: network.to_string(),
        caip2: network.to_caip2(),
        mainnet: network.is_mainnet(),
        status,
        reason,
        rpc: "ok",
        signers,
    }
}

/// A chain this probe could not grade: `down`, for `reason`, with no signers.
fn down_network(network: Network, reason: &'static str, rpc: &'static str) -> NetworkReport {
    NetworkReport {
        network: network.to_string(),
        caip2: network.to_caip2(),
        mainnet: network.is_mainnet(),
        status: Status::Down,
        reason: Some(reason),
        rpc,
        signers: Vec::new(),
    }
}

fn unreachable_network(network: Network, timed_out: bool) -> NetworkReport {
    if timed_out {
        down_network(network, "rpc_timeout", "timeout")
    } else {
        down_network(network, "rpc_unreachable", "unreachable")
    }
}

/// Grade a native Hedera ledger from its health check: settles the sponsor's
/// HBAR still pays for, or why the check did not pass. `None` when it did not
/// answer within the probe timeout.
#[cfg(feature = "hedera")]
pub fn hedera_report(
    network: Network,
    health: Option<Result<u64, crate::chain::hedera::HealthFailure>>,
    config: &ReadinessConfig,
) -> NetworkReport {
    use crate::chain::hedera::HealthFailure;
    match health {
        Some(Ok(remaining)) => {
            let status = if remaining < config.min_settles {
                Status::Down
            } else if remaining < config.warn_settles {
                Status::Degraded
            } else {
                Status::Ok
            };
            graded_network(
                network,
                vec![SignerReport {
                    index: 0,
                    status,
                    gas_ok: status != Status::Down,
                    settles_remaining: Some(remaining),
                }],
            )
        }
        None | Some(Err(HealthFailure::ConsensusTimeout)) => unreachable_network(network, true),
        Some(Err(failure @ HealthFailure::Unreachable(_))) => {
            down_network(network, failure.reason(), "unreachable")
        }
        // The ledger answered; what it answered is the problem.
        Some(Err(failure)) => down_network(network, failure.reason(), "ok"),
    }
}

/// The answer for the whole task.
///
/// `down` only for a mainnet: a testnet faucet running dry is routine and must
/// not page anyone. Anything short of all-green is `degraded`, and so is having
/// probed nothing at all -- "no chain checked" is not "every chain fine".
pub fn overall(networks: &[NetworkReport]) -> Status {
    if networks.is_empty() {
        return Status::Degraded;
    }
    networks
        .iter()
        .map(|n| match n.status {
            Status::Ok => Status::Ok,
            Status::Down if n.mainnet => Status::Down,
            _ => Status::Degraded,
        })
        .max()
        .unwrap_or(Status::Degraded)
}

/// Read one EVM chain: the chain id its RPC answers for, then the fee cap the
/// send path would set, then every signer's balance, all inside one timeout.
/// A chain id that is not the declared one stops there: balances read on
/// another chain say nothing about this one.
async fn probe_evm(evm: &EvmProvider, config: &ReadinessConfig) -> NetworkReport {
    let network = evm.chain().network();
    let read = async {
        let chain_id = evm.inner().get_chain_id().await?;
        if chain_id != evm.chain().chain_id {
            return Ok(Err(chain_id));
        }
        let fee_cap = evm.quote_fee_cap().await?;
        let mut balances = Vec::with_capacity(evm.signer_addresses().len());
        for address in evm.signer_addresses() {
            let balance = evm.inner().get_balance(*address).await?;
            balances.push(balance.saturating_to::<u128>());
        }
        Ok::<_, alloy::transports::TransportError>(Ok((fee_cap, balances)))
    };
    match tokio::time::timeout(config.probe_timeout, read).await {
        Err(_) => {
            tracing::warn!(%network, "[WARN] readiness probe timed out");
            unreachable_network(network, true)
        }
        Ok(Err(error)) => {
            // Server-side only, and scrubbed: a transport error can quote the
            // RPC URL, key included.
            tracing::warn!(
                %network,
                error = %crate::redact::scrub_urls(&format!("{error:?}")),
                "[WARN] readiness probe could not read the chain"
            );
            unreachable_network(network, false)
        }
        Ok(Ok(Err(actual))) => {
            tracing::warn!(
                %network,
                expected = evm.chain().chain_id,
                actual,
                "[WARN] readiness: the RPC answers for another chain"
            );
            down_network(network, "rpc_chain_id_mismatch", "wrong_chain")
        }
        Ok(Ok(Ok((fee_cap, balances)))) => {
            let signers: Vec<SignerReport> = balances
                .into_iter()
                .enumerate()
                .map(|(index, balance)| grade_signer(index, balance, fee_cap, config))
                .collect();
            let report = graded_network(network, signers);
            if report.status != Status::Ok {
                tracing::warn!(
                    %network,
                    status = ?report.status,
                    reason = report.reason.unwrap_or(""),
                    fee_cap_wei = fee_cap,
                    "[WARN] readiness: signer gas below threshold"
                );
            }
            report
        }
    }
}

#[derive(Clone, Debug)]
struct Snapshot {
    measured: Instant,
    checked_at_unix: u64,
    networks: Vec<NetworkReport>,
    unchecked: Vec<String>,
}

/// Router state: the providers, the settings, the cached probe and the one
/// refresh in flight.
pub struct ReadinessState<P> {
    providers: Arc<P>,
    config: ReadinessConfig,
    cache: std::sync::Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    snapshot: Option<Snapshot>,
    /// Present while a refresh runs. Every caller that needs a fresh probe
    /// waits on this one instead of starting its own.
    inflight: Option<watch::Receiver<Option<Snapshot>>>,
}

impl<P> ReadinessState<P>
where
    P: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    pub fn new(providers: Arc<P>, config: ReadinessConfig) -> Self {
        Self {
            providers,
            config,
            cache: std::sync::Mutex::new(CacheState::default()),
        }
    }

    /// The cached probe, refreshed when older than the TTL. `None` only if the
    /// refresh task died without an answer.
    ///
    /// The refresh runs in its own task and writes the cache itself; a caller
    /// only waits on it. Holding the probe inside the request instead meant a
    /// client that hung up mid-probe cancelled it, and the next caller started
    /// it again: ten requests aborted mid-probe sent ten rounds of RPC calls
    /// (round-1 refutation of this route). Now they share one, and its result
    /// is cached for whoever comes next.
    async fn snapshot(self: &Arc<Self>) -> Option<Snapshot> {
        let mut rx = {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(snapshot) = cache.snapshot.as_ref() {
                if snapshot.measured.elapsed() < self.config.ttl {
                    return Some(snapshot.clone());
                }
            }
            match cache.inflight.as_ref() {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    cache.inflight = Some(rx.clone());
                    let state = Arc::clone(self);
                    tokio::spawn(async move {
                        let fresh = state.probe().await;
                        {
                            let mut cache = state.cache.lock().unwrap_or_else(|p| p.into_inner());
                            cache.snapshot = Some(fresh.clone());
                            cache.inflight = None;
                        }
                        let _ = tx.send(Some(fresh));
                    });
                    rx
                }
            }
        };
        let answer = match rx.wait_for(Option::is_some).await {
            Ok(snapshot) => (*snapshot).clone(),
            Err(_) => None,
        };
        if answer.is_none() {
            // The task ended without sending. Forget it, or every later caller
            // would wait on a channel nobody will ever write to.
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if cache
                .inflight
                .as_ref()
                .is_some_and(|rx| rx.has_changed().is_err())
            {
                cache.inflight = None;
            }
            tracing::warn!("[WARN] readiness refresh ended without a result");
        }
        answer
    }

    async fn probe(&self) -> Snapshot {
        let mut probes = tokio::task::JoinSet::new();
        let mut unchecked = Vec::new();
        for provider in self.providers.values() {
            match provider {
                NetworkProvider::Evm(evm) => {
                    let network = evm.chain().network();
                    let providers = Arc::clone(&self.providers);
                    let config = self.config;
                    probes.spawn(async move {
                        match providers.by_network(network) {
                            Some(NetworkProvider::Evm(evm)) => Some(probe_evm(evm, &config).await),
                            _ => None,
                        }
                    });
                }
                #[cfg(feature = "hedera")]
                NetworkProvider::Hedera(hedera) => {
                    let provider = hedera.clone();
                    let config = self.config;
                    probes.spawn(async move {
                        let network = provider.network();
                        let health = tokio::time::timeout(config.probe_timeout, provider.health())
                            .await
                            .ok();
                        if let Some(Err(failure)) = &health {
                            // Server-side only, and scrubbed, like the EVM probe.
                            tracing::warn!(
                                %network,
                                reason = failure.reason(),
                                detail = %crate::redact::scrub_urls(&failure.to_string()),
                                "[WARN] readiness: the Hedera health check did not pass"
                            );
                        }
                        Some(hedera_report(network, health, &config))
                    });
                }
                other => unchecked.push(other.network().to_string()),
            }
        }
        let mut networks = Vec::new();
        while let Some(joined) = probes.join_next().await {
            match joined {
                Ok(Some(report)) => networks.push(report),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(?error, "[WARN] readiness probe task failed");
                }
            }
        }
        networks.sort_by(|a, b| a.network.cmp(&b.network));
        unchecked.sort();
        Snapshot {
            measured: Instant::now(),
            checked_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            networks,
            unchecked,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadyQuery {
    /// Scope the answer (and the status code) to one chain: `base` or
    /// `eip155:8453`.
    network: Option<String>,
}

/// `GET /health/ready`.
///
/// `200` when the task can settle everywhere it is configured to (`ok`) or
/// everywhere that matters (`degraded`); `503` when a mainnet cannot (`down`).
/// With `?network=`, the status and the code are that chain's alone -- a
/// testnet included, since the caller asked about it by name.
pub async fn get_health_ready<P>(
    State(state): State<Arc<ReadinessState<P>>>,
    Query(query): Query<ReadyQuery>,
) -> Response
where
    P: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    let scope = match query.network.as_deref() {
        None => None,
        Some(raw) => match Network::from_caip2(raw).or_else(|| raw.parse::<Network>().ok()) {
            Some(network) => Some(network.to_string()),
            None => {
                return no_store(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": "unknown_network" }),
                );
            }
        },
    };

    let Some(snapshot) = state.snapshot().await else {
        return no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "down", "error": "probe_failed" }),
        );
    };
    let networks: Vec<&NetworkReport> = match &scope {
        None => snapshot.networks.iter().collect(),
        Some(name) => snapshot
            .networks
            .iter()
            .filter(|n| &n.network == name)
            .collect(),
    };
    if scope.is_some() && networks.is_empty() {
        return no_store(
            StatusCode::NOT_FOUND,
            json!({ "error": "network_not_probed" }),
        );
    }

    let status = match &scope {
        None => overall(&snapshot.networks),
        Some(_) => networks
            .iter()
            .map(|n| n.status)
            .max()
            .unwrap_or(Status::Ok),
    };
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for n in &networks {
        let key = match n.status {
            Status::Ok => "ok",
            Status::Degraded => "degraded",
            Status::Down => "down",
        };
        *counts.entry(key).or_default() += 1;
    }
    let body = json!({
        "status": status,
        "checkedAtUnix": snapshot.checked_at_unix,
        "ageSecs": snapshot.measured.elapsed().as_secs(),
        "ttlSecs": state.config.ttl.as_secs(),
        "probeTimeoutMs": state.config.probe_timeout.as_millis() as u64,
        "thresholds": {
            "minSettles": state.config.min_settles,
            "warnSettles": state.config.warn_settles,
            "settleGasBudget": SETTLE_GAS_BUDGET as u64,
        },
        "summary": {
            "ok": counts.get("ok").copied().unwrap_or(0),
            "degraded": counts.get("degraded").copied().unwrap_or(0),
            "down": counts.get("down").copied().unwrap_or(0),
        },
        "networks": networks,
        "unchecked": if scope.is_some() { Vec::new() } else { snapshot.unchecked.clone() },
    });
    let code = if status == Status::Down {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    no_store(code, body)
}

fn no_store(code: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (code, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Mounted in `main.rs` with its own state, outside the main router's generic
/// facilitator state: all it needs is the provider map.
pub fn routes<P>() -> Router<Arc<ReadinessState<P>>>
where
    P: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    Router::new().route("/health/ready", get(get_health_ready::<P>))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::network::EthereumWallet;
    use alloy::signers::local::PrivateKeySigner;
    use axum::body::Body;
    use axum::http::Request;
    use std::borrow::Borrow;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// The Base mainnet signer at 18:42Z on 2026-09-14, in wei.
    const INCIDENT_BALANCE: u128 = 28_119_576_771_839;
    /// What the send path set on Base that day: 2 * 0.005 gwei + a 1 gwei tip.
    const INCIDENT_FEE_CAP: u128 = 1_010_000_000;

    fn config() -> ReadinessConfig {
        ReadinessConfig {
            ttl: Duration::ZERO,
            probe_timeout: Duration::from_secs(5),
            min_settles: DEFAULT_MIN_SETTLES,
            warn_settles: DEFAULT_WARN_SETTLES,
        }
    }

    #[test]
    fn the_incident_signer_grades_red() {
        let s = grade_signer(0, INCIDENT_BALANCE, INCIDENT_FEE_CAP, &config());
        assert_eq!(s.settles_remaining, Some(0));
        assert_eq!(s.status, Status::Down);
        assert!(!s.gas_ok);
    }

    #[test]
    fn a_funded_signer_grades_green_and_a_thin_one_amber() {
        // 0.05 ETH at a 1.01 gwei cap: 380 settles.
        let green = grade_signer(0, 50_000_000_000_000_000, INCIDENT_FEE_CAP, &config());
        assert_eq!(green.status, Status::Ok);
        assert!(green.gas_ok);
        // 0.005 ETH at the same cap: 38 settles, above the floor of 10.
        let amber = grade_signer(0, 5_000_000_000_000_000, INCIDENT_FEE_CAP, &config());
        assert_eq!(amber.settles_remaining, Some(38));
        assert_eq!(amber.status, Status::Degraded);
        assert!(amber.gas_ok);
    }

    #[test]
    fn a_zero_fee_chain_is_not_graded_on_balance() {
        let s = grade_signer(0, 0, 0, &config());
        assert_eq!(s.settles_remaining, None);
        assert_eq!(s.status, Status::Ok);
    }

    #[test]
    fn only_a_mainnet_takes_the_whole_task_down() {
        let down_testnet = unreachable_network(Network::BaseSepolia, false);
        let down_mainnet = unreachable_network(Network::Base, false);
        let green = graded_network(
            Network::Avalanche,
            vec![grade_signer(0, u128::MAX, 1, &config())],
        );
        assert_eq!(overall(std::slice::from_ref(&green)), Status::Ok);
        assert_eq!(overall(&[green.clone(), down_testnet]), Status::Degraded);
        assert_eq!(overall(&[green, down_mainnet]), Status::Down);
        assert_eq!(
            overall(&[]),
            Status::Degraded,
            "nothing probed is not green"
        );
    }

    // ---- against a JSON-RPC endpoint the test controls ----------------------

    struct MockRpc {
        url: String,
        balance_wei: Arc<std::sync::Mutex<u128>>,
        calls: Arc<AtomicUsize>,
        /// While set, every call hangs for a minute.
        hang: Arc<AtomicBool>,
    }

    async fn mock_rpc(balance_wei: u128, hang: bool) -> MockRpc {
        let balance = Arc::new(std::sync::Mutex::new(balance_wei));
        let calls = Arc::new(AtomicUsize::new(0));
        let hanging = Arc::new(AtomicBool::new(hang));
        let (b, c, h) = (
            Arc::clone(&balance),
            Arc::clone(&calls),
            Arc::clone(&hanging),
        );
        let app = Router::new().route(
            "/",
            axum::routing::post(move |Json(req): Json<serde_json::Value>| {
                let (b, c, h) = (Arc::clone(&b), Arc::clone(&c), Arc::clone(&h));
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    if h.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    let result = match req["method"].as_str().unwrap_or_default() {
                        "eth_chainId" => json!("0x2105"),
                        "eth_feeHistory" => json!({
                            "oldestBlock": "0x1",
                            "baseFeePerGas": ["0x4c4b40", "0x4c4b40"],
                            "gasUsedRatio": [0.5]
                        }),
                        // 1 gwei: at or above every tip floor the fee table
                        // sets, so the cap these tests expect does not move
                        // when that table does.
                        "eth_maxPriorityFeePerGas" => json!("0x3b9aca00"),
                        "eth_gasPrice" => json!("0x3c336080"),
                        "eth_getBalance" => json!(format!("{:#x}", *b.lock().unwrap())),
                        other => panic!("unexpected RPC method {other}"),
                    };
                    Json(json!({ "jsonrpc": "2.0", "id": req["id"], "result": result }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockRpc {
            url,
            balance_wei: balance,
            calls,
            hang: hanging,
        }
    }

    /// An address nothing listens on.
    async fn dead_url() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        url
    }

    struct Providers(HashMap<Network, NetworkProvider>);

    impl ProviderMap for Providers {
        type Value = NetworkProvider;
        fn by_network<N: Borrow<Network>>(&self, network: N) -> Option<&NetworkProvider> {
            self.0.get(network.borrow())
        }
        fn values(&self) -> impl Iterator<Item = &NetworkProvider> + Send {
            self.0.values()
        }
    }

    async fn router_for(
        chains: &[(Network, &str)],
        config: ReadinessConfig,
    ) -> (Router, Vec<String>) {
        let mut map = HashMap::new();
        let mut addresses = Vec::new();
        for (network, url) in chains {
            let signer = PrivateKeySigner::random();
            addresses.push(format!("{:x}", signer.address()));
            let provider = EvmProvider::try_new(EthereumWallet::from(signer), url, true, *network)
                .await
                .expect("provider");
            map.insert(*network, NetworkProvider::Evm(provider));
        }
        let state = Arc::new(ReadinessState::new(Arc::new(Providers(map)), config));
        (routes::<Providers>().with_state(state), addresses)
    }

    async fn get(router: &Router, uri: &str) -> (StatusCode, serde_json::Value, String) {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        (status, serde_json::from_str(&text).unwrap(), text)
    }

    /// Criterion 2 of the incident: green with the RPC answering and the
    /// signer funded; red with the RPC gone; red with the signer drained to
    /// exactly what the Base signer held on 2026-09-14.
    #[tokio::test]
    async fn it_turns_red_when_the_rpc_or_the_gas_goes_away() {
        let rpc = mock_rpc(50_000_000_000_000_000, false).await;
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config()).await;

        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body["status"], "ok");
        assert_eq!(body["networks"][0]["network"], "base");
        assert_eq!(body["networks"][0]["rpc"], "ok");
        assert_eq!(body["networks"][0]["signers"][0]["gasOk"], true);
        assert_eq!(body["networks"][0]["signers"][0]["settlesRemaining"], 380);

        *rpc.balance_wei.lock().unwrap() = INCIDENT_BALANCE;
        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["status"], "down");
        assert_eq!(body["networks"][0]["reason"], "signer_gas_critical");
        assert_eq!(body["networks"][0]["signers"][0]["gasOk"], false);
        assert_eq!(body["networks"][0]["signers"][0]["settlesRemaining"], 0);

        let dead = dead_url().await;
        let (router, _) = router_for(&[(Network::Base, dead.as_str())], config()).await;
        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["status"], "down");
        assert_eq!(body["networks"][0]["rpc"], "unreachable");
        assert_eq!(body["networks"][0]["reason"], "rpc_unreachable");
    }

    /// A node that accepts the connection and never answers is down, and the
    /// route answers within the probe timeout instead of hanging with it.
    #[tokio::test]
    async fn a_node_that_never_answers_is_down_not_a_hung_route() {
        let rpc = mock_rpc(50_000_000_000_000_000, true).await;
        let config = ReadinessConfig {
            probe_timeout: Duration::from_millis(300),
            ..config()
        };
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config).await;
        let started = std::time::Instant::now();
        let (code, body, _) = get(&router, "/health/ready").await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["networks"][0]["reason"], "rpc_timeout");
    }

    /// Criterion 3: no URL, no address, no balance -- in either state.
    #[tokio::test]
    async fn the_answer_names_no_endpoint_address_or_balance() {
        let rpc = mock_rpc(INCIDENT_BALANCE, false).await;
        let dead = dead_url().await;
        let (router, addresses) = router_for(
            &[
                (Network::Base, rpc.url.as_str()),
                (Network::Avalanche, dead.as_str()),
            ],
            config(),
        )
        .await;
        let (_, _, text) = get(&router, "/health/ready").await;
        let lower = text.to_ascii_lowercase();
        for forbidden in [
            "http",
            "127.0.0.1",
            rpc.url.as_str(),
            dead.as_str(),
            &INCIDENT_BALANCE.to_string(),
            &format!("{INCIDENT_BALANCE:x}"),
        ] {
            assert!(!lower.contains(forbidden), "{forbidden} leaked: {text}");
        }
        for address in addresses {
            assert!(!lower.contains(&address), "signer address leaked: {text}");
        }
    }

    /// Trap 1: however often it is called, one probe per TTL.
    #[tokio::test]
    async fn a_cached_answer_does_not_touch_the_rpc() {
        let rpc = mock_rpc(50_000_000_000_000_000, false).await;
        let config = ReadinessConfig {
            ttl: Duration::from_secs(60),
            ..config()
        };
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config).await;
        let (_, first, _) = get(&router, "/health/ready").await;
        let after_first = rpc.calls.load(Ordering::SeqCst);
        assert!(after_first > 0);
        for _ in 0..20 {
            get(&router, "/health/ready").await;
        }
        assert_eq!(rpc.calls.load(Ordering::SeqCst), after_first);
        assert_eq!(first["ttlSecs"], 60);
        assert_eq!(first["probeTimeoutMs"], 5000);
    }

    /// Twenty callers on an empty cache share one probe. From the round-1
    /// refutation, where the only test that caught a lock released before the
    /// probe was this one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_share_one_probe() {
        let rpc = mock_rpc(50_000_000_000_000_000, false).await;
        let config = ReadinessConfig {
            ttl: Duration::from_secs(60),
            ..config()
        };
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config).await;
        let before = rpc.calls.load(Ordering::SeqCst);
        let mut handles = Vec::new();
        for _ in 0..20 {
            let r = router.clone();
            handles.push(tokio::spawn(
                async move { get(&r, "/health/ready").await.0 },
            ));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), StatusCode::OK);
        }
        let used = rpc.calls.load(Ordering::SeqCst) - before;
        assert!(used <= 4, "20 concurrent callers sent {used} RPC calls");
    }

    /// Callers that hang up mid-probe must not restart it. From the round-1
    /// refutation: with the probe inside the request, ten requests aborted at
    /// 150 ms each sent ten rounds of RPC calls.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callers_that_hang_up_mid_probe_share_one_probe() {
        let rpc = mock_rpc(50_000_000_000_000_000, true).await;
        let config = ReadinessConfig {
            ttl: Duration::from_secs(60),
            probe_timeout: Duration::from_secs(5),
            ..config()
        };
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config).await;
        let before = rpc.calls.load(Ordering::SeqCst);
        for _ in 0..10 {
            let r = router.clone();
            let h = tokio::spawn(async move { get(&r, "/health/ready").await.0 });
            tokio::time::sleep(Duration::from_millis(150)).await;
            h.abort();
            let _ = h.await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let used = rpc.calls.load(Ordering::SeqCst) - before;
        assert!(used <= 1, "10 aborted callers sent {used} RPC calls");
    }

    /// A 0 would switch the gas red off in silence; it keeps the default.
    #[test]
    fn a_zero_min_settles_cannot_switch_the_gas_red_off() {
        std::env::set_var("HEALTH_READY_MIN_SETTLES", "0");
        let from_env = ReadinessConfig::from_env();
        std::env::remove_var("HEALTH_READY_MIN_SETTLES");
        assert_eq!(from_env.min_settles, DEFAULT_MIN_SETTLES);
    }

    /// `?network=` scopes the verdict: a testnet asked for by name can be the
    /// 503, and a chain that was not probed is a 404, never a green answer.
    #[tokio::test]
    async fn a_scoped_answer_is_that_chains_alone() {
        let rpc = mock_rpc(50_000_000_000_000_000, false).await;
        let dead = dead_url().await;
        let (router, _) = router_for(
            &[
                (Network::Base, rpc.url.as_str()),
                (Network::BaseSepolia, dead.as_str()),
            ],
            config(),
        )
        .await;

        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::OK, "a dead testnet does not page: {body}");
        assert_eq!(body["status"], "degraded");

        let (code, body, _) = get(&router, "/health/ready?network=eip155:8453").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body["status"], "ok");

        let (code, body, _) = get(&router, "/health/ready?network=base-sepolia").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");

        let (code, _, _) = get(&router, "/health/ready?network=avalanche").await;
        assert_eq!(code, StatusCode::NOT_FOUND);

        let (code, _, _) = get(&router, "/health/ready?network=not-a-chain").await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }

    /// An RPC that answers for another chain is not green: its balances were
    /// read on the wrong chain. Listed, `down`, with the reason, under both
    /// spellings of the network. Arc here, whose mismatch until 2.39.3 took it
    /// out of /supported and so out of this route too.
    #[tokio::test]
    async fn a_wrong_chain_id_is_listed_down_not_green() {
        // The mock answers eth_chainId for Base (0x2105), not for Arc.
        let rpc = mock_rpc(50_000_000_000_000_000, false).await;
        let (router, _) = router_for(&[(Network::Arc, rpc.url.as_str())], config()).await;
        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let arc = &body["networks"][0];
        assert_eq!(arc["network"], "arc", "{body}");
        assert_eq!(arc["caip2"], "eip155:5042", "{body}");
        assert_eq!(arc["status"], "down", "{body}");
        assert_eq!(arc["reason"], "rpc_chain_id_mismatch", "{body}");
        assert_eq!(arc["rpc"], "wrong_chain", "{body}");
        assert_eq!(arc["signers"], serde_json::json!([]), "{body}");
    }

    /// A probe that timed out is not a verdict for the life of the task: the
    /// next refresh measures again, and the chain turns green on its own, with
    /// no restart and no deploy.
    #[tokio::test]
    async fn a_chain_whose_probe_timed_out_turns_green_without_a_restart() {
        let rpc = mock_rpc(50_000_000_000_000_000, true).await;
        let config = ReadinessConfig {
            probe_timeout: Duration::from_millis(300),
            ..config()
        };
        let (router, _) = router_for(&[(Network::Base, rpc.url.as_str())], config).await;
        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(
            body["networks"][0]["network"], "base",
            "listed while down: {body}"
        );
        assert_eq!(body["networks"][0]["reason"], "rpc_timeout");

        rpc.hang.store(false, Ordering::SeqCst);
        let (code, body, _) = get(&router, "/health/ready").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body["networks"][0]["status"], "ok", "{body}");
    }

    /// Native Hedera's health check, graded: the reason is the ledger's, never
    /// a generic `rpc_unreachable` for a key or a balance problem.
    #[cfg(feature = "hedera")]
    #[test]
    fn a_hedera_health_answer_grades_with_its_own_reason() {
        use crate::chain::hedera::HealthFailure;
        let network = Network::Hedera;
        let report = |health| hedera_report(network, health, &config());
        let key = report(Some(Err(HealthFailure::SponsorKeyMismatch)));
        assert_eq!(
            (key.status, key.reason, key.rpc),
            (Status::Down, Some("signer_key_mismatch"), "ok")
        );
        assert_eq!(key.network, "hedera");
        assert_eq!(key.caip2, "hedera:mainnet");
        let store = report(Some(Err(HealthFailure::StoreUnavailable("x".into()))));
        assert_eq!(store.reason, Some("store_unavailable"));
        let timeout = report(Some(Err(HealthFailure::ConsensusTimeout)));
        assert_eq!(
            (timeout.reason, timeout.rpc),
            (Some("rpc_timeout"), "timeout")
        );
        let slow = report(None);
        assert_eq!((slow.reason, slow.rpc), (Some("rpc_timeout"), "timeout"));
        let gone = report(Some(Err(HealthFailure::Unreachable("x".into()))));
        assert_eq!(
            (gone.reason, gone.rpc),
            (Some("rpc_unreachable"), "unreachable")
        );
        assert_eq!(report(Some(Ok(0))).reason, Some("signer_gas_critical"));
        assert_eq!(report(Some(Ok(28))).reason, Some("signer_gas_low"));
        assert_eq!(report(Some(Ok(500))).status, Status::Ok);
    }

    /// A native Hedera ledger whose health check fails is listed with its
    /// state and reason -- refused connection or no answer in time -- and
    /// scoped by its CAIP-2 id, the only spelling `/supported` gives it. Until
    /// 2.39.3 a ledger that failed at startup was absent from this route.
    #[cfg(feature = "hedera")]
    #[tokio::test]
    async fn a_hedera_ledger_that_fails_its_health_is_listed_with_its_reason() {
        use crate::chain::hedera::HederaProvider;
        let closed = || {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let nodes = |port: u16| {
            HashMap::from([(
                format!("127.0.0.1:{port}"),
                hiero_sdk::AccountId::new(0, 0, 3),
            )])
        };
        // Accepts connections and never answers: TLS and gRPC both wait.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let silent_port = silent.local_addr().unwrap().port();
        for (port, reason) in [(closed(), "rpc_unreachable"), (silent_port, "rpc_timeout")] {
            let mirror = format!("https://127.0.0.1:{port}/").parse().unwrap();
            let provider =
                HederaProvider::for_health_tests(Network::HederaTestnet, mirror, nodes(port));
            let map = HashMap::from([(Network::HederaTestnet, NetworkProvider::Hedera(provider))]);
            let config = ReadinessConfig {
                probe_timeout: Duration::from_millis(500),
                ..config()
            };
            let state = Arc::new(ReadinessState::new(Arc::new(Providers(map)), config));
            let router = routes::<Providers>().with_state(state);

            let (code, body, _) = get(&router, "/health/ready").await;
            assert_eq!(
                code,
                StatusCode::OK,
                "a testnet does not take the task down: {body}"
            );
            let ledger = &body["networks"][0];
            assert_eq!(ledger["network"], "hedera-testnet", "{body}");
            assert_eq!(ledger["caip2"], "hedera:testnet", "{body}");
            assert_eq!(ledger["status"], "down", "{body}");
            assert_eq!(ledger["reason"], reason, "{body}");

            let (code, body, _) = get(&router, "/health/ready?network=hedera:testnet").await;
            assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert_eq!(body["networks"][0]["reason"], reason, "{body}");
        }
        drop(silent);
    }

    /// The low-balance alarm is the page for the moment this route turns a
    /// signer `degraded`, so `alerts.tf` derives its floor from the same
    /// numbers: `SETTLE_GAS_BUDGET * fee_cap * warnSettles`. Terraform cannot
    /// read Rust and carries copies; this fails when a copy drifts, or when the
    /// deployment sets the warn level the copy assumes is the default.
    ///
    /// Until 2.39.2 every floor was typed by hand and its description promised
    /// "roughly 100 settles": Arc's default bought about 19.
    #[test]
    fn the_low_balance_alarm_is_derived_from_these_thresholds() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/terraform/environments/production"
        );
        let alerts = std::fs::read_to_string(format!("{dir}/alerts.tf")).expect("alerts.tf");
        let local = |name: &str| -> u128 {
            alerts
                .lines()
                .map(str::trim)
                .find_map(|line| {
                    let (key, value) = line.split_once('=')?;
                    (key.trim() == name).then(|| value.trim().parse().ok())?
                })
                .unwrap_or_else(|| panic!("alerts.tf no longer declares `{name} = <integer>`"))
        };
        assert_eq!(
            local("settle_gas_budget"),
            SETTLE_GAS_BUDGET,
            "alerts.tf prices a settle differently from /health/ready"
        );
        assert_eq!(
            local("warn_settles"),
            u128::from(DEFAULT_WARN_SETTLES),
            "alerts.tf pages at a different settle count than /health/ready warns at"
        );

        // A deployment override would move /health/ready and leave the alarm
        // on the default. Carry it into alerts.tf instead of setting it here.
        let overridden = [
            "HEALTH_READY_WARN_SETTLES",
            "HEDERA_MAX_TRANSACTION_FEE_TINYBARS",
        ];
        for entry in std::fs::read_dir(dir).expect("terraform directory") {
            let path = entry.expect("directory entry").path();
            if path.extension().is_some_and(|e| e == "tf" || e == "tfvars") {
                let text = std::fs::read_to_string(&path).expect("terraform file");
                for name in overridden {
                    // Quoted: an ECS `environment` entry, not a comment naming it.
                    assert!(
                        !text.contains(&format!("\"{name}\"")),
                        "{} sets {name}; alerts.tf derives its floors from the default",
                        path.display()
                    );
                }
            }
        }

        #[cfg(feature = "hedera")]
        {
            let cost: u64 = alerts
                .split_once(r#""hedera-mainnet" = { cost = "#)
                .and_then(|(_, rest)| rest.split(',').next())
                .and_then(|value| value.trim().parse().ok())
                .expect("alerts.tf prices a Hedera settle in whole HBAR");
            assert_eq!(
                cost * 100_000_000,
                crate::chain::hedera::DEFAULT_MAX_TRANSACTION_FEE_TINYBARS,
                "alerts.tf prices a Hedera settle differently from its max transaction fee"
            );
        }
    }

    /// The derivation only RAISES a low-balance floor, never lowers one
    /// (c0der, 2026-09-23): the alarm of each derived chain fires at
    /// `max(derived, declared)`, and the declared floors are at least the
    /// operator's floors of 2026-09-23, which may rise but not fall. Without
    /// this, removing the `max` from `alerts.tf` left every test green while
    /// Base's floor fell from 0.005 to 0.000143 ETH (refutation of PR #99).
    ///
    /// Also: every fee cap is above zero, because `alerts.tf` divides by it and
    /// a zero would fail the Terraform plan after the merge.
    #[test]
    fn no_balance_floor_falls_below_the_operator_floor() {
        // The thresholds production ran on 2026-09-23 (`describe-alarms`), in
        // native units. Arc's comes from production.auto.tfvars.
        const DECLARED: [(&str, f64); 9] = [
            ("celo-mainnet", 12.0),
            ("ethereum-mainnet", 0.0035),
            ("arbitrum-mainnet", 0.0025),
            ("polygon-mainnet", 20.0),
            ("base-mainnet", 0.005),
            ("optimism-mainnet", 0.005),
            ("avalanche-mainnet", 0.2),
            ("monad-mainnet", 6.0),
            ("hedera-mainnet", 10.0),
        ];
        const HAND_SET: [(&str, f64); 6] = [
            ("sui-mainnet", 1.0),
            ("solana-mainnet", 0.02),
            ("stellar-mainnet", 5.0),
            ("near-mainnet", 1.0),
            ("algorand-mainnet", 5.0),
            ("xrpl-mainnet", 5.0),
        ];
        const ARC_OPERATOR_FLOOR: f64 = 5.0;

        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/terraform/environments/production"
        );
        let alerts = std::fs::read_to_string(format!("{dir}/alerts.tf")).expect("alerts.tf");
        // `"key" = number` entries of the block opened by `opening`.
        let block = |opening: &str| -> Vec<(String, f64)> {
            let start = alerts
                .find(opening)
                .unwrap_or_else(|| panic!("alerts.tf no longer declares `{opening}`"));
            alerts[start + opening.len()..]
                .lines()
                .skip(1)
                .map(str::trim)
                .take_while(|line| !line.starts_with('}'))
                .filter_map(|line| {
                    let (key, rest) = line.split_once('=')?;
                    let value = rest.split('#').next()?.trim().parse().ok()?;
                    Some((key.trim().trim_matches('"').to_string(), value))
                })
                .collect()
        };
        let declared = block("declared_floors = merge({");
        let hand_set = block("hand_set_floors = {");
        let fee_caps = block("evm_fee_cap_gwei = {");

        for (chain, floor) in DECLARED {
            let now = declared
                .iter()
                .find(|(c, _)| c == chain)
                .unwrap_or_else(|| panic!("{chain} lost its declared floor"))
                .1;
            assert!(
                now >= floor,
                "{chain}: declared floor {now} is below {floor}"
            );
        }
        for (chain, floor) in HAND_SET {
            let now = hand_set
                .iter()
                .find(|(c, _)| c == chain)
                .unwrap_or_else(|| panic!("{chain} lost its hand-set floor"))
                .1;
            assert!(
                now >= floor,
                "{chain}: hand-set floor {now} is below {floor}"
            );
        }

        // The declared floor has to reach the threshold: `min_native` is the
        // max of the derived floor and the declared one.
        let min_native = alerts
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("min_native") && line.contains("price.cost"))
            .expect("alerts.tf no longer derives min_native from the settle price");
        assert!(
            min_native.contains("max(")
                && min_native.contains("price.cost * local.warn_settles")
                && min_native.contains("local.declared_floors"),
            "min_native must be max(derived, declared): {min_native}"
        );

        let tfvars = std::fs::read_to_string(format!("{dir}/production.auto.tfvars"))
            .expect("production.auto.tfvars");
        let arc: f64 = tfvars
            .lines()
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim() == "arc_minimum_gas_usdc")
            .and_then(|(_, value)| value.split('#').next()?.trim().parse().ok())
            .expect("production.auto.tfvars no longer sets arc_minimum_gas_usdc");
        assert!(
            arc >= ARC_OPERATOR_FLOOR,
            "Arc's floor {arc} is below 5 USDC"
        );

        assert_eq!(fee_caps.len(), 9, "one fee cap per alarmed EVM mainnet");
        for (chain, gwei) in &fee_caps {
            assert!(
                *gwei > 0.0,
                "{chain}: fee cap {gwei} gwei; alerts.tf divides by it"
            );
        }
    }
}
