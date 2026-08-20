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
use crate::discovery_security::{safe_get, safe_post_json, SecurityReject};
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
    /// The live 402 pays a recipient the listing never declared — a hijack
    /// signal. Quarantines immediately, bypassing the failure hysteresis.
    PayToDrift,
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
            ProbeClass::PayToDrift => {
                // Security event, not a liveness event: quarantine on the first
                // observation rather than after the usual failure streak.
                rec.consecutive_ok = 0;
                rec.consecutive_fail = QUARANTINE_AFTER_FAILS;
                if rec.status != HealthStatus::Quarantined {
                    rec.quarantined_at = Some(now);
                }
                rec.status = HealthStatus::Quarantined;
                rec.next_probe_at = now + BACKOFF_SECS[BACKOFF_SECS.len() - 1];
            }
        }
        self.dirty.store(true, Ordering::SeqCst);
    }
}

/// JSON-RPC `initialize` handshake used to probe MCP endpoints, which answer
/// POST-only JSON-RPC rather than a bare GET 402.
const MCP_INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"uvd-bazaar-health","version":"1.0"}}}"#;

/// Probe an MCP endpoint with a JSON-RPC `initialize`. A 2xx JSON-RPC reply (or
/// a 402 challenge) means the server is live; anything else falls back to the
/// standard classification.
async fn probe_mcp(url: &url::Url) -> (ProbeClass, Option<u16>, u64) {
    let start = std::time::Instant::now();
    let result = safe_post_json(PROBE_UA, PROBE_TIMEOUT, url, MCP_INITIALIZE.to_string()).await;
    let latency = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let class = match code {
                402 => ProbeClass::Alive,
                // A JSON-RPC handshake that the server answers is a live MCP
                // service — that is this resource type's healthy signal.
                200 | 201 => ProbeClass::Alive,
                401 | 403 | 405 | 415 => ProbeClass::AuthGated,
                429 => ProbeClass::Degraded,
                404 | 410 => ProbeClass::Fail,
                c if (500..600).contains(&c) => ProbeClass::Fail,
                _ => ProbeClass::Degraded,
            };
            (class, Some(code), latency)
        }
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => (ProbeClass::Unprobeable, None, latency),
        Err(_) => (ProbeClass::Fail, None, latency),
    }
}

/// The payment terms a live 402 advertised, and -- separately -- whether we
/// managed to read them at all.
///
/// The distinction is the whole point. "The recipients match" and "we could not
/// find any recipients" are different answers, and collapsing them is what let
/// the hijack check pass silently on every resource we probe.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveTerms {
    /// `payTo` recipients, lowercased.
    pub pay_to: Vec<String>,
    /// Whether a parseable x402 challenge was found in either transport.
    pub readable: bool,
}

/// Extract the `payTo` recipients a live 402 advertises.
///
/// x402 allows the challenge in EITHER transport and sellers pick freely:
///
/// * base64 JSON in the `PAYMENT-REQUIRED` (or `X-PAYMENT-REQUIRED`) header
/// * JSON in the response body
///
/// This read the body only, and measured against production on 2026-08-20 that
/// was the wrong half: of 40 real Bazaar resources, **36 of 36 that answered
/// 402 carried the terms in the header and none in the body**. On Tenjin the
/// body is the free preview of the article -- perfectly valid JSON with no
/// payment terms in it at all -- so the parse succeeded and returned nothing.
///
/// Reported by an external prober measuring our catalog's walls.
fn pay_to_from_402(body: Option<&str>, header: Option<&str>) -> LiveTerms {
    let mut terms = LiveTerms::default();

    // Header first: it is where real sellers put it.
    if let Some(raw) = header {
        if let Some(v) = decode_payment_required(raw) {
            collect_pay_to(&v, &mut terms);
        }
    }
    if let Some(b) = body {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(b) {
            collect_pay_to(&v, &mut terms);
        }
    }
    terms
}

/// Decode a `PAYMENT-REQUIRED` header value into the challenge it carries.
///
/// Base64 in practice; a few sellers send bare JSON, so both are accepted.
fn decode_payment_required(raw: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Pull `accepts[].payTo` (v2) and a top-level `payTo` (v1) out of a challenge.
///
/// Marks `readable` only when the value actually looks like an x402 challenge.
/// A body that parses as JSON but carries no payment terms -- a free preview,
/// an error object -- must NOT count as "read": that is exactly the case that
/// made the check pass while seeing nothing.
fn collect_pay_to(v: &serde_json::Value, terms: &mut LiveTerms) {
    let mut found_shape = false;
    // `paymentRequirements` is the v1 spelling of `accepts`. Missing it made a
    // seller using it look like "no terms here" -- which is exactly the state
    // that let the hijack check pass while seeing nothing.
    for key in ["accepts", "paymentRequirements"] {
        if let Some(accepts) = v.get(key).and_then(|a| a.as_array()) {
            found_shape = true;
            for a in accepts {
                if let Some(p) = a.get("payTo").and_then(|p| p.as_str()) {
                    terms.pay_to.push(p.to_ascii_lowercase());
                }
            }
        }
    }
    if let Some(p) = v.get("payTo").and_then(|p| p.as_str()) {
        found_shape = true;
        terms.pay_to.push(p.to_ascii_lowercase());
    }
    if found_shape {
        terms.readable = true;
    }
}

/// Classify a single probe of `url` (GET, no payment attached).
///
/// On a 402 BOTH transports are captured -- the body and the `PAYMENT-REQUIRED`
/// header -- because the caller has to check for a payTo swap and sellers put
/// the challenge in either one. Reading only the body found nothing on 36 of 36
/// live resources measured 2026-08-20.
async fn probe(url: &url::Url) -> (ProbeClass, Option<u16>, u64, Option<String>, Option<String>) {
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
            // Only a 402 carries payment terms worth diffing.
            let (body, header) = if code == 402 {
                let header = resp
                    .headers()
                    .get("payment-required")
                    .or_else(|| resp.headers().get("x-payment-required"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                (resp.text().await.ok(), header)
            } else {
                (None, None)
            };
            return (class, Some(code), latency, body, header);
        }
        // A URL the SSRF connector refuses (private/template/bad-port) is not a
        // dead endpoint — it is simply not probeable this way.
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => (ProbeClass::Unprobeable, None, latency, None, None),
        // Resolution failure / connection error / redirect loop -> dead.
        Err(_) => (ProbeClass::Fail, None, latency, None, None),
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
            let mut due: Vec<(url::Url, String, Vec<String>)> = Vec::new();
            for (u, ty, pay_to) in registry.probe_targets().await {
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
                due.push((u, ty, pay_to));
            }

            if due.is_empty() {
                continue;
            }
            debug!(due = due.len(), "Health prober cycle");

            let mut handles = Vec::with_capacity(due.len());
            for (u, resource_type, expected_pay_to) in due {
                let sem = Arc::clone(&sem);
                let tracker = Arc::clone(&tracker);
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.ok();
                    // MCP endpoints answer a POST JSON-RPC handshake, not a GET
                    // 402 — probing them with GET would mark our own first-party
                    // MCP services dead.
                    let (mut class, http, latency, body, pr_header) = if resource_type == "mcp" {
                        let (c, h, l) = probe_mcp(&u).await;
                        (c, h, l, None, None)
                    } else {
                        probe(&u).await
                    };

                    // payTo drift (F4): a live 402 that now pays a recipient the
                    // listing never declared is a hijack signal, not a health
                    // signal. Quarantine immediately and alarm.
                    if class == ProbeClass::Alive && !expected_pay_to.is_empty() {
                        let live = pay_to_from_402(body.as_deref(), pr_header.as_deref());
                        let drifted: Vec<&String> = live
                            .pay_to
                            .iter()
                            .filter(|p| !expected_pay_to.contains(p))
                            .collect();
                        if !drifted.is_empty() {
                            warn!(
                                url = %u,
                                expected = ?expected_pay_to,
                                observed = ?live.pay_to,
                                "paytoswap: live 402 pays an undeclared recipient; quarantining"
                            );
                            class = ProbeClass::PayToDrift;
                        } else if !live.readable {
                            // A check that did NOT run must not look like one
                            // that passed. This is the state that hid the bug:
                            // the terms were in the header, the body parsed as
                            // a free preview, and the swap check quietly saw
                            // nothing on every resource it examined.
                            warn!(
                                url = %u,
                                has_body = body.is_some(),
                                has_header = pr_header.is_some(),
                                "paytoswap: could not read payment terms from either transport -- \
                                 the hijack check did not run for this resource"
                            );
                        }
                    }

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

    #[test]
    fn pay_to_extraction_handles_v2_and_v1_bodies() {
        // x402 v2: accepts[]
        let v2 = r#"{"x402Version":2,"accepts":[
            {"network":"eip155:8453","payTo":"0xAAAa0000000000000000000000000000000000aa"},
            {"network":"eip155:1","payTo":"0xBBBb0000000000000000000000000000000000bb"}]}"#;
        let got = pay_to_from_402(Some(v2), None);
        assert_eq!(got.pay_to.len(), 2);
        assert!(got
            .pay_to
            .contains(&"0xaaaa0000000000000000000000000000000000aa".to_string()));
        // v1-style top-level payTo
        let v1 = r#"{"payTo":"0xCCCc0000000000000000000000000000000000cc","amount":"1"}"#;
        assert_eq!(
            pay_to_from_402(Some(v1), None).pay_to,
            vec!["0xcccc0000000000000000000000000000000000cc".to_string()]
        );
        // Garbage yields nothing AND is not marked readable, so the caller can
        // tell "no drift" from "we never got to look".
        let junk = pay_to_from_402(Some("not json"), None);
        assert!(junk.pay_to.is_empty());
        assert!(!junk.readable);
    }

    #[tokio::test]
    async fn paytodrift_quarantines_immediately() {
        let t = HealthTracker::new();
        let u = "https://hijacked.example/pay";
        // A single drift observation quarantines — no failure streak required.
        t.record_probe(u, ProbeClass::PayToDrift, Some(402), 12)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
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

#[cfg(test)]
mod payment_required_transport_tests {
    use super::*;

    /// A real `PAYMENT-REQUIRED` header, base64 of the challenge Tenjin serves:
    /// x402 v2 with `accepts[].payTo` on Base. Shortened to the fields that
    /// matter, but the shape and the encoding are what production sends.
    const REAL_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6IDIsICJlcnJvciI6ICJQYXltZW50IHJlcXVpcmVkIiwgImFjY2VwdHMiOiBbeyJzY2hlbWUiOiAiZXhhY3QiLCAibmV0d29yayI6ICJlaXAxNTU6ODQ1MyIsICJhbW91bnQiOiAiMTAwMDAwIiwgImFzc2V0IjogIjB4ODMzNTg5ZkNENmVEYjZFMDhmNGM3QzMyRDRmNzFiNTRiZEEwMjkxMyIsICJwYXlUbyI6ICIweGIwNTllQUM5MzMwREM1ZjIzRjUzNDZhODEzNDhBZjFFOTlmMzc5YmQiLCAibWF4VGltZW91dFNlY29uZHMiOiAzMDB9XX0=";

    /// What Tenjin actually puts in the 402 BODY: the free preview of the
    /// article. Valid JSON, zero payment terms.
    const REAL_BODY: &str =
        r#"{"id":"01a01a4c","slug":"china-macro-weekly-3","title":"China Macro Weekly"}"#;

    #[test]
    fn the_terms_are_read_from_the_header() {
        // The bug: this returned nothing for 36 of 36 live resources, because
        // it looked only at the body -- where the terms are not.
        let terms = pay_to_from_402(Some(REAL_BODY), Some(REAL_HEADER));
        assert!(
            terms.readable,
            "a challenge in the header must count as read"
        );
        assert_eq!(
            terms.pay_to,
            vec!["0xb059eac9330dc5f23f5346a81348af1e99f379bd".to_string()]
        );
    }

    #[test]
    fn a_body_that_is_not_a_challenge_is_not_readable() {
        // THE failure that hid everything: the body parses fine and carries no
        // payment terms, so the old code returned an empty vec -- and the
        // caller's `if !live.is_empty()` guard read that as "nothing drifted".
        let terms = pay_to_from_402(Some(REAL_BODY), None);
        assert!(terms.pay_to.is_empty());
        assert!(
            !terms.readable,
            "valid JSON without payment terms is NOT a challenge we read"
        );
    }

    #[test]
    fn the_body_transport_still_works() {
        // Both transports are legal. Supporting the header must not drop the
        // sellers who use the body.
        let body = r#"{"accepts":[{"payTo":"0xAAAA"}],"x402Version":2}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert!(terms.readable);
        assert_eq!(terms.pay_to, vec!["0xaaaa".to_string()]);
    }

    #[test]
    fn a_v1_top_level_pay_to_is_read_too() {
        let terms = pay_to_from_402(Some(r#"{"payTo":"0xBBBB"}"#), None);
        assert!(terms.readable);
        assert_eq!(terms.pay_to, vec!["0xbbbb".to_string()]);
    }

    #[test]
    fn a_hijack_in_the_header_is_now_visible() {
        // The whole point: a live 402 paying an undeclared recipient. Before
        // this, a swap hidden in the header was invisible.
        let declared = ["0x1111111111111111111111111111111111111111".to_string()];
        let terms = pay_to_from_402(Some(REAL_BODY), Some(REAL_HEADER));
        let drifted: Vec<_> = terms
            .pay_to
            .iter()
            .filter(|p| !declared.contains(p))
            .collect();
        assert!(!drifted.is_empty(), "the swap must be detectable");
    }

    #[test]
    fn a_garbage_header_does_not_masquerade_as_a_reading() {
        for junk in ["not base64!!", "", "e30=", "bnVsbA=="] {
            let terms = pay_to_from_402(None, Some(junk));
            assert!(
                !terms.readable,
                "{junk:?} must not count as a challenge we read"
            );
        }
    }
}
