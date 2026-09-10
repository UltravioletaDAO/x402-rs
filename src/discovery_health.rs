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

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::discovery::DiscoveryRegistry;
use crate::discovery_price::{
    normalize_declared_option, CatalogPaymentOption, DeclaredPaymentOption,
};
use crate::discovery_security::{safe_get, safe_post_json, SecurityReject};
use crate::discovery_terms::{
    ObservationContext, ObservationPhase, ObservedTerms, TermsProvenance, TermsTransport,
    TransportReading,
};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProbeClass {
    #[default]
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
    /// Unix seconds of the last successful upload. `0` means never.
    last_persist: AtomicU64,
    /// ETag of the overlay this process last read or wrote.
    ///
    /// Only a replica that does NOT own the discovery jobs uses it: with the
    /// job lease exactly one task probes, so this is how the others learn what
    /// it found -- a HEAD, and a read of the 5 MB object only when it moved.
    etag: RwLock<Option<String>>,
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
            last_persist: AtomicU64::new(0),
            etag: RwLock::new(None),
        }
    }

    /// Attach an S3 overlay and load any existing records.
    pub async fn configure_s3(&self, bucket: String, key: String) {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);
        // Load existing overlay (best-effort).
        match client.get_object().bucket(&bucket).key(&key).send().await {
            Ok(obj) => {
                // Read the version BEFORE the body: `collect()` consumes the
                // output, and an ETag taken afterwards would have to come from
                // somewhere else.
                let etag = obj.e_tag().map(str::to_string);
                if let Ok(bytes) = obj.body.collect().await {
                    let data = bytes.into_bytes();
                    match serde_json::from_slice::<HashMap<String, HealthRecord>>(&data) {
                        Ok(loaded) => {
                            let n = loaded.len();
                            *self.records.write().await = loaded;
                            *self.etag.write().await = etag;
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

    /// Drop records for URLs the catalog no longer holds.
    ///
    /// The overlay is keyed by URL and written WHOLE on every prober tick. It
    /// only ever grew: nothing removed a record when its resource left the
    /// catalog, so on 2026-09-10 it reached 9.8 MB -- uploaded every 60 seconds,
    /// for resources that no longer exist. Pruning it against the catalog is not
    /// a loss: a record whose URL we no longer list is a record nothing can read.
    ///
    /// # An empty catalog prunes nothing
    ///
    /// A task whose S3 read fails at startup does not stop: `main` falls back to
    /// an empty in-memory registry and keeps serving. Its prober would then
    /// offer an empty keep-set, and pruning against it would delete the whole
    /// overlay -- every resource's liveness history and the cumulative counts
    /// the uptime attestation is built from -- because one GET failed. Same rule
    /// as the catalog store's: a read we could not complete tells us nothing,
    /// and must never become a delete.
    ///
    /// Returns how many were dropped.
    pub async fn retain_urls(&self, keep: &std::collections::HashSet<String>) -> usize {
        if keep.is_empty() {
            return 0;
        }
        let mut records = self.records.write().await;
        let before = records.len();
        records.retain(|url, _| keep.contains(url));
        let dropped = before - records.len();
        drop(records);
        if dropped > 0 {
            self.dirty.store(true, Ordering::SeqCst);
        }
        dropped
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

    /// Minimum seconds between two uploads of the overlay.
    ///
    /// It was "every tick", i.e. every 60 seconds, because `dirty` is set by any
    /// probe and the sweep probes on every tick. On 2026-09-10 that was **5,0 MB
    /// uploaded every minute** -- 7 GB a day -- to record that some liveness
    /// counters moved. Nothing reads this object except a task that is starting.
    fn persist_interval_secs() -> u64 {
        crate::discovery_config::health_persist_secs()
    }

    /// Persist the overlay to S3 if it changed AND the debounce has elapsed.
    async fn persist(&self) {
        if !self.dirty.load(Ordering::SeqCst) {
            return;
        }
        let now = now_secs();
        let last = self.last_persist.load(Ordering::SeqCst);
        if last != 0 && now.saturating_sub(last) < Self::persist_interval_secs() {
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
        // Cleared HERE and nowhere earlier. An earlier clear consumes the flag
        // on every call that returns without writing -- no overlay configured,
        // or configuration not finished yet -- and the first real upload then
        // finds nothing to write. A failed PUT sets it back below.
        self.dirty.store(false, Ordering::SeqCst);
        match overlay
            .client
            .put_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .body(body.into())
            .send()
            .await
        {
            Ok(out) => {
                self.last_persist.store(now, Ordering::SeqCst);
                // Remember what we wrote, so that if this task later stops
                // owning the jobs its first refresh does not re-read its own
                // object.
                *self.etag.write().await = out.e_tag().map(str::to_string);
            }
            Err(e) => {
                self.dirty.store(true, Ordering::SeqCst);
                error!(error = %e, "Failed to persist health overlay");
            }
        }
    }

    /// Re-read the overlay if the object moved. Non-owners only.
    ///
    /// The counterpart of single ownership for liveness: one task probes, and
    /// this is how the other two learn what it found. Without it a non-owner
    /// would annotate every listing with the health it loaded at boot, and
    /// quarantine decisions made hours ago would be the newest it ever had.
    ///
    /// A HEAD first, and a GET only when the ETag moved. The overlay is ~5.8 MB
    /// and usually unchanged between refreshes, so the cheap question is the
    /// one worth asking every time. Best-effort throughout: a failed refresh
    /// leaves the copy this task already has, which is exactly what it had
    /// before this existed.
    pub async fn refresh_overlay(&self) {
        let guard = self.overlay.read().await;
        let Some(overlay) = guard.as_ref() else {
            return;
        };

        let head = match overlay
            .client
            .head_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .send()
            .await
        {
            Ok(head) => head,
            Err(e) => {
                debug!(error = %e, "Could not check the health overlay for changes");
                return;
            }
        };

        let latest = head.e_tag().map(str::to_string);
        // An unreadable ETag means "reload": the alternative is to skip
        // forever on a store that stops reporting one.
        if latest.is_some() && latest == *self.etag.read().await {
            return;
        }

        let obj = match overlay
            .client
            .get_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .send()
            .await
        {
            Ok(obj) => obj,
            Err(e) => {
                debug!(error = %e, "Could not re-read the health overlay");
                return;
            }
        };
        let etag = obj.e_tag().map(str::to_string);
        let Ok(bytes) = obj.body.collect().await else {
            return;
        };
        match serde_json::from_slice::<HashMap<String, HealthRecord>>(&bytes.into_bytes()) {
            Ok(loaded) => {
                let n = loaded.len();
                *self.records.write().await = loaded;
                *self.etag.write().await = etag;
                info!(
                    count = n,
                    "Reloaded the health overlay published by the job owner"
                );
            }
            // A parse failure must NOT empty the records: what is in memory is
            // still the last good view, and replacing it with nothing would
            // un-quarantine every dead endpoint in the catalog.
            Err(e) => warn!(error = %e, "Health overlay parse failed; keeping the current records"),
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
///
/// It used to carry the recipients and nothing else, which is why a resource
/// could be marked alive and go on advertising a price from months ago (F4):
/// the one component that actually reads a live challenge was throwing away
/// every field except `payTo`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveTerms {
    /// `payTo` recipients, lowercased. The union of BOTH transports: a hijack
    /// declared anywhere in the challenge is a hijack.
    pub pay_to: Vec<String>,
    /// Whether a parseable x402 challenge was found in either transport.
    pub readable: bool,
    /// The full requirements from the transport that won. Never a blend of the
    /// two: fields taken from different transports compose an offer nobody made.
    pub accepts: Vec<CatalogPaymentOption>,
    /// Which transport `accepts` came from.
    pub transport: Option<TermsTransport>,
    /// Protocol version that transport declared, when it declared one.
    pub x402_version: Option<u64>,
    /// The losing transport's reading, kept whenever the two disagreed.
    pub conflict: Option<TransportReading>,
    /// Options in the challenge we could not read, counted by cause.
    pub rejected: BTreeMap<String, usize>,
}

/// One transport's reading of a challenge, before the two are reconciled.
#[derive(Debug, Default, PartialEq, Eq)]
struct ChallengeReading {
    pay_to: Vec<String>,
    accepts: Vec<CatalogPaymentOption>,
    x402_version: Option<u64>,
    rejected: BTreeMap<String, usize>,
    /// Whether the document looked like an x402 challenge at all. A body that
    /// parses as JSON but carries no payment terms -- a free preview, an error
    /// object -- has not been read.
    found_shape: bool,
}

/// Read the payment terms a live 402 advertises, from whichever transport
/// carries them.
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
///
/// # When the two transports disagree
///
/// They are not merged. A record whose network came from a header and whose
/// amount came from a body describes an offer neither document made, and it
/// would be indistinguishable from a real one afterwards. Instead:
///
/// 1. The higher declared `x402Version` wins -- a seller serving two protocol
///    versions is telling us which one is current by numbering it.
/// 2. On a tie, or with no version declared, the header wins, because that is
///    where sellers actually put the challenge.
/// 3. The loser is preserved whole, in `conflict`, as evidence.
///
/// `pay_to` stays the union of both, deliberately: the hijack check must fire
/// on a recipient declared anywhere in the response, whichever transport the
/// terms were finally taken from.
fn pay_to_from_402(body: Option<&str>, header: Option<&str>) -> LiveTerms {
    let from_header = header
        .and_then(decode_payment_required)
        .map(|v| read_challenge(&v))
        .filter(|r| r.found_shape);
    let from_body = body
        .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
        .map(|v| read_challenge(&v))
        .filter(|r| r.found_shape);

    let mut terms = LiveTerms::default();
    for reading in [from_header.as_ref(), from_body.as_ref()]
        .into_iter()
        .flatten()
    {
        terms.readable = true;
        for p in &reading.pay_to {
            if !terms.pay_to.contains(p) {
                terms.pay_to.push(p.clone());
            }
        }
    }
    if !terms.readable {
        return terms;
    }

    let (winner, winning_transport, loser, losing_transport) = match (from_header, from_body) {
        (Some(h), Some(b)) => {
            let header_wins = match (h.x402_version, b.x402_version) {
                (Some(hv), Some(bv)) if bv > hv => false,
                _ => true,
            };
            if header_wins {
                (h, TermsTransport::Header, Some(b), TermsTransport::Body)
            } else {
                (b, TermsTransport::Body, Some(h), TermsTransport::Header)
            }
        }
        (Some(h), None) => (h, TermsTransport::Header, None, TermsTransport::Body),
        (None, Some(b)) => (b, TermsTransport::Body, None, TermsTransport::Header),
        (None, None) => unreachable!("readable implies at least one reading"),
    };

    terms.transport = Some(winning_transport);
    terms.x402_version = winner.x402_version;
    terms.accepts = winner.accepts;
    terms.rejected = winner.rejected;
    if let Some(other) = loser {
        // Only a real disagreement is worth keeping. Two transports carrying the
        // same offer is the common case and is not evidence of anything.
        if other.accepts != terms.accepts || other.x402_version != terms.x402_version {
            terms.conflict = Some(TransportReading {
                transport: losing_transport,
                x402_version: other.x402_version,
                accepts: other.accepts,
            });
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

/// Read one challenge document: its recipients, its full requirements and the
/// protocol version it declares.
///
/// `found_shape` is set only when the value actually looks like an x402
/// challenge. A body that parses as JSON but carries no payment terms -- a free
/// preview, an error object -- must NOT count as "read": that is exactly the
/// case that made the hijack check pass while seeing nothing.
///
/// Requirements go through [`normalize_declared_option`], the same single
/// normalization rule the aggregator, the crawler and `POST /discovery/register`
/// use. An observation parsed by its own private rules would be comparable with
/// nothing -- and would be free to invent the `exact` that P0 removed.
fn read_challenge(v: &serde_json::Value) -> ChallengeReading {
    let mut reading = ChallengeReading::default();
    reading.x402_version = v.get("x402Version").and_then(|x| x.as_u64());

    // `paymentRequirements` is the v1 spelling of `accepts`. Missing it made a
    // seller using it look like "no terms here" -- which is exactly the state
    // that let the hijack check pass while seeing nothing.
    for key in ["accepts", "paymentRequirements"] {
        if let Some(accepts) = v.get(key).and_then(|a| a.as_array()) {
            reading.found_shape = true;
            for a in accepts {
                if let Some(p) = a.get("payTo").and_then(|p| p.as_str()) {
                    reading.pay_to.push(p.to_ascii_lowercase());
                }
                match serde_json::from_value::<DeclaredPaymentOption>(a.clone()) {
                    Ok(declared) => match normalize_declared_option(declared) {
                        Ok(option) => reading.accepts.push(option),
                        Err(reject) => {
                            *reading
                                .rejected
                                .entry(reject.rule().to_string())
                                .or_insert(0) += 1;
                        }
                    },
                    Err(_) => {
                        *reading
                            .rejected
                            .entry("option-malformed".to_string())
                            .or_insert(0) += 1;
                    }
                }
            }
        }
    }
    if let Some(p) = v.get("payTo").and_then(|p| p.as_str()) {
        reading.found_shape = true;
        reading.pay_to.push(p.to_ascii_lowercase());
    }
    reading
}

/// Classify a single probe of `url` (GET, no payment attached).
///
/// On a 402 BOTH transports are captured -- the body and the `PAYMENT-REQUIRED`
/// header -- because the caller has to check for a payTo swap and sellers put
/// the challenge in either one. Reading only the body found nothing on 36 of 36
/// live resources measured 2026-08-20.
/// One probe's result.
///
/// A struct rather than a tuple because it grew a sixth member: the origin's own
/// `Retry-After`. A politeness instruction that arrives and is dropped on the
/// floor is worse than not asking for one, and a six-tuple is where that happens.
#[derive(Debug, Default)]
struct ProbeOutcome {
    class: ProbeClass,
    http: Option<u16>,
    latency_ms: u64,
    /// 402 response body, when there was one.
    body: Option<String>,
    /// `PAYMENT-REQUIRED` header, when there was one.
    challenge_header: Option<String>,
    /// What the origin asked us to wait, when it asked.
    retry_after: Option<Duration>,
}

async fn probe(url: &url::Url) -> ProbeOutcome {
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
            // Read before the body is consumed: `text()` takes the response.
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(crate::discovery_revalidation::parse_retry_after);
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
            ProbeOutcome {
                class,
                http: Some(code),
                latency_ms: latency,
                body,
                challenge_header: header,
                retry_after,
            }
        }
        // A URL the SSRF connector refuses (private/template/bad-port) is not a
        // dead endpoint — it is simply not probeable this way.
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => ProbeOutcome {
            class: ProbeClass::Unprobeable,
            latency_ms: latency,
            ..ProbeOutcome::default()
        },
        // Resolution failure / connection error / redirect loop -> dead.
        Err(_) => ProbeOutcome {
            class: ProbeClass::Fail,
            latency_ms: latency,
            ..ProbeOutcome::default()
        },
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

            // Probing is periodic discovery work: one task does it, and the
            // others read the overlay it publishes with
            // [`HealthTracker::refresh_overlay`]. Every replica probing the
            // same catalog is that many times the outbound TLS handshakes --
            // the CPU that took production down on 2026-09-10 -- and that many
            // writers of one whole-object PUT, where the last one to finish
            // erases what the others found. The 2.21.2 debounce made each
            // writer cheaper; this makes there be one.
            //
            // Note this gate covers the two overlay prunes and both persists
            // below as well, which is the point: a task that probes nothing has
            // nothing to prune and nothing to publish.
            if !crate::discovery_owner::owns_jobs() {
                continue;
            }

            let now = now_secs();

            // Collect due URLs from the registry (a plain snapshot of URLs, so
            // no registry guard is held across the probes below). Cap probes
            // per host per tick so a mega-host (e.g. orbisapi.com with thousands
            // of listings) is spread across ticks rather than hammered.
            let mut per_host: HashMap<String, usize> = HashMap::new();
            let mut due: Vec<(url::Url, String, Vec<String>)> = Vec::new();
            let targets = registry.probe_targets().await;
            // The catalog is bounded now, so the overlay has to be too: a health
            // record for a URL that left the catalog is written to S3 every tick
            // and read by nobody.
            let live: std::collections::HashSet<String> =
                targets.iter().map(|(u, _, _)| u.to_string()).collect();
            let pruned = tracker.retain_urls(&live).await;
            if pruned > 0 {
                info!(
                    pruned = pruned,
                    held = live.len(),
                    "dropped health records for resources no longer in the catalog"
                );
            }
            // The observed-terms overlay is a second object written whole, so it
            // needs the same hygiene against the same keep-set.
            let pruned_terms = registry.terms().retain_urls(&live).await;
            if pruned_terms > 0 {
                info!(
                    pruned = pruned_terms,
                    held = live.len(),
                    "dropped observed terms for resources no longer in the catalog"
                );
            }
            // ----------------------------------------------------------------
            // The budget, split.
            //
            // `max_per_tick` is the SAME allowance 2.21.2 settled on. Demand
            // spends from it first; the periodic sweep keeps a reserved floor
            // so a busy resource cannot starve the long tail. Nothing here adds
            // a probe: this decides which probes the tick spends.
            // ----------------------------------------------------------------
            let queue = registry.revalidation();
            let (demand_budget, _reserved) = crate::discovery_revalidation::split_budget(
                max_per_tick,
                crate::discovery_config::long_tail_share(),
            );

            let mut demanded: Vec<(url::Url, String, Vec<String>)> = Vec::new();
            if crate::discovery_config::revalidation_enabled() {
                // Fold in what the other replicas asked for, then take the top
                // of the queue. Both are owner-only: this whole block is behind
                // the ownership gate above.
                let folded = queue.absorb_shared(now).await;
                queue.evict_expired(now).await;
                let batch = queue.take_batch(demand_budget, now).await;
                if !batch.is_empty() || folded > 0 {
                    // Resolved BEFORE the macro: an `.await` inside a tracing
                    // macro's argument list holds a non-Send `format_args!`
                    // temporary across the suspension point.
                    let depth = queue.depth().await;
                    debug!(
                        folded_from_replicas = folded,
                        taken = batch.len(),
                        depth = depth,
                        demand_budget = demand_budget,
                        "revalidation batch"
                    );
                }
                let by_url: HashMap<String, (url::Url, String, Vec<String>)> = targets
                    .iter()
                    .map(|(u, ty, p)| (u.to_string(), (u.clone(), ty.clone(), p.clone())))
                    .collect();
                for (url, reason) in batch {
                    // A queued URL that has left the catalog is simply dropped:
                    // we do not probe what we no longer list.
                    if let Some(target) = by_url.get(&url) {
                        let host = target.0.host_str().unwrap_or_default().to_string();
                        *per_host.entry(host).or_insert(0) += 1;
                        debug!(url = %url, reason = reason.as_str(), "revalidating on demand");
                        demanded.push(target.clone());
                    }
                }
            }

            let already: std::collections::HashSet<String> =
                demanded.iter().map(|(u, _, _)| u.to_string()).collect();
            due.extend(demanded);

            for (u, ty, pay_to) in targets {
                if due.len() >= max_per_tick {
                    break;
                }
                if already.contains(u.as_str()) {
                    continue;
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
                let terms_overlay = registry.terms();
                let registry_for_terms = registry.clone();
                let queue_for_probe = registry.revalidation();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.ok();
                    // MCP endpoints answer a POST JSON-RPC handshake, not a GET
                    // 402 — probing them with GET would mark our own first-party
                    // MCP services dead.
                    let outcome = if resource_type == "mcp" {
                        let (c, h, l) = probe_mcp(&u).await;
                        ProbeOutcome {
                            class: c,
                            http: h,
                            latency_ms: l,
                            ..ProbeOutcome::default()
                        }
                    } else {
                        probe(&u).await
                    };
                    let ProbeOutcome {
                        mut class,
                        http,
                        latency_ms: latency,
                        body,
                        challenge_header: pr_header,
                        retry_after,
                    } = outcome;

                    // The challenge is read ONCE, and read whole. Two callers
                    // want it and they want different halves: the hijack check
                    // wants the recipients, the terms overlay wants the price.
                    // Probing twice for that would double every seller's load.
                    let live = if class == ProbeClass::Alive
                        && (body.is_some() || pr_header.is_some())
                    {
                        Some(pay_to_from_402(body.as_deref(), pr_header.as_deref()))
                    } else {
                        None
                    };

                    // payTo drift (F4): a live 402 that now pays a recipient the
                    // listing never declared is a hijack signal, not a health
                    // signal. Quarantine immediately and alarm.
                    //
                    // A changed AMOUNT is deliberately not in this branch and
                    // must never be: a seller repricing is ordinary commerce,
                    // and quarantining for it would hide a live resource over a
                    // change it is entitled to make. The price change is
                    // recorded below, as an observation.
                    if !expected_pay_to.is_empty() {
                        if let Some(live) = live.as_ref() {
                            if pay_to_drifted(&expected_pay_to, live) {
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
                    }

                    tracker.record_probe(u.as_str(), class, http, latency).await;

                    // Politeness feedback. A host that refuses us goes into
                    // backoff -- its own `Retry-After` when it sent one, an
                    // exponential schedule with jitter when it did not -- so a
                    // failing origin is asked less often rather than by every
                    // replica at the same instant.
                    match http {
                        Some(429) | Some(503) => {
                            queue_for_probe
                                .note_refusal(u.as_str(), retry_after, now_secs())
                                .await
                        }
                        Some(code) if (500..600).contains(&code) => {
                            queue_for_probe.note_refusal(u.as_str(), None, now_secs()).await
                        }
                        None => queue_for_probe.note_refusal(u.as_str(), None, now_secs()).await,
                        _ => queue_for_probe.note_success(u.as_str()).await,
                    }

                    // Record what the origin actually said, with the context it
                    // said it in. Written even when the probe quarantined the
                    // resource: the reading happened, and hiding it would lose
                    // the evidence of what it was hidden for.
                    if let Some(live) = live {
                        record_observation(
                            &registry_for_terms,
                            &terms_overlay,
                            &u,
                            &resource_type,
                            http,
                            live,
                        )
                        .await;
                    }
                }));
            }
            for h in handles {
                let _ = h.await;
            }
            tracker.persist().await;
            // Its own debounce, slower than the tick: a price observed twice in
            // five minutes is the same observation, and this object is larger.
            registry.terms().persist().await;
        }
    })
}

/// Whether a live challenge pays a recipient the listing never declared.
///
/// The AMOUNT is not an input here, and must never become one. A seller
/// repricing is ordinary commerce; a seller redirecting the money is a hijack.
/// Quarantine is the response to the second, and applying it to the first would
/// hide a live resource over a change it is entitled to make. A price change is
/// recorded as an observation instead, where a reader can see it and decide.
fn pay_to_drifted(expected: &[String], live: &LiveTerms) -> bool {
    live.pay_to.iter().any(|p| !expected.contains(p))
}

/// Store one reading of an origin's live terms in the observed-terms overlay.
///
/// The record's fingerprint is captured alongside it, so a later listing can
/// tell "observed against these exact terms" from "observed, and the listing has
/// been revised since" -- which is the difference between a fresh price and one
/// that is due for revalidation.
///
/// Nothing here can fail a probe. A response that was not a challenge at all
/// records nothing -- we learned nothing, and overwriting a real reading with an
/// empty one would erase evidence. A challenge we DID read but whose options we
/// could not parse is recorded, with the causes counted: that dates the look,
/// and the freshness assessment reports the price as unverified rather than
/// treating an empty reading as agreement.
async fn record_observation(
    registry: &DiscoveryRegistry,
    overlay: &Arc<crate::discovery_terms::TermsOverlay>,
    url: &url::Url,
    resource_type: &str,
    http_status: Option<u16>,
    live: LiveTerms,
) {
    if !live.readable {
        return;
    }
    let content_hash = registry
        .get(url.as_str())
        .await
        .map(|r| r.content_fingerprint());
    let observation = ObservedTerms {
        accepts: live.accepts,
        observed_at: now_secs(),
        context: ObservationContext::anonymous_get(resource_type),
        // A challenge is the verification phase by construction: no payment has
        // been made, so an `upto` amount here is the ceiling, not a charge.
        phase: ObservationPhase::Verification,
        provenance: TermsProvenance::OriginResponse,
        transport: live.transport.unwrap_or(TermsTransport::Header),
        x402_version: live.x402_version,
        http_status,
        content_hash,
        conflict: live.conflict,
        rejected: live.rejected,
        truncated: false,
    };
    if observation.conflict.is_some() {
        warn!(
            url = %url,
            context = %observation.context.key(),
            transport = ?observation.transport,
            "the header and the body of this 402 declare different terms; keeping both"
        );
    } else {
        debug!(
            url = %url,
            context = %observation.context.key(),
            options = observation.accepts.len(),
            rejected = ?observation.rejected,
            "recorded the payment terms this origin advertises"
        );
    }
    overlay.record(url.as_str(), observation).await;
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
    async fn the_overlay_drops_records_for_resources_that_left_the_catalog() {
        // 9.8 MB on 2026-09-10, uploaded whole every 60 seconds, and nothing
        // ever removed an entry. A record whose URL is no longer listed is a
        // record nothing can read.
        let t = HealthTracker::new();
        for url in ["https://a.example/x", "https://gone.example/x"] {
            t.record_probe(url, ProbeClass::Alive, Some(402), 5).await;
        }
        assert_eq!(t.snapshot().await.len(), 2);

        let live: std::collections::HashSet<String> =
            ["https://a.example/x".to_string()].into_iter().collect();
        assert_eq!(t.retain_urls(&live).await, 1);

        let held = t.snapshot().await;
        assert_eq!(held.len(), 1);
        assert!(held.contains_key("https://a.example/x"));
    }

    #[tokio::test]
    async fn the_overlay_is_not_uploaded_again_within_the_debounce() {
        // It was uploaded every tick, i.e. every 60 seconds: 5,0 MB a minute,
        // 7 GB a day, to record that some counters moved. Nothing reads this
        // object except a task that is starting.
        let t = HealthTracker::new();
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5)
            .await;
        assert!(t.dirty.load(Ordering::SeqCst), "a probe marks it changed");

        // No overlay is attached, so `persist` cannot upload; what it must do is
        // leave the dirty flag alone rather than consume it, or the first real
        // upload after configuration would find nothing to write.
        t.persist().await;
        assert!(
            t.dirty.load(Ordering::SeqCst),
            "an upload that did not happen must not clear the change flag"
        );

        // And a tracker that just uploaded holds off.
        t.last_persist.store(now_secs(), Ordering::SeqCst);
        assert!(
            now_secs().saturating_sub(t.last_persist.load(Ordering::SeqCst))
                < HealthTracker::persist_interval_secs(),
            "the debounce window is what suppresses the next upload"
        );
    }

    #[tokio::test]
    async fn an_empty_catalog_never_empties_the_overlay() {
        // `main` falls back to an EMPTY in-memory registry when the S3 read
        // fails at startup, and keeps serving. Without this guard one transient
        // GET failure would delete every liveness record and every cumulative
        // count the uptime attestation is built from.
        let t = HealthTracker::new();
        for url in ["https://a.example/x", "https://b.example/x"] {
            t.record_probe(url, ProbeClass::Alive, Some(402), 5).await;
        }
        let nothing: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(t.retain_urls(&nothing).await, 0);
        assert_eq!(
            t.snapshot().await.len(),
            2,
            "a catalog we could not read is not a catalog with no resources"
        );
    }

    #[tokio::test]
    async fn pruning_nothing_does_not_dirty_the_overlay() {
        // A no-op prune must not schedule a 9.8 MB upload.
        let t = HealthTracker::new();
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5)
            .await;
        t.dirty.store(false, Ordering::SeqCst);
        let live: std::collections::HashSet<String> =
            ["https://a.example/x".to_string()].into_iter().collect();
        assert_eq!(t.retain_urls(&live).await, 0);
        assert!(!t.dirty.load(Ordering::SeqCst));
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

    // ========================================================================
    // The whole challenge, not just its recipients
    // ========================================================================

    /// A full v2 challenge in the body: scheme, network, asset, amount, payTo.
    const FULL_BODY: &str = r#"{"x402Version":2,"accepts":[{
        "scheme":"exact","network":"eip155:8453",
        "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        "amount":"30000",
        "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545",
        "maxTimeoutSeconds":300}]}"#;

    #[test]
    fn the_whole_requirement_is_read_not_only_the_recipient() {
        // F4: this component is the only one that sees a live 402, and it kept
        // the recipients and threw the price away. A resource could be marked
        // alive and go on advertising an amount from months ago.
        let terms = pay_to_from_402(None, Some(REAL_HEADER));
        assert_eq!(terms.accepts.len(), 1);
        let o = &terms.accepts[0];
        assert_eq!(o.scheme.to_string(), "exact");
        assert_eq!(o.network.to_string(), "eip155:8453");
        assert_eq!(o.amount.to_string(), "100000");
        assert_eq!(o.max_timeout_seconds, 300);
        assert_eq!(terms.x402_version, Some(2));
        assert_eq!(terms.transport, Some(TermsTransport::Header));
    }

    #[test]
    fn the_v1_spelling_of_the_amount_is_read_by_the_same_rule_as_every_import() {
        // `maxAmountRequired` is the v1 name for the same number. The prober
        // goes through the one shared normalization rule, so a challenge and a
        // feed entry describing the same offer produce the same record -- and
        // the prober cannot invent the `exact` that P0 removed.
        let body = r#"{"x402Version":1,"paymentRequirements":[{
            "scheme":"upto","network":"base",
            "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "maxAmountRequired":"100000",
            "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545"}]}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert_eq!(terms.accepts.len(), 1);
        assert_eq!(terms.accepts[0].scheme.to_string(), "upto");
        assert_eq!(terms.accepts[0].amount.to_string(), "100000");
        assert_eq!(terms.x402_version, Some(1));
    }

    #[test]
    fn an_unreadable_option_is_counted_by_cause_and_never_becomes_a_price() {
        let body = r#"{"x402Version":2,"accepts":[{
            "scheme":"exact","network":"eip155:8453",
            "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount":"0.002",
            "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545"}]}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert!(terms.readable, "we read the challenge");
        assert!(terms.accepts.is_empty(), "and refused the option");
        assert_eq!(terms.rejected.get("amount-not-an-integer"), Some(&1));
    }

    #[test]
    fn two_transports_carrying_the_same_offer_are_not_a_conflict() {
        let terms = pay_to_from_402(Some(FULL_BODY), Some(header_of(FULL_BODY).as_str()));
        assert!(terms.conflict.is_none());
        assert_eq!(terms.accepts.len(), 1);
    }

    #[test]
    fn a_header_and_a_body_that_disagree_are_kept_apart_not_blended() {
        // The header says 0.10, the body says 0.03. There is no single offer
        // here, and manufacturing one -- a network from the header, an amount
        // from the body -- would be indistinguishable afterwards from an offer
        // the seller really made.
        let cheaper = FULL_BODY.replace("30000", "100000");
        let terms = pay_to_from_402(Some(FULL_BODY), Some(header_of(&cheaper).as_str()));
        assert_eq!(
            terms.transport,
            Some(TermsTransport::Header),
            "same protocol version: the header is where sellers put the challenge"
        );
        assert_eq!(terms.accepts[0].amount.to_string(), "100000");
        let conflict = terms.conflict.expect("the other reading is kept");
        assert_eq!(conflict.transport, TermsTransport::Body);
        assert_eq!(conflict.accepts[0].amount.to_string(), "30000");
    }

    #[test]
    fn a_newer_protocol_version_decides_which_transport_is_current() {
        // A seller serving two protocol versions is telling us which is current
        // by numbering it. The tie-break is the version, not the transport.
        let v1_header = header_of(
            &FULL_BODY
                .replace(r#""x402Version":2"#, r#""x402Version":1"#)
                .replace("30000", "100000"),
        );
        let terms = pay_to_from_402(Some(FULL_BODY), Some(v1_header.as_str()));
        assert_eq!(terms.transport, Some(TermsTransport::Body));
        assert_eq!(terms.x402_version, Some(2));
        assert_eq!(terms.accepts[0].amount.to_string(), "30000");
        let conflict = terms.conflict.expect("the v1 reading is kept as evidence");
        assert_eq!(conflict.x402_version, Some(1));
    }

    #[test]
    fn a_hijacked_recipient_is_seen_in_either_transport_even_when_one_wins() {
        // The terms come from one transport; the drift check sees BOTH. A
        // recipient declared anywhere in the response is a recipient declared.
        let other_payee = FULL_BODY.replace(
            "0xe4dc963c56979E0260fc146b87eE24F18220e545",
            "0x000000000000000000000000000000000000dEaD",
        );
        let terms = pay_to_from_402(Some(&other_payee), Some(header_of(FULL_BODY).as_str()));
        assert_eq!(terms.pay_to.len(), 2);
        assert!(terms
            .pay_to
            .contains(&"0x000000000000000000000000000000000000dead".to_string()));
    }

    #[test]
    fn a_repriced_offer_is_not_a_hijack() {
        // Same recipient, a very different number. This must NOT quarantine:
        // the whole distinction between an identity failure and a commercial
        // one lives in this predicate.
        let expected = vec!["0xe4dc963c56979e0260fc146b87ee24f18220e545".to_string()];
        let repriced = FULL_BODY.replace(r#""amount":"30000""#, r#""amount":"500000""#);
        let terms = pay_to_from_402(Some(&repriced), None);
        assert!(terms.readable);
        assert!(
            !pay_to_drifted(&expected, &terms),
            "a price change is not a payTo swap"
        );
        assert_eq!(
            terms.accepts[0].amount.to_string(),
            "500000",
            "and the new price is what gets recorded"
        );
    }

    #[test]
    fn a_redirected_payment_still_is_a_hijack() {
        let expected = vec!["0xe4dc963c56979e0260fc146b87ee24f18220e545".to_string()];
        let hijacked = FULL_BODY.replace(
            "0xe4dc963c56979E0260fc146b87eE24F18220e545",
            "0x000000000000000000000000000000000000dEaD",
        );
        let terms = pay_to_from_402(Some(&hijacked), None);
        assert!(pay_to_drifted(&expected, &terms));
    }

    fn header_of(json: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(json)
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
