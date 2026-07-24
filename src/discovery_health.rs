//! Bazaar health prober (WS-B) — the "pre-ping" that keeps the curated catalog
//! alive-only.
//!
//! An x402 resource is UP iff it answers HTTP 402 (a live payment challenge).
//! A background task probes registered URLs with the SSRF-hardened
//! [`crate::discovery_security::safe_get`] connector (never attaching payment),
//! classifies the response, and drives a small hysteresis state machine so that
//! dead endpoints are quarantined (hidden from the default listing) and
//! recoveries resurface automatically. Liveness lives in a **separate overlay**
//! (`bazaar/health.json`), never inline on the resource, so imports and the
//! retention GC can never clobber it.
//!
//! Probe classification:
//! - `402` -> alive (a live x402 resource).
//! - `401/403/405/415` -> auth-gated (healthy for its design; e.g. Execution
//!   Market authenticates before 402, and POST-only endpoints answer 405 to GET).
//! - `200/201/429` -> degraded (responds, no payment challenge).
//! - `404/410` / dead / 5xx / DNS-fail -> fail (counts toward quarantine).
//! - SSRF-refused / template / non-http -> unprobeable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::discovery::DiscoveryRegistry;
use crate::discovery_security::{safe_get, SecurityReject};
use crate::types_v2::{HealthState, HealthStatus};

/// Consecutive fail-class probes before a resource is quarantined.
const QUARANTINE_AFTER_FAILS: u32 = 3;
/// Consecutive alive probes to recover a quarantined resource.
const RECOVER_AFTER_OK: u32 = 2;
/// Re-probe cadence for a healthy resource (seconds).
const HEALTHY_REPROBE_SECS: u64 = 7 * 24 * 3600;
/// Backoff schedule (seconds) for quarantined resources, indexed by fail streak.
const BACKOFF_SECS: [u64; 4] = [3600, 6 * 3600, 24 * 3600, 72 * 3600];
/// Max probes issued to a single host in one tick (politeness for mega-hosts).
const MAX_PER_HOST_PER_TICK: usize = 3;
/// Probe request timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
/// User-Agent for probes.
const PROBE_UA: &str = "uvd-bazaar-health/1.0 (+https://facilitator.ultravioletadao.xyz)";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Persisted per-resource liveness record (overlay `bazaar/health.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthRecord {
    pub status: HealthStatus,
    #[serde(default)]
    pub last_checked: Option<u64>,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub consecutive_ok: u32,
    #[serde(default)]
    pub consecutive_fail: u32,
    #[serde(default)]
    pub next_probe_at: u64,
    #[serde(default)]
    pub quarantined_at: Option<u64>,
    /// Cumulative probe totals (for WS-E uptime attestation).
    #[serde(default)]
    pub total_probes: u64,
    #[serde(default)]
    pub total_ok: u64,
}

impl HealthRecord {
    fn to_state(&self) -> HealthState {
        HealthState {
            status: self.status,
            last_checked: self.last_checked,
            http_status: self.http_status,
            latency_ms: self.latency_ms,
        }
    }
}

/// Outcome class of a single probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeClass {
    Alive,
    AuthGated,
    Degraded,
    Fail,
    Unprobeable,
}

struct S3Overlay {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
}

/// In-memory health records + optional S3 overlay persistence.
pub struct HealthTracker {
    records: Arc<RwLock<HashMap<String, HealthRecord>>>,
    overlay: RwLock<Option<S3Overlay>>,
    dirty: AtomicBool,
}

impl Default for HealthTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthTracker {
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            overlay: RwLock::new(None),
            dirty: AtomicBool::new(false),
        }
    }

    /// Attach an S3 overlay and load any existing records.
    pub async fn configure_s3(&self, bucket: String, key: String) {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);
        // Load existing overlay (best-effort).
        match client.get_object().bucket(&bucket).key(&key).send().await {
            Ok(obj) => {
                if let Ok(bytes) = obj.body.collect().await {
                    let data = bytes.into_bytes();
                    match serde_json::from_slice::<HashMap<String, HealthRecord>>(&data) {
                        Ok(loaded) => {
                            let n = loaded.len();
                            *self.records.write().await = loaded;
                            info!(count = n, "Loaded health overlay from S3");
                        }
                        Err(e) => warn!(error = %e, "Health overlay parse failed; starting empty"),
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "No existing health overlay (starting empty)");
            }
        }
        *self.overlay.write().await = Some(S3Overlay {
            client,
            bucket,
            key,
        });
    }

    /// Cumulative uptime for a URL as `(uptime_bps, total_probes, total_ok)`
    /// (WS-E attestation). `None` until the URL has at least one probe.
    pub async fn uptime(&self, url: &str) -> Option<(u16, u64, u64)> {
        let records = self.records.read().await;
        let r = records.get(url)?;
        if r.total_probes == 0 {
            return None;
        }
        let bps = ((r.total_ok as u128 * 10_000) / r.total_probes as u128) as u16;
        Some((bps, r.total_probes, r.total_ok))
    }

    /// Cumulative uptime aggregated over every probed URL starting with
    /// `prefix` — a curated product usually owns many resource URLs (all of
    /// MeshRelay's channels, every Tenjin article), so its attested uptime is
    /// the aggregate rather than one representative URL.
    pub async fn uptime_prefix(&self, prefix: &str) -> Option<(u16, u64, u64)> {
        let records = self.records.read().await;
        let (mut probes, mut oks) = (0u64, 0u64);
        for (url, r) in records.iter() {
            if url.starts_with(prefix) {
                probes = probes.saturating_add(r.total_probes);
                oks = oks.saturating_add(r.total_ok);
            }
        }
        if probes == 0 {
            return None;
        }
        let bps = ((oks as u128 * 10_000) / probes as u128) as u16;
        Some((bps, probes, oks))
    }

    /// Response-facing snapshot: url -> HealthState, for annotating listings.
    pub async fn snapshot(&self) -> HashMap<String, HealthState> {
        self.records
            .read()
            .await
            .iter()
            .map(|(u, r)| (u.clone(), r.to_state()))
            .collect()
    }

    /// Persist the overlay to S3 if dirty. Debounced by the caller's cadence.
    async fn persist(&self) {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let guard = self.overlay.read().await;
        let Some(overlay) = guard.as_ref() else {
            return;
        };
        let records = self.records.read().await;
        let body = match serde_json::to_vec(&*records) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "Health overlay serialize failed");
                return;
            }
        };
        drop(records);
        if let Err(e) = overlay
            .client
            .put_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .body(body.into())
            .send()
            .await
        {
            error!(error = %e, "Failed to persist health overlay");
        }
    }

    /// Apply a probe result to the record for `url`, driving the state machine.
    async fn record_probe(&self, url: &str, class: ProbeClass, http: Option<u16>, latency: u64) {
        let now = now_secs();
        let mut records = self.records.write().await;
        let rec = records.entry(url.to_string()).or_insert(HealthRecord {
            status: HealthStatus::Unknown,
            last_checked: None,
            http_status: None,
            latency_ms: None,
            consecutive_ok: 0,
            consecutive_fail: 0,
            next_probe_at: 0,
            quarantined_at: None,
            total_probes: 0,
            total_ok: 0,
        });
        rec.last_checked = Some(now);
        rec.http_status = http;
        rec.latency_ms = Some(latency);
        if class != ProbeClass::Unprobeable {
            rec.total_probes = rec.total_probes.saturating_add(1);
            if matches!(
                class,
                ProbeClass::Alive | ProbeClass::AuthGated | ProbeClass::Degraded
            ) {
                rec.total_ok = rec.total_ok.saturating_add(1);
            }
        }

        match class {
            ProbeClass::Alive => {
                rec.consecutive_ok = rec.consecutive_ok.saturating_add(1);
                rec.consecutive_fail = 0;
                let recovering = rec.status == HealthStatus::Quarantined;
                if !recovering || rec.consecutive_ok >= RECOVER_AFTER_OK {
                    rec.status = HealthStatus::Alive;
                    rec.quarantined_at = None;
                    rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
                } else {
                    // Still quarantined but recovering — re-probe soon to confirm.
                    rec.next_probe_at = now + BACKOFF_SECS[0];
                }
            }
            ProbeClass::AuthGated => {
                rec.consecutive_fail = 0;
                rec.status = HealthStatus::AuthGated;
                rec.quarantined_at = None;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
            ProbeClass::Degraded => {
                rec.consecutive_fail = 0;
                rec.status = HealthStatus::Degraded;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
            ProbeClass::Fail => {
                rec.consecutive_ok = 0;
                rec.consecutive_fail = rec.consecutive_fail.saturating_add(1);
                if rec.consecutive_fail >= QUARANTINE_AFTER_FAILS {
                    if rec.status != HealthStatus::Quarantined {
                        rec.quarantined_at = Some(now);
                    }
                    rec.status = HealthStatus::Quarantined;
                }
                let idx =
                    (rec.consecutive_fail.saturating_sub(1) as usize).min(BACKOFF_SECS.len() - 1);
                rec.next_probe_at = now + BACKOFF_SECS[idx];
            }
            ProbeClass::Unprobeable => {
                rec.status = HealthStatus::Unprobeable;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
        }
        self.dirty.store(true, Ordering::SeqCst);
    }
}

/// Classify a single probe of `url` (GET, no payment attached).
async fn probe(url: &url::Url) -> (ProbeClass, Option<u16>, u64) {
    let start = std::time::Instant::now();
    let result = safe_get(PROBE_UA, PROBE_TIMEOUT, url).await;
    let latency = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let class = match code {
                402 => ProbeClass::Alive,
                401 | 403 | 405 | 415 => ProbeClass::AuthGated,
                200 | 201 | 429 => ProbeClass::Degraded,
                404 | 410 => ProbeClass::Fail,
                c if (500..600).contains(&c) => ProbeClass::Fail,
                _ => ProbeClass::Degraded,
            };
            (class, Some(code), latency)
        }
        // A URL the SSRF connector refuses (private/template/bad-port) is not a
        // dead endpoint — it is simply not probeable this way.
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => (ProbeClass::Unprobeable, None, latency),
        // Resolution failure / connection error / redirect loop -> dead.
        Err(_) => (ProbeClass::Fail, None, latency),
    }
}

/// Start the background health prober. Wakes every `tick_secs`, probes the due
/// URLs (bounded per tick so the initial full sweep spreads over hours), and
/// debounce-persists the overlay.
pub fn start_health_task(
    registry: DiscoveryRegistry,
    tracker: Arc<HealthTracker>,
    tick_secs: u64,
    concurrency: usize,
    max_rps: u64,
) -> tokio::task::JoinHandle<()> {
    info!(
        tick_secs = tick_secs,
        concurrency = concurrency,
        max_rps = max_rps,
        "Starting Bazaar health prober"
    );
    tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        // Bound work per tick so we respect max_rps on average and spread the
        // initial ~21k sweep over hours rather than hammering all at once.
        let max_per_tick = (max_rps.max(1) * tick_secs).max(1) as usize;
        let interval = Duration::from_secs(tick_secs.max(5));
        loop {
            tokio::time::sleep(interval).await;
            let now = now_secs();

            // Collect due URLs from the registry (a plain snapshot of URLs, so
            // no registry guard is held across the probes below). Cap probes
            // per host per tick so a mega-host (e.g. orbisapi.com with thousands
            // of listings) is spread across ticks rather than hammered.
            let mut per_host: HashMap<String, usize> = HashMap::new();
            let mut due: Vec<url::Url> = Vec::new();
            for u in registry.all_urls().await {
                if due.len() >= max_per_tick {
                    break;
                }
                if !tracker_due(&tracker, &u, now) {
                    continue;
                }
                let host = u.host_str().unwrap_or_default().to_string();
                let c = per_host.entry(host).or_insert(0);
                if *c >= MAX_PER_HOST_PER_TICK {
                    continue;
                }
                *c += 1;
                due.push(u);
            }

            if due.is_empty() {
                continue;
            }
            debug!(due = due.len(), "Health prober cycle");

            let mut handles = Vec::with_capacity(due.len());
            for u in due {
                let sem = Arc::clone(&sem);
                let tracker = Arc::clone(&tracker);
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.ok();
                    let (class, http, latency) = probe(&u).await;
                    tracker.record_probe(u.as_str(), class, http, latency).await;
                }));
            }
            for h in handles {
                let _ = h.await;
            }
            tracker.persist().await;
        }
    })
}

/// Whether `url` is due for a probe now (blocking helper is cheap: one read).
fn tracker_due(tracker: &HealthTracker, url: &url::Url, now: u64) -> bool {
    // Best-effort non-async read via try_read; if contended, treat as due.
    match tracker.records.try_read() {
        Ok(records) => records
            .get(url.as_str())
            .map(|r| r.next_probe_at <= now)
            .unwrap_or(true),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn status_of(t: &HealthTracker, url: &str) -> HealthStatus {
        t.snapshot().await.get(url).unwrap().status
    }

    #[tokio::test]
    async fn quarantine_after_three_fails_and_recovers_after_two() {
        let t = HealthTracker::new();
        let u = "https://x.example/a";
        t.record_probe(u, ProbeClass::Fail, Some(404), 10).await;
        t.record_probe(u, ProbeClass::Fail, Some(404), 10).await;
        assert_ne!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Fail, Some(404), 10).await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        // recovery needs two consecutive alives
        t.record_probe(u, ProbeClass::Alive, Some(402), 10).await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Alive, Some(402), 10).await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Alive);
    }

    #[tokio::test]
    async fn alive_and_authgated_are_immediate() {
        let t = HealthTracker::new();
        t.record_probe("https://a/x", ProbeClass::Alive, Some(402), 5)
            .await;
        assert_eq!(status_of(&t, "https://a/x").await, HealthStatus::Alive);
        t.record_probe("https://b/x", ProbeClass::AuthGated, Some(401), 5)
            .await;
        assert_eq!(status_of(&t, "https://b/x").await, HealthStatus::AuthGated);
    }
}
