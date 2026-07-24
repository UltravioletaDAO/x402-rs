//! ERC-8004 attested curation (WS-E) — the differentiator.
//!
//! Turns the health prober's uptime data into ON-CHAIN, independently-verifiable
//! reputation: the facilitator's prober wallet writes ERC-8004
//! `giveFeedback(agentId, uptimeBps, 2, "uptime", …)` for curated products that
//! have an ERC-8004 identity, and reads `getSummary` back so the bazaar can show
//! a trustless `verification` badge. Nobody else ships probe-derived, on-chain
//! curation.
//!
//! **On-chain writes are gated behind `ENABLE_BAZAAR_ATTESTATIONS` (default
//! OFF).** With the flag off, the reputation READER still runs (RPC reads are
//! free) so the `verification` field reflects any existing on-chain reputation,
//! but no gas is ever spent. Reuses the exact `IReputationRegistry` +
//! provider/gas pattern from the `/feedback` handler.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::chain::NetworkProvider;
use crate::erc8004::{get_contracts, IIdentityRegistry, IReputationRegistry};
use crate::network::Network;
use crate::provider_cache::ProviderMap;
use crate::types_v2::VerificationInfo;

/// Tag used for uptime feedback (matches the ERC-8004 example vocabulary).
const UPTIME_TAG: &str = "uptime";
/// Uptime is a percentage with 2 decimals (basis points / 100): 99.77% -> 9977.
const UPTIME_DECIMALS: u8 = 2;

/// Runtime config for attested curation.
#[derive(Debug, Clone)]
pub struct AttestationConfig {
    /// Master switch for ON-CHAIN writes (default false — reads still happen).
    pub enabled: bool,
    /// The prober/reviewer address whose feedback consumers trust. Used to scope
    /// `getSummary` so third parties verify our claims without trusting our API.
    pub reviewer: Option<Address>,
    /// Refresh cadence (seconds) for the reputation cache + attestations.
    pub interval_secs: u64,
    /// Public base URL for hosted evidence files.
    pub evidence_base: String,
}

impl AttestationConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("ENABLE_BAZAAR_ATTESTATIONS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let reviewer = std::env::var("BAZAAR_ATTESTATION_REVIEWER")
            .ok()
            .and_then(|s| s.parse::<Address>().ok());
        let interval_secs = std::env::var("BAZAAR_ATTESTATION_INTERVAL")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(86_400);
        let evidence_base = std::env::var("FACILITATOR_URL")
            .unwrap_or_else(|_| "https://facilitator.ultravioletadao.xyz".to_string());
        Self {
            enabled,
            reviewer,
            interval_secs,
            evidence_base,
        }
    }
}

/// A curated product with an ERC-8004 identity to attest/verify.
#[derive(Debug, Clone)]
pub struct AttestTarget {
    /// Manifest entry name — the key of the verification cache. Keyed by label
    /// (not URL) so the annotation joins regardless of URL variants such as a
    /// trailing slash.
    pub label: String,
    /// Representative resource URL prefix (on-chain feedback `endpoint`, and
    /// the prefix used to aggregate probe uptime).
    pub url: String,
    pub network: Network,
    pub agent_id: u64,
}

/// SHA-256 hex key for an evidence object (never embeds the raw URL — F9).
pub fn evidence_key(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    hex::encode(h.finalize())
}

/// keccak256-free content hash committed on-chain (`feedbackHash`).
pub fn evidence_hash(body: &[u8]) -> B256 {
    let mut h = Sha256::new();
    h.update(body);
    B256::from_slice(&h.finalize())
}

/// Build the off-chain evidence JSON for an uptime attestation.
pub fn evidence_json(target: &AttestTarget, uptime_bps: u16, probes: u64, oks: u64) -> Vec<u8> {
    let doc = serde_json::json!({
        "type": "uptime",
        "endpoint": target.url,
        "network": target.network.to_string(),
        "agentId": target.agent_id,
        "uptime": uptime_bps as f64 / 100.0,
        "window": { "probes": probes, "ok": oks },
        "prober": "uvd-bazaar-health/1.0",
    });
    serde_json::to_vec(&doc).unwrap_or_default()
}

fn evm_provider<'a>(
    map: &'a impl ProviderMap<Value = NetworkProvider>,
    net: &Network,
) -> Option<&'a EvmProvider> {
    match map.by_network(net) {
        Some(NetworkProvider::Evm(p)) => Some(p),
        _ => None,
    }
}

/// Read the on-chain verification for `agent_id`: first confirm the ERC-8004
/// identity exists (`ownerOf`), then — only when a reviewer is configured —
/// read the reputation `getSummary` scoped to that reviewer (the contract
/// requires a non-empty `clientAddresses`, so an unscoped summary is not
/// possible). Returns `(feedback_count, summary_value, decimals)` when the
/// identity exists (`(0, 0, _)` when there is no reviewer/feedback), or `None`
/// when the identity does not exist. RPC reads only — no gas.
pub async fn read_reputation(
    provider: &EvmProvider,
    identity_registry: Address,
    reputation_registry: Address,
    agent_id: u64,
    reviewer: Option<Address>,
) -> Option<(u64, i128, u8)> {
    // 1) Identity must exist (owner != zero).
    let identity = IIdentityRegistry::new(identity_registry, provider.inner().clone());
    match identity.ownerOf(U256::from(agent_id)).call().await {
        Ok(owner) if owner != Address::ZERO => {}
        Ok(_) => return None,
        Err(e) => {
            warn!(agent_id, error = %format!("{e:?}"), "ownerOf read failed");
            return None;
        }
    }

    // 2) Reputation summary is only queryable with a concrete reviewer set.
    let Some(reviewer) = reviewer else {
        return Some((0, 0, UPTIME_DECIMALS));
    };
    let reg = IReputationRegistry::new(reputation_registry, provider.inner().clone());
    match reg
        .getSummary(
            U256::from(agent_id),
            vec![reviewer],
            UPTIME_TAG.to_string(),
            String::new(),
        )
        .call()
        .await
    {
        Ok(r) => Some((r.count, r.summaryValue, r.summaryValueDecimals)),
        Err(e) => {
            warn!(agent_id, error = %format!("{e:?}"), "getSummary read failed");
            Some((0, 0, UPTIME_DECIMALS))
        }
    }
}

/// Write an uptime attestation on-chain (`giveFeedback`). Spends gas — only
/// called when attestations are enabled. Mirrors the `/feedback` handler's
/// EIP-1559 vs legacy gas handling.
pub async fn attest_uptime(
    provider: &EvmProvider,
    reputation_registry: Address,
    target: &AttestTarget,
    uptime_bps: u16,
    feedback_uri: &str,
    feedback_hash: B256,
) -> Result<B256, String> {
    let reg = IReputationRegistry::new(reputation_registry, provider.inner().clone());
    let call = reg.giveFeedback(
        U256::from(target.agent_id),
        uptime_bps as i128,
        UPTIME_DECIMALS,
        UPTIME_TAG.to_string(),
        String::new(),
        target.url.clone(),
        feedback_uri.to_string(),
        feedback_hash,
    );
    let sent = if !provider.is_eip1559() {
        let gp = provider
            .inner()
            .get_gas_price()
            .await
            .map_err(|e| format!("gas price: {e:?}"))?;
        call.gas_price(gp).send().await
    } else {
        call.send().await
    };
    let pending = sent.map_err(|e| format!("send: {e:?}"))?;
    let receipt = pending
        .get_receipt()
        .await
        .map_err(|e| format!("receipt: {e:?}"))?;
    if !receipt.status() {
        return Err("attestation tx reverted".to_string());
    }
    Ok(receipt.transaction_hash)
}

/// Refresh the reputation cache for every target (read-only), and — when
/// enabled — write fresh uptime attestations. `uptime_of` returns
/// `(uptime_bps, probes, oks)` for a target URL, or `None` to skip.
pub async fn run_cycle<M, F>(
    map: &M,
    config: &AttestationConfig,
    targets: &[AttestTarget],
    cache: &Arc<RwLock<HashMap<String, VerificationInfo>>>,
    evidence: &Arc<RwLock<HashMap<String, Vec<u8>>>>,
    uptime_of: F,
) where
    M: ProviderMap<Value = NetworkProvider>,
    F: Fn(&str) -> Option<(u16, u64, u64)>,
{
    for t in targets {
        let Some(provider) = evm_provider(map, &t.network) else {
            continue;
        };
        let Some(contracts) = get_contracts(&t.network) else {
            continue;
        };

        // Optional write (gas) — only when enabled and we have fresh uptime.
        if config.enabled {
            if let Some((bps, probes, oks)) = uptime_of(&t.url) {
                let body = evidence_json(t, bps, probes, oks);
                let hash = evidence_hash(&body);
                let key = evidence_key(&t.url);
                let uri = format!("{}/discovery/attestation/{}", config.evidence_base, key);
                // Host the evidence body so the on-chain feedbackURI resolves and
                // the committed feedbackHash is independently checkable.
                evidence.write().await.insert(key, body);
                match attest_uptime(provider, contracts.reputation_registry, t, bps, &uri, hash)
                    .await
                {
                    Ok(tx) => {
                        info!(agent_id = t.agent_id, uptime_bps = bps, tx = %tx, "Wrote uptime attestation")
                    }
                    Err(e) => warn!(agent_id = t.agent_id, error = %e, "Attestation write failed"),
                }
            }
        }

        // Read-back (free) — populate the verification cache for the response.
        if let Some((count, value, decimals)) = read_reputation(
            provider,
            contracts.identity_registry,
            contracts.reputation_registry,
            t.agent_id,
            config.reviewer,
        )
        .await
        {
            let uptime = if decimals > 0 {
                value as f64 / 10f64.powi(decimals as i32)
            } else {
                value as f64
            };
            cache.write().await.insert(
                t.label.clone(),
                VerificationInfo {
                    protocol: "erc8004".to_string(),
                    network: t.network.to_string(),
                    agent_id: t.agent_id,
                    feedback_count: count,
                    uptime: if count > 0 { Some(uptime) } else { None },
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_key_is_deterministic_sha256_hex() {
        let k = evidence_key("https://tenjin.blog/api/read/x/y");
        assert_eq!(k.len(), 64);
        assert!(k.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(k, evidence_key("https://tenjin.blog/api/read/x/y"));
        assert_ne!(k, evidence_key("https://tenjin.blog/api/read/x/z"));
    }

    #[test]
    fn config_defaults_to_writes_disabled() {
        std::env::remove_var("ENABLE_BAZAAR_ATTESTATIONS");
        assert!(!AttestationConfig::from_env().enabled);
    }

    #[test]
    fn evidence_hash_and_json_roundtrip() {
        let t = AttestTarget {
            label: "Execution Market".to_string(),
            url: "https://mcp.execution.market/mcp".to_string(),
            network: crate::network::Network::Base,
            agent_id: 2106,
        };
        let body = evidence_json(&t, 9977, 100, 99);
        assert!(!body.is_empty());
        let h = evidence_hash(&body);
        assert_eq!(h.len(), 32);
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["agentId"], 2106);
        assert_eq!(parsed["uptime"], 99.77);
    }
}

/// Background task: every `interval_secs`, refresh the reputation cache and
/// (when enabled) write fresh uptime attestations. Uptime comes from the health
/// tracker's cumulative counters.
#[allow(clippy::too_many_arguments)]
pub fn start_attestation_task<M>(
    map: Arc<M>,
    health: Arc<crate::discovery_health::HealthTracker>,
    config: AttestationConfig,
    targets: Vec<AttestTarget>,
    cache: Arc<RwLock<HashMap<String, VerificationInfo>>>,
    evidence: Arc<RwLock<HashMap<String, Vec<u8>>>>,
) -> tokio::task::JoinHandle<()>
where
    M: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    info!(
        enabled = config.enabled,
        targets = targets.len(),
        "Starting Bazaar attestation task (on-chain writes {})",
        if config.enabled {
            "ENABLED"
        } else {
            "disabled"
        }
    );
    tokio::spawn(async move {
        let interval = Duration::from_secs(config.interval_secs.max(60));
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            // Snapshot cumulative uptime per target (async) so run_cycle's
            // closure stays synchronous.
            let mut up: HashMap<String, (u16, u64, u64)> = HashMap::new();
            for t in &targets {
                // Aggregate across every probed URL under the product's prefix.
                if let Some(u) = health.uptime_prefix(&t.url).await {
                    up.insert(t.url.clone(), u);
                }
            }
            run_cycle(&*map, &config, &targets, &cache, &evidence, |url| {
                up.get(url).copied()
            })
            .await;
            tokio::time::sleep(interval).await;
        }
    })
}
