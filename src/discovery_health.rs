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
//!   Market authenticates before 402).
//! - `200/201/429` -> degraded (responds, no payment challenge).
//! - `404/410` / dead / 5xx / DNS-fail -> fail (counts toward quarantine).
//! - SSRF-refused / template / non-http -> unprobeable.
//!
//! The request is the one the listing declares (see [`declared_request`]): a
//! listing whose `bazaar` extension says `POST` is probed with a POST carrying
//! its own example body, because a GET to a POST-only endpoint answers 405 or
//! 404 and says nothing about whether the service is up. A listing that
//! declares nothing is probed with GET, and gets ONE POST `{}` retry in the same
//! cycle when that GET answers 405 or 400.

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
use crate::discovery_revalidation::RefreshReason;
use crate::discovery_security::{safe_get, safe_post_json, safe_send_json, SecurityReject};
use crate::discovery_terms::{
    ObservationContext, ObservationPhase, ObservedTerms, TermsProvenance, TermsTransport,
    TransportReading,
};
use crate::types_v2::{HealthState, HealthStatus, QuarantineReason};

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
/// Longest example body the prober will send, serialized.
///
/// The body is a third party's, so it is bounded like everything else a
/// listing hands us. Past this the probe still goes out with the declared
/// method, carrying [`EMPTY_JSON_BODY`] -- the same body a listing that
/// declares no example gets. Never truncated: half a JSON document is not a
/// smaller one.
const MAX_PROBE_BODY_BYTES: usize = 8 * 1024;
/// The body of a body-method probe with no usable example.
const EMPTY_JSON_BODY: &str = "{}";
/// Most of a 402 response body the prober reads; see [`read_capped`].
const MAX_PROBE_RESPONSE_BYTES: usize = 256 * 1024;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An HTTP method the prober sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMethod {
    Get,
    Post,
    Put,
    Patch,
}

impl ProbeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeMethod::Get => "GET",
            ProbeMethod::Post => "POST",
            ProbeMethod::Put => "PUT",
            ProbeMethod::Patch => "PATCH",
        }
    }

    /// Read a method as a listing declares it (or as a health record stored
    /// it). Case and surrounding whitespace are not significant.
    ///
    /// `HEAD` and `DELETE` probe as `GET`: a HEAD carries no challenge body to
    /// read, and the prober never sends a DELETE to somebody else's endpoint.
    /// Anything else is not a declaration we can act on.
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "GET" | "HEAD" | "DELETE" => Some(ProbeMethod::Get),
            "POST" => Some(ProbeMethod::Post),
            "PUT" => Some(ProbeMethod::Put),
            "PATCH" => Some(ProbeMethod::Patch),
            _ => None,
        }
    }

    fn to_reqwest(self) -> reqwest::Method {
        match self {
            ProbeMethod::Get => reqwest::Method::GET,
            ProbeMethod::Post => reqwest::Method::POST,
            ProbeMethod::Put => reqwest::Method::PUT,
            ProbeMethod::Patch => reqwest::Method::PATCH,
        }
    }
}

/// The request a listing declares, as far as the prober honours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRequest {
    /// The listing declares a method: the probe uses exactly that one. A body
    /// method is sent `{}` first; `example` -- the listing's own example body,
    /// when it declared a usable one -- only when `{}` is refused with a 400 or
    /// a 422. Always `None` for GET.
    Declared {
        method: ProbeMethod,
        example: Option<String>,
    },
    /// No usable declaration. GET -- or the method this origin's own challenge
    /// or the fallback last named -- with ONE retry by the other method. See
    /// [`probe_listing`].
    Undeclared,
}

impl ProbeRequest {
    /// Request slots one probe of this request may spend on its host: 2 when
    /// it can send an extra request in the same cycle, else 1. The scheduler
    /// reserves them up front, so the extra request counts against
    /// [`MAX_PER_HOST_PER_TICK`] like any other and is never starved of it.
    fn slots(&self) -> usize {
        match self {
            ProbeRequest::Undeclared
            | ProbeRequest::Declared {
                example: Some(_), ..
            } => 2,
            ProbeRequest::Declared { .. } => 1,
        }
    }
}

/// One catalog resource, as the prober sees it.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub url: url::Url,
    pub resource_type: String,
    /// Recipients the listing declares, lowercased: the payTo drift baseline.
    pub pay_to: Vec<String>,
    /// The request the listing declares.
    pub request: ProbeRequest,
}

impl ProbeTarget {
    /// Request slots one probe of this target may spend on its host. The MCP
    /// handshake is always one request.
    fn slots(&self) -> usize {
        if self.resource_type == "mcp" {
            1
        } else {
            self.request.slots()
        }
    }
}

/// Choose this tick's probes: the demanded ones first, then the periodic
/// sweep. Every one is admitted against its host's request slots ([`admit`])
/// and against the tick's budget, and both are counted in REQUESTS -- a probe
/// that may send an extra one spends two -- so neither the per-host cap nor
/// `DISCOVERY_HEALTH_MAX_RPS` is exceeded by the fallback.
///
/// Returns the probes to run, and the demanded ones that did not fit, for the
/// revalidation queue to hand back next tick rather than lose.
fn plan_tick(
    tracker: &HealthTracker,
    targets: Vec<ProbeTarget>,
    demanded: Vec<(ProbeTarget, RefreshReason)>,
    max_per_tick: usize,
    now: u64,
) -> (Vec<ProbeTarget>, Vec<(String, RefreshReason)>) {
    let mut per_host: HashMap<String, usize> = HashMap::new();
    let mut spent = 0usize;
    let mut due = Vec::new();
    let mut overflow = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for (target, reason) in demanded {
        taken.insert(target.url.to_string());
        if spent + target.slots() <= max_per_tick && admit(&mut per_host, &target) {
            spent += target.slots();
            due.push(target);
        } else {
            overflow.push((target.url.to_string(), reason));
        }
    }
    for target in targets {
        if spent >= max_per_tick {
            break;
        }
        if taken.contains(target.url.as_str()) || !tracker_due(tracker, &target, now) {
            continue;
        }
        if spent + target.slots() > max_per_tick || !admit(&mut per_host, &target) {
            continue;
        }
        spent += target.slots();
        due.push(target);
    }
    (due, overflow)
}

/// Take `target`'s request slots on its host for this tick, if they fit under
/// [`MAX_PER_HOST_PER_TICK`].
///
/// A target that may send an extra request reserves two up front, so the
/// fallback counts against the per-host cap like any other request -- and,
/// reserved before the probe rather than asked for during it, is never left
/// without room by the host's other listings.
fn admit(per_host: &mut HashMap<String, usize>, target: &ProbeTarget) -> bool {
    let host = target.url.host_str().unwrap_or_default().to_string();
    let used = per_host.entry(host).or_insert(0);
    if *used + target.slots() > MAX_PER_HOST_PER_TICK {
        return false;
    }
    *used += target.slots();
    true
}

/// Read the request a listing declares from its `bazaar` extension.
///
/// The method, from the first of these that names one:
///
/// 1. `extensions.bazaar.info.input.method` -- the x402 Bazaar spelling, and
///    what the Coinbase feed carries;
/// 2. the extension's JSON Schema: `schema.properties.input.method` (the HTTP
///    shape `uvd-x402-sdk` writes) or the `const` / first `enum` value of
///    `schema.properties.input.properties.method`;
/// 3. POST, when either half declares a body (`body` or `bodyType`) without a
///    method -- the body shape `uvd-x402-sdk` writes, and MeshRelay's.
///
/// The `resource.method` an origin puts in its own 402 is never in the catalog
/// record; the prober learns it from the live challenge instead (see
/// [`probe_and_record`]).
///
/// Only the method and the example body are used. The input also declares
/// `queryParams` and `headers`, and they are ignored on purpose: the listing is
/// a third party, and nothing it says goes into the URL or the headers we send.
/// The example must be JSON and at most [`MAX_PROBE_BODY_BYTES`] serialized,
/// or there is none.
pub fn declared_request(extensions: Option<&serde_json::Value>) -> ProbeRequest {
    let bazaar = extensions.and_then(|e| e.get("bazaar"));
    let info_input = bazaar
        .and_then(|b| b.get("info"))
        .and_then(|i| i.get("input"));
    let schema_input = bazaar
        .and_then(|b| b.get("schema"))
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.get("input"));
    let method = info_input
        .and_then(|i| i.get("method"))
        .and_then(|m| m.as_str())
        .and_then(ProbeMethod::parse)
        .or_else(|| schema_input.and_then(schema_method))
        .or_else(|| declares_body(info_input, schema_input).then_some(ProbeMethod::Post));
    match method {
        None => ProbeRequest::Undeclared,
        Some(ProbeMethod::Get) => ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        },
        Some(method) => ProbeRequest::Declared {
            method,
            example: info_input.and_then(example_body),
        },
    }
}

/// The method a JSON Schema input declares: the plain `method` of the SDK's
/// HTTP shape, or the `const` / first `enum` value of `properties.method`.
fn schema_method(schema_input: &serde_json::Value) -> Option<ProbeMethod> {
    if let Some(m) = schema_input.get("method").and_then(|m| m.as_str()) {
        return ProbeMethod::parse(m);
    }
    let method = schema_input.get("properties")?.get("method")?;
    method
        .get("const")
        .and_then(|c| c.as_str())
        .or_else(|| {
            method
                .get("enum")
                .and_then(|e| e.as_array())
                .and_then(|e| e.first())
                .and_then(|m| m.as_str())
        })
        .and_then(ProbeMethod::parse)
}

/// Whether either half of the extension declares a request body.
fn declares_body(
    info_input: Option<&serde_json::Value>,
    schema_input: Option<&serde_json::Value>,
) -> bool {
    let present = |v: Option<&serde_json::Value>| v.is_some_and(|v| !v.is_null());
    let info = info_input.is_some_and(|i| present(i.get("body")) || present(i.get("bodyType")));
    let schema = schema_input
        .and_then(|s| s.get("properties"))
        .is_some_and(|p| present(p.get("body")) || present(p.get("bodyType")));
    info || schema
}

/// The request the prober sends to `url`: the one the listing declares --
/// except where only a GET may go, which gets exactly that: no body method and
/// no fallback.
///
/// * `get_only`: the owner asked for it (`probeGetOnly` in the curation
///   manifest, `config/bazaar_curation.json`).
/// * This facilitator's own origin. A listing is anybody's to write
///   (`POST /discovery/register` is open), and a body method aimed at our own
///   API -- `/register`, `/feedback`, `/settle` -- would be this service
///   calling its own write endpoints with a stranger's JSON, on a schedule.
pub fn probe_request(
    url: &url::Url,
    extensions: Option<&serde_json::Value>,
    get_only: bool,
) -> ProbeRequest {
    if get_only || is_own_origin(url) {
        return ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        };
    }
    declared_request(extensions)
}

/// Whether `url` is on this facilitator's public origin
/// ([`crate::interop::PUBLIC_URL`]), any port, any case, with or without the
/// trailing FQDN dot.
fn is_own_origin(url: &url::Url) -> bool {
    let own = url::Url::parse(crate::interop::PUBLIC_URL).ok();
    match (url.host_str(), own.as_ref().and_then(|u| u.host_str())) {
        (Some(host), Some(own)) => host.trim_end_matches('.').eq_ignore_ascii_case(own),
        _ => false,
    }
}

/// The listing's example body, serialized, when it is JSON and within bounds.
fn example_body(input: &serde_json::Value) -> Option<String> {
    // Only JSON. A form or a text body is not something this prober builds,
    // and sending one we assembled from somebody else's description would be
    // inventing a request.
    if let Some(body_type) = input.get("bodyType").filter(|t| !t.is_null()) {
        let is_json = body_type
            .as_str()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("json"));
        if !is_json {
            return None;
        }
    }
    let body = input.get("body").filter(|b| !b.is_null())?;
    let serialized = serde_json::to_string(body).ok()?;
    // `{}` is what the first probe sends anyway: not an example worth a retry.
    (serialized.len() <= MAX_PROBE_BODY_BYTES && serialized != EMPTY_JSON_BODY)
        .then_some(serialized)
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
    /// HTTP method of the last probe of an HTTP listing. Absent on a record
    /// written before the prober honoured a listing's declared method -- every
    /// one of those was a GET -- and on MCP endpoints, which are probed by
    /// their handshake whatever they declare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_method: Option<String>,
    /// Why the record is quarantined, while it is. Absent on a record written
    /// before the reason was kept; [`Self::reason`] reads those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_reason: Option<QuarantineReason>,
    /// The method this origin's own challenge names (`resource.method` of its
    /// 402), for a listing whose catalog record declares none. Where the next
    /// probe of such a listing starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_method: Option<String>,
}

impl HealthRecord {
    /// Why the record is quarantined, `None` when it is not.
    ///
    /// A record from before the reason was kept is read by its signature: a
    /// fail-class probe never carries a 402 (402 is alive, or drift), and a
    /// drift zeroes the alive streak, so a quarantined record whose last probe
    /// answered 402 with no alive streak was put there by a payTo drift. One
    /// alive probe into a recovery (a streak of 1) reads as the fail streak it
    /// is recovering from: it has already seen one clean challenge.
    fn reason(&self) -> Option<QuarantineReason> {
        if self.status != HealthStatus::Quarantined {
            return None;
        }
        let drift_signature = self.http_status == Some(402) && self.consecutive_ok == 0;
        Some(self.quarantine_reason.unwrap_or(if drift_signature {
            QuarantineReason::PayToDrift
        } else {
            QuarantineReason::FailStreak
        }))
    }

    fn to_state(&self) -> HealthState {
        // The same number the uptime attestation publishes
        // (`HealthTracker::uptime`), so the listing and the on-chain feedback
        // can never disagree about it.
        let uptime_bps = (self.total_probes > 0)
            .then(|| ((self.total_ok as u128 * 10_000) / self.total_probes as u128) as u16);
        HealthState {
            status: self.status,
            last_checked: self.last_checked,
            http_status: self.http_status,
            latency_ms: self.latency_ms,
            uptime_bps,
            probe_count: (self.total_probes > 0).then_some(self.total_probes),
            probe_method: self.probe_method.clone(),
            quarantine_reason: self.reason(),
        }
    }

    /// The method the last probe used; a record without one was a GET.
    fn last_method(&self) -> ProbeMethod {
        self.probe_method
            .as_deref()
            .and_then(ProbeMethod::parse)
            .unwrap_or(ProbeMethod::Get)
    }

    /// Held in quarantine by a payTo drift rather than by failures. No 402 lifts
    /// such a hold early, whatever the request: it keeps its two-probe recovery.
    fn held_for_drift(&self) -> bool {
        self.reason() == Some(QuarantineReason::PayToDrift)
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

    /// Where the next probe of a listing that declares nothing starts: the
    /// method its own challenge named (`resource.method`), else the one the
    /// last probe used -- which, after a fallback, is the one that answered.
    async fn recorded_method(&self, url: &str) -> Option<ProbeMethod> {
        self.records
            .read()
            .await
            .get(url)
            .and_then(|r| r.learned_method.as_deref().or(r.probe_method.as_deref()))
            .and_then(ProbeMethod::parse)
    }

    /// Apply a probe result to the record for `url`, driving the state machine.
    ///
    /// `method` is the request the probe sent, for an HTTP listing; `None` for
    /// a probe the method does not describe (the MCP handshake), which leaves
    /// the record's method alone.
    async fn record_probe(
        &self,
        url: &str,
        class: ProbeClass,
        http: Option<u16>,
        latency: u64,
        method: Option<ProbeMethod>,
    ) {
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
            probe_method: None,
            quarantine_reason: None,
            learned_method: None,
        });
        // A record from before the reason was kept gets it now, read off its
        // signature while the last status code is still the one that set it.
        rec.quarantine_reason = rec.reason();
        // Evidence about one request says nothing about another. A listing
        // that declares POST and was probed with GET collected 405s and 404s
        // from a request it never serves; when the request changes, the
        // streaks restart, and a quarantine the other request built is lifted
        // by the first live challenge this one gets. Not a drift hold: that is
        // a security hold, and it keeps its own recovery rule.
        let request_changed = method.is_some_and(|m| rec.last_method() != m);
        let drift_hold = rec.held_for_drift();
        let quarantined_by_other_request =
            request_changed && rec.status == HealthStatus::Quarantined && !drift_hold;
        if request_changed {
            rec.consecutive_ok = 0;
            rec.consecutive_fail = 0;
        }
        if let Some(m) = method {
            rec.probe_method = Some(m.as_str().to_string());
        }
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
            // A drift hold is a security hold. An answer that is not a payment
            // challenge -- a 401, a 405, a 400 to `{}`, a hop to a private
            // address -- says nothing about who gets paid, so it cannot lift
            // it: only two clean challenges in a row do.
            ProbeClass::AuthGated | ProbeClass::Degraded | ProbeClass::Unprobeable
                if drift_hold =>
            {
                rec.consecutive_ok = 0;
                rec.next_probe_at = now + BACKOFF_SECS[BACKOFF_SECS.len() - 1];
            }
            ProbeClass::Alive => {
                rec.consecutive_ok = rec.consecutive_ok.saturating_add(1);
                rec.consecutive_fail = 0;
                let recovering = rec.status == HealthStatus::Quarantined;
                if !recovering
                    || quarantined_by_other_request
                    || rec.consecutive_ok >= RECOVER_AFTER_OK
                {
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
                        rec.quarantine_reason = Some(QuarantineReason::FailStreak);
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
                rec.quarantine_reason = Some(QuarantineReason::PayToDrift);
                rec.next_probe_at = now + BACKOFF_SECS[BACKOFF_SECS.len() - 1];
            }
        }
        if rec.status != HealthStatus::Quarantined {
            rec.quarantine_reason = None;
        }
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Remember the method this origin's own challenge names, for a listing
    /// whose catalog record declares none.
    /// Forget it with `None`.
    async fn learn_method(&self, url: &str, method: Option<ProbeMethod>) {
        let learned = method.map(|m| m.as_str().to_string());
        if let Some(rec) = self.records.write().await.get_mut(url) {
            if rec.learned_method != learned {
                rec.learned_method = learned;
                self.dirty.store(true, Ordering::SeqCst);
            }
        }
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
    /// The method the winning transport's `resource` object names, when it
    /// names one (`resource.method` of an x402 v2 challenge).
    pub resource_method: Option<ProbeMethod>,
    /// The host of that `resource.url`, lowercased.
    pub resource_host: Option<String>,
}

/// One transport's reading of a challenge, before the two are reconciled.
#[derive(Debug, Default, PartialEq, Eq)]
struct ChallengeReading {
    pay_to: Vec<String>,
    accepts: Vec<CatalogPaymentOption>,
    x402_version: Option<u64>,
    rejected: BTreeMap<String, usize>,
    resource_method: Option<ProbeMethod>,
    resource_host: Option<String>,
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
    terms.resource_method = winner.resource_method;
    terms.resource_host = winner.resource_host;
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
    reading.resource_method = v
        .get("resource")
        .and_then(|r| r.get("method"))
        .and_then(|m| m.as_str())
        .and_then(ProbeMethod::parse);
    reading.resource_host = v
        .get("resource")
        .and_then(|r| r.get("url"))
        .and_then(|u| u.as_str())
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));

    // `paymentRequirements` is the v1 spelling of `accepts`. Missing it made a
    // seller using it look like "no terms here" -- which is exactly the state
    // that let the hijack check pass while seeing nothing.
    for key in ["accepts", "paymentRequirements"] {
        if let Some(accepts) = v.get(key).and_then(|a| a.as_array()) {
            reading.found_shape = true;
            for a in accepts {
                if let Some(p) = a.get("payTo").and_then(drift_recipient_value) {
                    reading.pay_to.push(p);
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
    if let Some(p) = v.get("payTo").filter(|p| p.is_string()) {
        reading.found_shape = true;
        if let Some(p) = drift_recipient_value(p) {
            reading.pay_to.push(p);
        }
    }
    reading
}

/// A live `payTo`, lowercased, for the drift check -- or `None` when it is a
/// URN.
///
/// A URN (RFC 8141) names something; it is not an account and cannot receive a
/// transfer, so it cannot redirect anybody's money. The `agent-pay` option
/// carries one (`urn:x402:agent-pay:see-quote`), the catalog drops that option
/// at import (`parse_catalog_address`), so it is never in the declared set --
/// and comparing it anyway reported its seller as hijacked: one offer like that
/// in a challenge quarantined every listing that challenge guards, at once.
///
/// That is the ONLY exclusion, on purpose. The check fails closed: a payTo we
/// cannot parse is still compared, because a client may pay a spelling we do
/// not read (`0X` + hex, a chain we do not support).
fn drift_recipient(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    (!lowered.starts_with("urn:")).then_some(lowered)
}

/// [`drift_recipient`] for the JSON value an option carries. A `payTo` that is
/// not a string -- a number, an object -- is no recipient we can name, and is
/// compared as written, so it counts as undeclared: failing closed. Only an
/// absent or `null` one is nothing at all.
fn drift_recipient_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => drift_recipient(s),
        other => Some(other.to_string().to_ascii_lowercase()),
    }
}

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

/// How a probe reaches the network.
///
/// Production is [`SafeTransport`], the SSRF-hardened connector. The seam
/// exists for the tests: that connector refuses loopback by design, so they
/// drive the same probe logic against a local server through a client that
/// builds its requests with the same [`wire_body`] and
/// [`crate::discovery_security::json_request`].
#[async_trait::async_trait]
trait ProbeTransport: Send + Sync {
    async fn send(
        &self,
        url: &url::Url,
        method: ProbeMethod,
        body: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject>;
}

/// What goes on the wire for `method`: nothing for a GET, else the method and
/// its JSON body (`{}` when there is none). Never a payment header.
fn wire_body(method: ProbeMethod, body: Option<&str>) -> Option<(reqwest::Method, String)> {
    match method {
        ProbeMethod::Get => None,
        m => Some((m.to_reqwest(), body.unwrap_or(EMPTY_JSON_BODY).to_string())),
    }
}

/// The production transport: every request through `discovery_security`, so a
/// POST gets exactly the SSRF checks, redirect handling and timeout a GET gets.
struct SafeTransport;

#[async_trait::async_trait]
impl ProbeTransport for SafeTransport {
    async fn send(
        &self,
        url: &url::Url,
        method: ProbeMethod,
        body: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject> {
        match wire_body(method, body) {
            None => safe_get(PROBE_UA, PROBE_TIMEOUT, url).await,
            Some((m, b)) => safe_send_json(PROBE_UA, PROBE_TIMEOUT, url, m, b).await,
        }
    }
}

/// A listing's probe: what it found, and with which request.
#[derive(Debug)]
struct ListingProbe {
    outcome: ProbeOutcome,
    /// The request whose answer `outcome` is.
    method: ProbeMethod,
    /// Whether that answer came from the fallback's extra request.
    fell_back: bool,
}

/// Probe one listing with the request it declares. At most ONE extra request
/// per listing per cycle, and its answer replaces the first only when it
/// [`proves`] the request: a 402 carrying a challenge we can read. Anything
/// less leaves the first answer standing.
///
/// * **Declared GET**: one GET.
/// * **Declared POST/PUT/PATCH**: that method with `{}`. Only when `{}` is
///   refused as malformed -- 400 or 422 -- and the listing declared an example,
///   the same method once more, carrying the example. A seller that does work
///   before it challenges does it with an empty body, and a stranger's example
///   travels only when the endpoint needs it to get as far as the 402.
/// * **Undeclared**: GET, or the method this origin last named or answered
///   with (`recorded`). A GET answering 405, 400 or 404 -- "not like this", or
///   the 404 an Express app gives a method it has no route for -- gets ONE
///   POST `{}`; a remembered body method refused with a 405, 400, 404 or 422
///   gets ONE GET, so a remembered method cannot get stuck. A 429, a 5xx or a
///   timeout gets nothing more: that is the origin asking to be left alone.
///
/// A 405 to the method the listing declares, or to the one it answered with
/// last time, is [`refused_expected_method`]: degraded, not auth-gated.
async fn probe_listing<T: ProbeTransport + ?Sized>(
    transport: &T,
    url: &url::Url,
    request: &ProbeRequest,
    recorded: Option<ProbeMethod>,
) -> ListingProbe {
    let (first, example, expected) = match request {
        ProbeRequest::Declared { method, example } => (*method, example.as_deref(), true),
        ProbeRequest::Undeclared => match recorded {
            Some(m) if m != ProbeMethod::Get => (m, None, true),
            _ => (ProbeMethod::Get, None, false),
        },
    };
    let mut outcome = probe_once(transport, url, first, None).await;
    if expected {
        refused_expected_method(&mut outcome);
    }
    let retry = match request {
        ProbeRequest::Declared { .. } => match example {
            Some(example) if matches!(outcome.http, Some(400) | Some(422)) => {
                Some((first, Some(example)))
            }
            _ => None,
        },
        ProbeRequest::Undeclared if first == ProbeMethod::Get => {
            matches!(outcome.http, Some(405) | Some(400) | Some(404))
                .then_some((ProbeMethod::Post, None))
        }
        // Only a refusal of the request, never a refusal of us: a 429, a 5xx or
        // a timeout is the origin asking to be left alone, not a wrong method.
        ProbeRequest::Undeclared => {
            matches!(outcome.http, Some(405) | Some(400) | Some(404) | Some(422))
                .then_some((ProbeMethod::Get, None))
        }
    };
    let Some((method, body)) = retry else {
        return ListingProbe {
            outcome,
            method: first,
            fell_back: false,
        };
    };
    let second = probe_once(transport, url, method, body).await;
    if proves(&second) {
        ListingProbe {
            outcome: second,
            method,
            fell_back: true,
        }
    } else {
        ListingProbe {
            outcome,
            method: first,
            fell_back: false,
        }
    }
}

/// Whether a probe proved its request: a 402 carrying a challenge we can read.
fn proves(outcome: &ProbeOutcome) -> bool {
    outcome.class == ProbeClass::Alive
        && pay_to_from_402(outcome.body.as_deref(), outcome.challenge_header.as_deref()).readable
}

/// Read at most `cap` bytes of a response body; `None` past that.
///
/// A stranger's 402 used to be read whole. A challenge is a few KB; the free
/// preview some sellers put beside it can be an article. Past the cap the body
/// is dropped rather than truncated -- the `PAYMENT-REQUIRED` header still
/// carries the challenge, which is where sellers put it.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Option<String> {
    if resp.content_length().is_some_and(|n| n > cap as u64) {
        return None;
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if buf.len() + chunk.len() > cap {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A 405 to the method a listing is known to take -- the one it declares, or
/// the one that answered it last time -- is the endpoint refusing the very
/// request it is listed for. It responds, so it is not dead; but it is not
/// "healthy for its design" either, which is what auth-gated means and what a
/// consumer admits as live. Degraded.
///
/// A 405 to a GET we sent on a guess stays auth-gated, as it always was: that
/// is the case the fallback exists for.
fn refused_expected_method(outcome: &mut ProbeOutcome) {
    if outcome.http == Some(405) {
        outcome.class = ProbeClass::Degraded;
    }
}

/// Classify a single request to `url` (no payment attached).
///
/// On a 402 BOTH transports are captured -- the body and the `PAYMENT-REQUIRED`
/// header -- because the caller has to check for a payTo swap and sellers put
/// the challenge in either one. Reading only the body found nothing on 36 of 36
/// live resources measured 2026-08-20.
async fn probe_once<T: ProbeTransport + ?Sized>(
    transport: &T,
    url: &url::Url,
    method: ProbeMethod,
    body: Option<&str>,
) -> ProbeOutcome {
    let start = std::time::Instant::now();
    let result = transport.send(url, method, body).await;
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
                (read_capped(resp, MAX_PROBE_RESPONSE_BYTES).await, header)
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
            let targets = registry.probe_targets().await;
            // The catalog is bounded now, so the overlay has to be too: a health
            // record for a URL that left the catalog is written to S3 every tick
            // and read by nobody.
            let live: std::collections::HashSet<String> =
                targets.iter().map(|t| t.url.to_string()).collect();
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

            let mut demanded: Vec<(ProbeTarget, RefreshReason)> = Vec::new();
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
                let by_url: HashMap<String, &ProbeTarget> =
                    targets.iter().map(|t| (t.url.to_string(), t)).collect();
                for (url, reason) in batch {
                    // A queued URL that has left the catalog is simply dropped:
                    // we do not probe what we no longer list.
                    if let Some(target) = by_url.get(&url) {
                        debug!(url = %url, reason = reason.as_str(), "revalidating on demand");
                        demanded.push(((*target).clone(), reason));
                    }
                }
            }

            let (due, overflow) = plan_tick(&tracker, targets, demanded, max_per_tick, now);
            // Demand that did not fit this tick's requests goes back to the
            // queue instead of being lost; it is taken again next tick.
            for (url, reason) in overflow {
                queue.requeue(&url, reason, now).await;
            }

            if due.is_empty() {
                continue;
            }
            debug!(due = due.len(), "Health prober cycle");

            let mut handles = Vec::with_capacity(due.len());
            for target in due {
                let sem = Arc::clone(&sem);
                let tracker = Arc::clone(&tracker);
                let registry = registry.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.ok();
                    probe_and_record(&SafeTransport, &registry, &tracker, target).await;
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

/// Probe one target and record everything the probe found: the liveness
/// verdict, the politeness feedback for its host, and the terms its challenge
/// advertised.
///
/// One function for the scheduler and for the tests, so what the tests drive
/// is the production path -- including the method handed to the record, which
/// is what turns the one-off re-probe off again.
async fn probe_and_record<T: ProbeTransport + ?Sized>(
    transport: &T,
    registry: &DiscoveryRegistry,
    tracker: &HealthTracker,
    target: ProbeTarget,
) {
    let queue = registry.revalidation();
    let ProbeTarget {
        url: u,
        resource_type,
        pay_to: expected_pay_to,
        request,
    } = target;
    // MCP endpoints answer a POST JSON-RPC handshake, not a GET
    // 402 — probing them with GET would mark our own first-party
    // MCP services dead.
    let (outcome, method, fell_back) = if resource_type == "mcp" {
        let (c, h, l) = probe_mcp(&u).await;
        let outcome = ProbeOutcome {
            class: c,
            http: h,
            latency_ms: l,
            ..ProbeOutcome::default()
        };
        (outcome, None, false)
    } else {
        let recorded = tracker.recorded_method(u.as_str()).await;
        let probed = probe_listing(transport, &u, &request, recorded).await;
        if probed.fell_back {
            debug!(
                url = %u,
                method = probed.method.as_str(),
                "the extra request answered 402"
            );
        }
        (probed.outcome, Some(probed.method), probed.fell_back)
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
    let live = if class == ProbeClass::Alive && (body.is_some() || pr_header.is_some()) {
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

    tracker
        .record_probe(u.as_str(), class, http, latency, method)
        .await;

    // A listing whose catalog record declares nothing may still say how it is
    // called in its own challenge (`resource.method`, where MeshRelay puts
    // it), and the next probe starts there. Only a POST, only from a clean
    // challenge that names this listing's own host -- a 402 reached through a
    // redirect, or one that just failed the drift check, names nothing for us
    // -- and forgotten as soon as the fallback has to answer instead.
    if request == ProbeRequest::Undeclared {
        let named = live
            .as_ref()
            .filter(|_| class == ProbeClass::Alive)
            .filter(|l| {
                l.resource_host
                    .as_deref()
                    .zip(u.host_str())
                    .is_some_and(|(theirs, ours)| theirs.eq_ignore_ascii_case(ours))
            })
            .and_then(|l| l.resource_method)
            .filter(|m| *m == ProbeMethod::Post);
        if fell_back {
            tracker.learn_method(u.as_str(), None).await;
        } else if named.is_some() {
            tracker.learn_method(u.as_str(), named).await;
        }
    }

    // Politeness feedback. A host that refuses us goes into
    // backoff -- its own `Retry-After` when it sent one, an
    // exponential schedule with jitter when it did not -- so a
    // failing origin is asked less often rather than by every
    // replica at the same instant.
    match http {
        Some(429) | Some(503) => {
            queue
                .note_refusal(u.as_str(), retry_after, now_secs())
                .await
        }
        Some(code) if (500..600).contains(&code) => {
            queue.note_refusal(u.as_str(), None, now_secs()).await
        }
        None => queue.note_refusal(u.as_str(), None, now_secs()).await,
        _ => queue.note_success(u.as_str()).await,
    }

    // Record what the origin actually said, with the context it
    // said it in. Written even when the probe quarantined the
    // resource: the reading happened, and hiding it would lose
    // the evidence of what it was hidden for.
    if let Some(live) = live {
        record_observation(
            registry,
            &registry.terms(),
            &u,
            &resource_type,
            // `None` is the MCP handshake, which is a POST.
            method.unwrap_or(ProbeMethod::Post),
            http,
            live,
        )
        .await;
    }
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
    method: ProbeMethod,
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
        // The request that drew this challenge. A price read from a POST with
        // the listing's example body is a reading of THAT request, and says so.
        context: ObservationContext::anonymous(method.as_str(), resource_type),
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

/// Whether `target` is due for a probe now (blocking helper is cheap: one read).
fn tracker_due(tracker: &HealthTracker, target: &ProbeTarget, now: u64) -> bool {
    // Best-effort non-async read via try_read; if contended, treat as due.
    match tracker.records.try_read() {
        Ok(records) => records
            .get(target.url.as_str())
            .map(|r| r.next_probe_at <= now || probed_with_another_request(r, target))
            .unwrap_or(true),
        Err(_) => false,
    }
}

/// Whether a record's verdict came from a request this listing does not call
/// for, so it is re-probed now rather than at its scheduled time.
///
/// Without this, a listing that declares POST and was quarantined by GET 404s
/// would sit hidden until its backoff ran out (up to 72 h), and one labelled
/// auth-gated by a GET 405 would wait out the 7-day healthy cadence. Once
/// re-probed, the record carries the method it was probed with and this stops
/// matching, so it costs one probe per listing, once. The re-probes still go
/// through the per-host cap, so a host with many such listings is worked
/// through over several ticks.
fn probed_with_another_request(rec: &HealthRecord, target: &ProbeTarget) -> bool {
    // The MCP handshake is what an MCP endpoint is probed with, whatever it
    // declares; there is no other request to try.
    if target.resource_type == "mcp" {
        return false;
    }
    // A payTo-drift hold keeps its schedule: it is a security hold, not a
    // liveness verdict, and a different request is no reason to look sooner.
    // Except ONCE for a hold a build without the URN exclusion set (no method
    // recorded): that build quarantined every seller whose challenge carried an
    // agent-pay quote reference, for 72 hours. Looking again does not lift
    // anything -- the hold still needs two clean challenges in a row, and a
    // real swap is seen again on this very probe.
    if rec.held_for_drift() {
        return rec.probe_method.is_none();
    }
    match &target.request {
        ProbeRequest::Declared { method, .. } => rec.last_method() != *method,
        // Probed only by a build without the fallback, and its GET answered
        // exactly what the fallback exists for.
        ProbeRequest::Undeclared => {
            rec.probe_method.is_none()
                && matches!(rec.http_status, Some(405) | Some(400) | Some(404))
        }
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
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        assert_ne!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        // recovery needs two consecutive alives
        t.record_probe(u, ProbeClass::Alive, Some(402), 10, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Alive, Some(402), 10, None)
            .await;
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
            t.record_probe(url, ProbeClass::Alive, Some(402), 5, None)
                .await;
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
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5, None)
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
            t.record_probe(url, ProbeClass::Alive, Some(402), 5, None)
                .await;
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
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5, None)
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
        t.record_probe(u, ProbeClass::PayToDrift, Some(402), 12, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
    }

    #[tokio::test]
    async fn alive_and_authgated_are_immediate() {
        let t = HealthTracker::new();
        t.record_probe("https://a/x", ProbeClass::Alive, Some(402), 5, None)
            .await;
        assert_eq!(status_of(&t, "https://a/x").await, HealthStatus::Alive);
        t.record_probe("https://b/x", ProbeClass::AuthGated, Some(401), 5, None)
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

#[cfg(test)]
mod declared_method_tests {
    //! The prober against a local seller: what it sends, and what it concludes.
    //!
    //! The SSRF connector refuses loopback by design, so these drive the same
    //! probe logic through [`Loopback`], which builds every request with the
    //! production [`wire_body`] and `json_request`. No test reaches a real host.
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    const CDP_PAGE: &str = include_str!("../tests/fixtures/bazaar/cdp-pricing-page.json");
    const REPORT_CASES: &str =
        include_str!("../tests/fixtures/bazaar/declared-method-listings.json");

    /// What the seller saw of one request.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        content_type: Option<String>,
        payment_header: bool,
        body: String,
    }

    /// How the local seller answers one path.
    #[derive(Clone)]
    struct Route {
        /// Status for a GET.
        get: u16,
        /// Status for any body method.
        other: u16,
        /// When set, a body method gets its 402 only for exactly this JSON;
        /// any other body gets `refuse`, as a validating seller would answer.
        expect_body: Option<Value>,
        refuse: u16,
        /// The PAYMENT-REQUIRED header a 402 carries; `None` is a 402 with no
        /// challenge anywhere.
        challenge: Option<String>,
        /// Bytes of body beside a challenged 402 (a free preview); 0 is `{}`.
        pad: usize,
    }

    #[derive(Clone, Default)]
    struct Seller {
        routes: Arc<HashMap<String, Route>>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Seller {
        fn seen(&self, path: &str) -> Vec<Seen> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.path == path)
                .cloned()
                .collect()
        }

        fn methods(&self, path: &str) -> Vec<String> {
            self.seen(path).into_iter().map(|s| s.method).collect()
        }
    }

    async fn answer(
        State(seller): State<Seller>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let body = String::from_utf8_lossy(&body).into_owned();
        seller.seen.lock().unwrap().push(Seen {
            method: method.to_string(),
            path: uri.path().to_string(),
            content_type: headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            payment_header: headers
                .keys()
                .any(|k| k.as_str() == "x-payment" || k.as_str() == "payment-signature"),
            body: body.clone(),
        });
        let Some(route) = seller.routes.get(uri.path()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut code = if method == Method::GET {
            route.get
        } else {
            route.other
        };
        if code == 402 && method != Method::GET {
            if let Some(expected) = &route.expect_body {
                if serde_json::from_str::<Value>(&body).ok().as_ref() != Some(expected) {
                    code = route.refuse;
                }
            }
        }
        let status = StatusCode::from_u16(code).unwrap();
        let preview = if route.pad > 0 {
            "x".repeat(route.pad)
        } else {
            "{}".to_string()
        };
        match (code, &route.challenge) {
            (402, Some(c)) => (status, [("payment-required", c.clone())], preview).into_response(),
            (402, None) => (status, "payment required").into_response(),
            _ => status.into_response(),
        }
    }

    async fn serve(routes: HashMap<String, Route>) -> (String, Seller) {
        let seller = Seller {
            routes: Arc::new(routes),
            seen: Arc::default(),
        };
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(seller.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seller)
    }

    /// The prober's transport, pointed at loopback.
    struct Loopback {
        client: reqwest::Client,
        /// When set, every request goes to this local seller, whatever host
        /// the listing names -- so a listing keeps its real URL end to end.
        serving: Option<url::Url>,
    }

    impl Loopback {
        fn new() -> Self {
            Self {
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                serving: None,
            }
        }

        fn serving(base: &str) -> Self {
            Self {
                serving: Some(url::Url::parse(base).unwrap()),
                ..Self::new()
            }
        }
    }

    #[async_trait::async_trait]
    impl ProbeTransport for Loopback {
        async fn send(
            &self,
            url: &url::Url,
            method: ProbeMethod,
            body: Option<&str>,
        ) -> Result<reqwest::Response, SecurityReject> {
            let mut to = url.clone();
            if let Some(base) = &self.serving {
                let _ = to.set_scheme("http");
                let _ = to.set_host(base.host_str());
                let _ = to.set_port(base.port());
            }
            crate::discovery_security::json_request(
                &self.client,
                &to,
                wire_body(method, body).as_ref(),
            )
            .send()
            .await
            .map_err(|e| SecurityReject::Http(e.to_string()))
        }
    }

    fn header_of(challenge: &Value) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(challenge.to_string())
    }

    fn challenge_for(accepts: &Value) -> String {
        header_of(&json!({ "x402Version": 2, "accepts": accepts }))
    }

    fn route(get: u16, other: u16, accepts: &Value) -> Route {
        Route {
            get,
            other,
            expect_body: None,
            refuse: 400,
            challenge: Some(challenge_for(accepts)),
            pad: 0,
        }
    }

    fn at(base: &str, path: &str) -> url::Url {
        url::Url::parse(&format!("{base}{path}")).unwrap()
    }

    fn captured(resource: &str) -> Value {
        let page: Value = serde_json::from_str(CDP_PAGE).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["resource"] == resource)
            .unwrap_or_else(|| panic!("{resource} is not in cdp-pricing-page.json"))
            .clone()
    }

    /// Tavily's listing as the Coinbase feed carries it: POST with an example
    /// body, and two payment options -- `exact` to an address, and `agent-pay`
    /// whose payTo is a quote reference.
    fn tavily() -> Value {
        captured("https://x402.tavily.com/search")
    }

    fn accepts() -> Value {
        tavily()["accepts"].clone()
    }

    fn post(example: Option<&str>) -> ProbeRequest {
        ProbeRequest::Declared {
            method: ProbeMethod::Post,
            example: example.map(str::to_string),
        }
    }

    fn get() -> ProbeRequest {
        ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        }
    }

    fn target(url: &str, request: ProbeRequest) -> ProbeTarget {
        ProbeTarget {
            url: url::Url::parse(url).unwrap(),
            resource_type: "http".to_string(),
            pay_to: vec![],
            request,
        }
    }

    async fn status_of(t: &HealthTracker, url: &str) -> HealthStatus {
        t.snapshot().await.get(url).unwrap().status
    }

    async fn reason_of(t: &HealthTracker, url: &str) -> Option<QuarantineReason> {
        t.snapshot().await.get(url).unwrap().quarantine_reason
    }

    // ------------------------------------------------------------------------
    // Where the method comes from
    // ------------------------------------------------------------------------

    #[test]
    fn declared_request_reads_every_declaration_in_order() {
        // The feed's own shapes: GET with query params, POST with a body, and
        // an entry that declares nothing.
        let onesource = captured("https://api.onesource.io/api/chain/erc20-balance");
        assert_eq!(
            declared_request(onesource.get("extensions")),
            get(),
            "query params are declared, and never sent: nothing a listing says goes into the URL"
        );
        let enrich = captured("https://stableenrich.dev/api/fullenrich/people-search");
        let ProbeRequest::Declared { method, example } = declared_request(enrich.get("extensions"))
        else {
            panic!("StableEnrich declares POST");
        };
        assert_eq!(method, ProbeMethod::Post);
        assert_eq!(
            serde_json::from_str::<Value>(&example.unwrap()).unwrap(),
            enrich["extensions"]["bazaar"]["info"]["input"]["body"]
        );
        let nodate = captured("https://api.example-nodate.test/quote");
        assert_eq!(
            declared_request(nodate.get("extensions")),
            ProbeRequest::Undeclared
        );
        assert_eq!(declared_request(None), ProbeRequest::Undeclared);
        assert_eq!(
            declared_request(Some(&json!({ "other": {} }))),
            ProbeRequest::Undeclared
        );

        // 1. info.input.method, case and spaces aside.
        let info = |m: Value| json!({ "bazaar": { "info": { "input": { "method": m } } } });
        for (raw, want) in [
            (json!(" post "), Some(ProbeMethod::Post)),
            (json!("put"), Some(ProbeMethod::Put)),
            (json!("PATCH"), Some(ProbeMethod::Patch)),
            (json!("get"), Some(ProbeMethod::Get)),
            (json!("HEAD"), Some(ProbeMethod::Get)),
            (json!("DELETE"), Some(ProbeMethod::Get)),
            (json!("FETCH"), None),
            (json!(""), None),
            (json!(42), None),
            (Value::Null, None),
        ] {
            let got = declared_request(Some(&info(raw.clone())));
            match want {
                None => assert_eq!(got, ProbeRequest::Undeclared, "{raw}"),
                Some(ProbeMethod::Get) => assert_eq!(got, get(), "{raw}"),
                Some(m) => assert_eq!(
                    got,
                    ProbeRequest::Declared {
                        method: m,
                        example: None
                    },
                    "{raw}: a body method with no example"
                ),
            }
        }

        // 2. The JSON Schema half: the SDK's HTTP shape, a const, an enum.
        let schema =
            |input: Value| json!({ "bazaar": { "schema": { "properties": { "input": input } } } });
        assert_eq!(
            declared_request(Some(&schema(json!({ "type": "http", "method": "GET" })))),
            get()
        );
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "method": { "const": "PUT" } } })
            ))),
            ProbeRequest::Declared {
                method: ProbeMethod::Put,
                example: None
            }
        );
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "method": { "enum": ["POST", "GET"] } } })
            ))),
            post(None)
        );

        // 3. A body declared without a method is a POST: the SDK's body shape
        // (MeshRelay's), a bare bodyType, a bare example body.
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "body": { "type": "object" } } })
            ))),
            post(None)
        );
        assert_eq!(
            declared_request(Some(
                &json!({ "bazaar": { "info": { "input": { "bodyType": "json" } } } })
            )),
            post(None)
        );
        assert_eq!(
            declared_request(Some(
                &json!({ "bazaar": { "info": { "input": { "body": { "q": 1 } } } } })
            )),
            post(Some(r#"{"q":1}"#))
        );

        // The order: info.input.method wins over the schema.
        assert_eq!(
            declared_request(Some(&json!({ "bazaar": {
                "info": { "input": { "method": "GET" } },
                "schema": { "properties": { "input": { "properties": { "body": {} } } } }
            } }))),
            get()
        );
    }

    #[test]
    fn our_own_origin_and_opted_out_prefixes_only_ever_get_a_get() {
        let ext = json!({ "bazaar": { "info": { "input": {
            "method": "POST", "body": { "agentUri": "https://x.example", "recipient": "0x1" }
        } } } });
        for own in [
            "https://facilitator.ultravioletadao.xyz/register",
            "https://FACILITATOR.ultravioletadao.xyz./feedback",
            "https://facilitator.ultravioletadao.xyz:8443/settle",
        ] {
            let url = url::Url::parse(own).unwrap();
            assert_eq!(probe_request(&url, Some(&ext), false), get(), "{own}");
        }
        let other = url::Url::parse("https://facilitator.ultravioletadao.xyz.evil.com/x").unwrap();
        assert!(matches!(
            probe_request(&other, Some(&ext), false),
            ProbeRequest::Declared {
                method: ProbeMethod::Post,
                ..
            }
        ));
        // The owner's opt-out, read from the curation manifest.
        assert_eq!(probe_request(&other, Some(&ext), true), get());
        assert_eq!(
            probe_request(&other, None, true),
            get(),
            "and no fallback either"
        );
    }

    // ------------------------------------------------------------------------
    // What goes on the wire
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_post_only_listing_is_probed_with_post_and_found_alive() {
        let listing = tavily();
        let request = declared_request(listing.get("extensions"));
        let (base, seller) = serve(HashMap::from([(
            "/search".to_string(),
            route(405, 402, &listing["accepts"]),
        )]))
        .await;
        let url = at(&base, "/search");

        // What the prober did until now: a GET, which a POST-only endpoint
        // refuses -- and that refusal was filed as auth-gated.
        let get = probe_once(&Loopback::new(), &url, ProbeMethod::Get, None).await;
        assert_eq!(get.http, Some(405));
        assert_eq!(get.class, ProbeClass::AuthGated);

        let probed = probe_listing(&Loopback::new(), &url, &request, None).await;
        assert_eq!(probed.outcome.class, ProbeClass::Alive);
        assert_eq!(probed.outcome.http, Some(402));
        assert_eq!(probed.method, ProbeMethod::Post);
        assert!(!probed.fell_back);

        let posts: Vec<Seen> = seller
            .seen("/search")
            .into_iter()
            .filter(|s| s.method == "POST")
            .collect();
        assert_eq!(posts.len(), 1, "one probe: `{{}}` was enough");
        assert_eq!(posts[0].body, EMPTY_JSON_BODY, "the example stays home");
        assert_eq!(posts[0].content_type.as_deref(), Some("application/json"));
        assert!(!posts[0].payment_header, "unpaid: no payment header");
    }

    #[tokio::test]
    async fn the_example_body_travels_only_when_empty_is_refused() {
        let listing = tavily();
        let example = listing["extensions"]["bazaar"]["info"]["input"]["body"].clone();
        let request = declared_request(listing.get("extensions"));
        let mut validating = route(405, 402, &accepts());
        validating.expect_body = Some(example.clone());
        let mut strict = validating.clone();
        strict.refuse = 422;
        let (base, seller) = serve(HashMap::from([
            ("/validating".to_string(), validating),
            ("/strict".to_string(), strict),
        ]))
        .await;

        for path in ["/validating", "/strict"] {
            let probed = probe_listing(&Loopback::new(), &at(&base, path), &request, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive, "{path}");
            assert!(probed.fell_back, "{path}");
            let seen = seller.seen(path);
            assert_eq!(seen.len(), 2, "{path}: one extra request, no more");
            assert_eq!(seen[0].body, EMPTY_JSON_BODY, "{path}");
            assert_eq!(
                serde_json::from_str::<Value>(&seen[1].body).unwrap(),
                example,
                "{path}: the listing's own example, second"
            );
        }

        // No example declared: the 400 stands, and nothing else is sent.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/validating"),
            &post(None),
            None,
        )
        .await;
        assert_eq!(probed.outcome.http, Some(400));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
        assert_eq!(seller.seen("/validating").len(), 3);
    }

    #[tokio::test]
    async fn a_declared_put_or_patch_is_sent_as_declared() {
        let (base, seller) = serve(HashMap::from([(
            "/item".to_string(),
            route(405, 402, &accepts()),
        )]))
        .await;
        let url = at(&base, "/item");
        for method in [ProbeMethod::Put, ProbeMethod::Patch] {
            let request = ProbeRequest::Declared {
                method,
                example: Some(r#"{"id":1}"#.to_string()),
            };
            let probed = probe_listing(&Loopback::new(), &url, &request, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive);
            let last = seller.seen("/item").pop().unwrap();
            assert_eq!(last.method, method.as_str());
            assert_eq!(last.body, EMPTY_JSON_BODY);
        }
    }

    #[tokio::test]
    async fn a_405_to_the_declared_method_is_degraded_not_auth_gated() {
        let (base, seller) = serve(HashMap::from([(
            "/moved".to_string(),
            route(405, 405, &accepts()),
        )]))
        .await;
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/moved"),
            &post(Some(r#"{"q":1}"#)),
            None,
        )
        .await;
        assert_eq!(probed.outcome.http, Some(405));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
        assert_eq!(
            seller.methods("/moved"),
            ["POST"],
            "a refusal of the method is not a refusal of the body: the example stays home"
        );
    }

    #[tokio::test]
    async fn a_declared_post_that_answers_404_fails_as_always() {
        let (base, _seller) = serve(HashMap::from([(
            "/gone".to_string(),
            route(404, 404, &accepts()),
        )]))
        .await;
        let url = at(&base, "/gone");
        let probed = probe_listing(&Loopback::new(), &url, &post(None), None).await;
        assert_eq!(probed.outcome.class, ProbeClass::Fail);
        assert_eq!(probed.outcome.http, Some(404));

        let t = HealthTracker::new();
        for n in 1..=3 {
            t.record_probe(
                url.as_str(),
                ProbeClass::Fail,
                Some(404),
                1,
                Some(probed.method),
            )
            .await;
            let quarantined = status_of(&t, url.as_str()).await == HealthStatus::Quarantined;
            assert_eq!(quarantined, n == 3, "fail #{n}");
        }
        assert_eq!(
            reason_of(&t, url.as_str()).await,
            Some(QuarantineReason::FailStreak)
        );
    }

    #[tokio::test]
    async fn an_oversized_or_non_json_example_is_never_sent() {
        let ext = |body: Value, body_type: Value| {
            json!({ "bazaar": { "info": { "input": {
                "method": "POST", "bodyType": body_type, "body": body
            } } } })
        };
        let example_of = |e: &Value| match declared_request(Some(e)) {
            ProbeRequest::Declared { example, .. } => example,
            other => panic!("POST is declared: {other:?}"),
        };
        // `{"q":"…"}` serializes to 8 bytes plus the string.
        let at_cap = json!({ "q": "x".repeat(MAX_PROBE_BODY_BYTES - 8) });
        assert_eq!(
            serde_json::to_string(&at_cap).unwrap().len(),
            MAX_PROBE_BODY_BYTES
        );
        assert_eq!(
            example_of(&ext(at_cap.clone(), json!("json"))),
            Some(serde_json::to_string(&at_cap).unwrap()),
            "exactly at the cap is kept"
        );
        let giant = json!({ "q": "x".repeat(MAX_PROBE_BODY_BYTES - 7) });
        assert_eq!(example_of(&ext(giant.clone(), json!("json"))), None);
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), json!("form-data"))),
            None
        );
        assert_eq!(example_of(&ext(json!("a=1"), json!("text"))), None);
        assert_eq!(example_of(&ext(Value::Null, json!("json"))), None);
        assert_eq!(
            example_of(&ext(json!({}), json!("json"))),
            None,
            "`{{}}` is no example"
        );
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), json!(" JSON "))),
            Some(r#"{"a":1}"#.to_string()),
            "bodyType is not case-sensitive"
        );
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), Value::Null)),
            Some(r#"{"a":1}"#.to_string()),
            "no bodyType: the example is JSON already"
        );

        // On the wire: a seller that refuses `{}` never sees the giant either.
        let mut validating = route(405, 402, &accepts());
        validating.expect_body = Some(giant.clone());
        let (base, seller) = serve(HashMap::from([("/echo".to_string(), validating)])).await;
        let request = declared_request(Some(&ext(giant, json!("json"))));
        let probed = probe_listing(&Loopback::new(), &at(&base, "/echo"), &request, None).await;
        assert_eq!(probed.outcome.http, Some(400));
        let seen = seller.seen("/echo");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, EMPTY_JSON_BODY);
    }

    #[tokio::test]
    async fn a_body_probe_goes_through_the_same_connector_checks_as_a_get() {
        use crate::discovery_security::{json_request, safe_send_json};
        let loopback = url::Url::parse("http://127.0.0.1:8080/x").unwrap();
        assert!(matches!(
            safe_send_json(
                PROBE_UA,
                PROBE_TIMEOUT,
                &loopback,
                reqwest::Method::POST,
                "{}".into()
            )
            .await,
            Err(SecurityReject::DisallowedAddress(_))
        ));
        let bad_port = url::Url::parse("http://93.184.216.34:6379/x").unwrap();
        assert!(matches!(
            safe_send_json(
                PROBE_UA,
                PROBE_TIMEOUT,
                &bad_port,
                reqwest::Method::PUT,
                "{}".into()
            )
            .await,
            Err(SecurityReject::Port(6379))
        ));

        // The shape each hop sends, built without sending it.
        let client = reqwest::Client::new();
        let url = url::Url::parse("https://seller.example/x").unwrap();
        let body = wire_body(ProbeMethod::Patch, Some(r#"{"a":1}"#));
        let req = json_request(&client, &url, body.as_ref()).build().unwrap();
        assert_eq!(req.method(), reqwest::Method::PATCH);
        assert_eq!(
            req.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(req.body().unwrap().as_bytes(), Some(&br#"{"a":1}"#[..]));
        assert!(!req.headers().keys().any(|k| k.as_str().contains("payment")));
        let req = json_request(&client, &url, wire_body(ProbeMethod::Get, None).as_ref())
            .build()
            .unwrap();
        assert_eq!(req.method(), reqwest::Method::GET);
        assert!(req.body().is_none());
        assert!(req.headers().get(reqwest::header::CONTENT_TYPE).is_none());
    }

    #[tokio::test]
    async fn an_oversized_402_body_is_dropped_and_the_header_still_read() {
        let mut big = route(402, 402, &accepts());
        big.pad = MAX_PROBE_RESPONSE_BYTES + 1;
        let mut small = route(402, 402, &accepts());
        small.pad = 1024;
        let (base, _seller) = serve(HashMap::from([
            ("/big".to_string(), big),
            ("/small".to_string(), small),
        ]))
        .await;
        let out = probe_once(&Loopback::new(), &at(&base, "/big"), ProbeMethod::Get, None).await;
        assert_eq!(out.class, ProbeClass::Alive);
        assert!(out.body.is_none(), "never read whole");
        assert!(
            pay_to_from_402(out.body.as_deref(), out.challenge_header.as_deref()).readable,
            "the challenge is in the header"
        );
        let out = probe_once(
            &Loopback::new(),
            &at(&base, "/small"),
            ProbeMethod::Get,
            None,
        )
        .await;
        assert_eq!(out.body.map(|b| b.len()), Some(1024));
    }

    // ------------------------------------------------------------------------
    // The fallback
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_listing_with_no_method_is_found_by_one_post_after_a_405_400_or_404() {
        let mut routes = HashMap::new();
        for code in [405u16, 400, 404] {
            routes.insert(format!("/r{code}"), route(code, 402, &accepts()));
        }
        let (base, seller) = serve(routes).await;
        for code in [405u16, 400, 404] {
            let path = format!("/r{code}");
            let url = at(&base, &path);
            let probed =
                probe_listing(&Loopback::new(), &url, &ProbeRequest::Undeclared, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive, "{code}");
            assert_eq!(probed.method, ProbeMethod::Post, "{code}");
            assert!(probed.fell_back, "{code}");
            let seen = seller.seen(&path);
            assert_eq!(seller.methods(&path), ["GET", "POST"], "{code}");
            assert_eq!(seen[1].body, EMPTY_JSON_BODY);
            assert_eq!(seen[1].content_type.as_deref(), Some("application/json"));
            assert!(!seen[1].payment_header);
        }

        // The method that worked is remembered, and the next cycle starts there.
        let url = at(&base, "/r404");
        let t = HealthTracker::new();
        t.record_probe(
            url.as_str(),
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        let recorded = t.recorded_method(url.as_str()).await;
        assert_eq!(recorded, Some(ProbeMethod::Post));
        assert_eq!(
            t.snapshot().await[url.as_str()].probe_method.as_deref(),
            Some("POST"),
            "and the listing says which request answered"
        );
        let again =
            probe_listing(&Loopback::new(), &url, &ProbeRequest::Undeclared, recorded).await;
        assert_eq!(again.outcome.class, ProbeClass::Alive);
        assert!(!again.fell_back);
        assert_eq!(seller.methods("/r404"), ["GET", "POST", "POST"]);
    }

    #[tokio::test]
    async fn the_fallback_is_one_request_and_has_to_be_proven_by_a_readable_challenge() {
        let (base, seller) = serve(HashMap::from([
            ("/both-405".to_string(), route(405, 405, &accepts())),
            (
                "/unreadable".to_string(),
                Route {
                    challenge: None,
                    ..route(405, 402, &accepts())
                },
            ),
            ("/teapot".to_string(), route(418, 402, &accepts())),
        ]))
        .await;

        for path in ["/both-405", "/unreadable"] {
            let probed = probe_listing(
                &Loopback::new(),
                &at(&base, path),
                &ProbeRequest::Undeclared,
                None,
            )
            .await;
            assert_eq!(
                seller.seen(path).len(),
                2,
                "{path}: never more than one extra probe"
            );
            assert_eq!(
                probed.outcome.class,
                ProbeClass::AuthGated,
                "{path}: the GET stands"
            );
            assert_eq!(probed.outcome.http, Some(405), "{path}");
            assert_eq!(probed.method, ProbeMethod::Get, "{path}");
            assert!(!probed.fell_back, "{path}");
        }
        // Any other refusal is not what the fallback is for.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/teapot"),
            &ProbeRequest::Undeclared,
            None,
        )
        .await;
        assert_eq!(seller.methods("/teapot"), ["GET"]);
        assert_eq!(probed.outcome.http, Some(418));
    }

    #[tokio::test]
    async fn a_remembered_post_that_stops_answering_is_retried_with_get() {
        let (base, seller) = serve(HashMap::from([
            ("/now-get".to_string(), route(402, 404, &accepts())),
            ("/gone".to_string(), route(404, 405, &accepts())),
            ("/busy".to_string(), route(402, 503, &accepts())),
        ]))
        .await;
        // An origin asking to be left alone is not a wrong method: no retry.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/busy"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/busy"), ["POST"]);
        assert_eq!(probed.outcome.http, Some(503));
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/now-get"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/now-get"), ["POST", "GET"]);
        assert_eq!(probed.outcome.class, ProbeClass::Alive);
        assert_eq!(probed.method, ProbeMethod::Get);

        // Both refuse: the remembered method's 405 is the answer, and it is
        // not "healthy for its design".
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/gone"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/gone"), ["POST", "GET"]);
        assert_eq!(probed.method, ProbeMethod::Post);
        assert_eq!(probed.outcome.http, Some(405));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
    }

    #[test]
    fn a_tick_is_planned_in_requests_demand_included() {
        let t = HealthTracker::new();
        let undeclared = |u: &str| target(u, ProbeRequest::Undeclared);
        // The sweep: a host with three listings that may each send two
        // requests, and one that sends one.
        let targets = vec![
            undeclared("https://busy.example/a"),
            undeclared("https://busy.example/b"),
            target("https://busy.example/c", get()),
            target("https://quiet.example/x", get()),
        ];
        let (due, overflow) = plan_tick(&t, targets.clone(), Vec::new(), 120, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://busy.example/a",
                "https://busy.example/c",
                "https://quiet.example/x"
            ]
        );
        assert!(overflow.is_empty());

        // The global budget is in requests too: 3 requests, not 3 listings --
        // a second two-slot probe does not fit even on a fresh host.
        let spread = vec![
            undeclared("https://busy.example/a"),
            undeclared("https://other.example/y"),
            target("https://other.example/z", get()),
        ];
        let (due, _) = plan_tick(&t, spread, Vec::new(), 3, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://busy.example/a", "https://other.example/z"]);

        // Demand goes first and is held to the same cap; what does not fit is
        // handed back, and is not probed by the sweep in the same tick.
        let demanded = vec![
            (
                undeclared("https://busy.example/a"),
                RefreshReason::PurchaseIntent,
            ),
            (
                undeclared("https://busy.example/b"),
                RefreshReason::ListingStale,
            ),
        ];
        let (due, overflow) = plan_tick(&t, targets, demanded, 120, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://busy.example/a",
                "https://busy.example/c",
                "https://quiet.example/x"
            ]
        );
        assert_eq!(
            overflow,
            [(
                "https://busy.example/b".to_string(),
                RefreshReason::ListingStale
            )]
        );
    }

    #[tokio::test]
    async fn demand_that_did_not_fit_goes_back_to_the_queue() {
        let q = crate::discovery_revalidation::RevalidationQueue::new();
        let url = "https://busy.example/b";
        let now = now_secs();
        assert!(q.request(url, RefreshReason::ListingStale, now).await);
        assert_eq!(q.take_batch(10, now).await.len(), 1);
        assert!(
            !q.request(url, RefreshReason::ListingStale, now).await,
            "inside its window a request is refused"
        );
        q.requeue(url, RefreshReason::ListingStale, now).await;
        let again = q.take_batch(10, now).await;
        assert_eq!(again, [(url.to_string(), RefreshReason::ListingStale)]);
    }

    #[test]
    fn the_extra_request_counts_against_the_per_host_cap() {
        let mut per_host = HashMap::new();
        let a = target("https://busy.example/a", ProbeRequest::Undeclared);
        let b = target("https://busy.example/b", ProbeRequest::Undeclared);
        let c = target("https://busy.example/c", get());
        let d = target("https://busy.example/d", post(Some(r#"{"q":1}"#)));
        assert!(
            admit(&mut per_host, &a),
            "two slots: the GET and its fallback"
        );
        assert!(!admit(&mut per_host, &b), "2 + 2 is over the cap of 3");
        assert!(
            !admit(&mut per_host, &d),
            "an example retry reserves two as well"
        );
        assert!(
            admit(&mut per_host, &c),
            "a GET-only probe fits the last slot"
        );
        assert_eq!(per_host["busy.example"], MAX_PER_HOST_PER_TICK);
        assert!(admit(
            &mut per_host,
            &target("https://quiet.example/x", post(None))
        ));
        let mut mcp = target("https://mcp.example/mcp", ProbeRequest::Undeclared);
        mcp.resource_type = "mcp".to_string();
        assert_eq!(mcp.slots(), 1, "the MCP handshake is one request");
    }

    #[tokio::test]
    async fn an_origin_that_names_its_method_in_its_challenge_is_asked_that_way_next() {
        let accepts = accepts();
        let naming = |url: &str, method: &str| {
            header_of(&json!({
                "x402Version": 2,
                "resource": { "url": url, "method": method },
                "accepts": accepts
            }))
        };
        let path = |p: &str, get: u16, post: u16, challenge: String| {
            (
                p.to_string(),
                Route {
                    challenge: Some(challenge),
                    ..route(get, post, &accepts)
                },
            )
        };
        let mut hijacked = accepts.clone();
        hijacked[0]["payTo"] = json!("0x000000000000000000000000000000000000dEaD");
        let drifted = header_of(&json!({
            "x402Version": 2,
            "resource": { "url": "https://seller.example/drifted", "method": "POST" },
            "accepts": hijacked
        }));
        let (base, seller) = serve(HashMap::from([
            path(
                "/query",
                402,
                402,
                naming("https://seller.example/query", "POST"),
            ),
            path(
                "/elsewhere",
                402,
                402,
                naming("https://other.example/q", "POST"),
            ),
            path(
                "/put",
                402,
                402,
                naming("https://seller.example/put", "PUT"),
            ),
            path(
                "/forget",
                402,
                404,
                naming("https://seller.example/forget", "POST"),
            ),
            path("/drifted", 402, 402, drifted),
        ]))
        .await;
        let registry = DiscoveryRegistry::new();
        let t = HealthTracker::new();
        let lo = Loopback::serving(&base);
        let undeclared = |p: &str| ProbeTarget {
            pay_to: vec!["0xfe2d09ca270818e9736207ee27f0fa464a67ac66".to_string()],
            ..target(
                &format!("https://seller.example{p}"),
                ProbeRequest::Undeclared,
            )
        };
        let learned = |t: &HealthTracker, p: &str| {
            let url = format!("https://seller.example{p}");
            let records = t.records.try_read().unwrap();
            records[url.as_str()].learned_method.clone()
        };

        for p in ["/query", "/elsewhere", "/put", "/forget", "/drifted"] {
            probe_and_record(&lo, &registry, &t, undeclared(p)).await;
            assert_eq!(seller.methods(p), ["GET"], "{p}");
        }
        assert_eq!(learned(&t, "/query").as_deref(), Some("POST"));
        assert_eq!(
            learned(&t, "/elsewhere"),
            None,
            "another host names nothing for us"
        );
        assert_eq!(learned(&t, "/put"), None, "only a POST is learned");
        assert_eq!(
            learned(&t, "/drifted"),
            None,
            "a challenge that failed the drift check names nothing"
        );

        probe_and_record(&lo, &registry, &t, undeclared("/query")).await;
        assert_eq!(
            seller.methods("/query"),
            ["GET", "POST"],
            "its own challenge said POST"
        );
        probe_and_record(&lo, &registry, &t, undeclared("/elsewhere")).await;
        assert_eq!(seller.methods("/elsewhere"), ["GET", "GET"]);

        // The remembered POST stops answering and the GET still does: the
        // fallback answers, and what was learned is forgotten.
        probe_and_record(&lo, &registry, &t, undeclared("/forget")).await;
        assert_eq!(seller.methods("/forget"), ["GET", "POST", "GET"]);
        assert_eq!(learned(&t, "/forget"), None);
    }

    // ------------------------------------------------------------------------
    // The state machine
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_quarantine_built_by_get_is_reprobed_at_once_and_lifted_by_the_first_402() {
        let url = "https://seller.example/search";
        let post_target = target(url, post(None));
        let t = HealthTracker::new();
        // What the GET-only build left: three GET 404s, quarantined, and the
        // next probe an hour or more away.
        for _ in 0..3 {
            t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
                .await;
        }
        assert_eq!(status_of(&t, url).await, HealthStatus::Quarantined);
        let now = now_secs();
        assert!(
            tracker_due(&t, &post_target, now),
            "re-probed this cycle, not after the backoff"
        );
        // The same record under a listing that declares GET keeps its schedule:
        // that one was probed with the right request.
        assert!(!tracker_due(&t, &target(url, get()), now));

        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Alive,
            "the first live challenge to the right request lifts what the wrong one built"
        );
        assert!(
            !tracker_due(&t, &post_target, now_secs()),
            "once probed right, it is back on its schedule"
        );
    }

    #[tokio::test]
    async fn a_get_405_or_404_without_a_declared_method_is_reprobed_once_for_the_fallback() {
        for (class, code) in [(ProbeClass::AuthGated, 405u16), (ProbeClass::Fail, 404)] {
            let url = "https://seller.example/lookup";
            let undeclared = target(url, ProbeRequest::Undeclared);
            let t = HealthTracker::new();
            t.record_probe(url, class, Some(code), 1, None).await;
            assert!(
                tracker_due(&t, &undeclared, now_secs()),
                "{code}: not after its schedule"
            );
            t.record_probe(url, class, Some(code), 1, Some(ProbeMethod::Get))
                .await;
            assert!(
                !tracker_due(&t, &undeclared, now_secs()),
                "{code}: a build with the fallback already tried it"
            );
        }
    }

    #[tokio::test]
    async fn mcp_endpoints_keep_their_handshake_and_their_schedule() {
        let url = "https://mcp.seller.example/mcp";
        let mut mcp = target(url, post(None));
        mcp.resource_type = "mcp".to_string();
        let t = HealthTracker::new();
        t.record_probe(url, ProbeClass::AuthGated, Some(405), 1, None)
            .await;
        assert!(!tracker_due(&t, &mcp, now_secs()));
        t.record_probe(url, ProbeClass::PayToDrift, Some(402), 1, None)
            .await;
        assert!(!tracker_due(&t, &mcp, now_secs()));
    }

    #[tokio::test]
    async fn evidence_about_another_request_does_not_count_toward_quarantine() {
        let url = "https://seller.example/search";
        let t = HealthTracker::new();
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
            .await;
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
            .await;
        // One POST failure is one failure of THIS request, not the third.
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Post))
            .await;
        assert_ne!(status_of(&t, url).await, HealthStatus::Quarantined);
    }

    #[tokio::test]
    async fn a_drift_hold_is_never_lifted_by_a_single_402() {
        let url = "https://hijacked.example/pay";
        let post_target = target(url, post(None));

        // A hold this build set: its schedule stands, and a new request does
        // not shortcut its recovery.
        let t = HealthTracker::new();
        t.record_probe(
            url,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        assert!(!tracker_due(&t, &post_target, now_secs()));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Quarantined,
            "one clean challenge to another request does not lift it"
        );
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Alive,
            "two do, as always"
        );
        assert_eq!(reason_of(&t, url).await, None);

        // A hold an older build set (no method, no reason): read by its
        // signature, looked at once now -- and still not lifted by one 402.
        let legacy = HealthTracker::new();
        legacy
            .record_probe(url, ProbeClass::PayToDrift, Some(402), 1, None)
            .await;
        legacy
            .records
            .write()
            .await
            .get_mut(url)
            .unwrap()
            .quarantine_reason = None;
        assert_eq!(
            reason_of(&legacy, url).await,
            Some(QuarantineReason::PayToDrift)
        );
        assert!(
            tracker_due(&legacy, &post_target, now_secs()),
            "looked at once"
        );
        legacy
            .record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
        assert_eq!(status_of(&legacy, url).await, HealthStatus::Quarantined);
        assert!(
            !tracker_due(&legacy, &post_target, now_secs()),
            "and only once"
        );
    }

    #[tokio::test]
    async fn a_drift_hold_is_not_lifted_by_an_answer_that_is_not_a_challenge() {
        for (class, code) in [
            (ProbeClass::AuthGated, Some(401)),
            (ProbeClass::Degraded, Some(400)),
            (ProbeClass::Unprobeable, None),
        ] {
            let url = "https://hijacked.example/pay";
            let t = HealthTracker::new();
            t.record_probe(
                url,
                ProbeClass::PayToDrift,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            t.record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            t.record_probe(url, class, code, 1, Some(ProbeMethod::Post))
                .await;
            assert_eq!(
                status_of(&t, url).await,
                HealthStatus::Quarantined,
                "{class:?}"
            );
            assert_eq!(
                reason_of(&t, url).await,
                Some(QuarantineReason::PayToDrift),
                "{class:?}"
            );
            // And it reset the clean streak: one more challenge is not two.
            t.record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            assert_eq!(
                status_of(&t, url).await,
                HealthStatus::Quarantined,
                "{class:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_legacy_drift_hold_stays_a_drift_hold_through_a_failure() {
        // Read off its signature on the first probe by this build, and KEPT:
        // once its last status code is a 404, the signature would say "fail
        // streak", and a change of request would lift it with one 402.
        let url = "https://hijacked.example/pay";
        let old: HashMap<String, HealthRecord> = serde_json::from_value(json!({
            "https://hijacked.example/pay": { "status": "quarantined", "http_status": 402 }
        }))
        .unwrap();
        let t = HealthTracker::new();
        *t.records.write().await = old;
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Get))
            .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Quarantined,
            "a new request does not lift a drift hold"
        );
    }

    #[tokio::test]
    async fn the_quarantine_reason_is_kept_and_published() {
        let url = "https://seller.example/x";
        let t = HealthTracker::new();
        for _ in 0..3 {
            t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Get))
                .await;
        }
        let json = serde_json::to_value(&t.snapshot().await[url]).unwrap();
        assert_eq!(json["quarantineReason"], "fail_streak");
        // One clean challenge into the recovery: still a fail streak, not a
        // drift -- the reason is kept, not guessed from the last status code.
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
            .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::FailStreak));
        t.record_probe(
            url,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        for _ in 0..2 {
            t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
                .await;
        }
        let json = serde_json::to_value(&t.snapshot().await[url]).unwrap();
        assert_eq!(json["status"], "alive");
        assert!(
            json.get("quarantineReason").is_none(),
            "only while quarantined"
        );

        // A record from before the reason existed, as the overlay holds it.
        let old: HashMap<String, HealthRecord> = serde_json::from_value(json!({
            "https://a.example/x": { "status": "quarantined", "http_status": 402 },
            "https://b.example/x": { "status": "quarantined", "http_status": 404 },
            "https://c.example/x": {
                "status": "quarantined", "http_status": 402, "consecutive_ok": 1
            }
        }))
        .unwrap();
        let t = HealthTracker::new();
        *t.records.write().await = old;
        assert_eq!(
            reason_of(&t, "https://a.example/x").await,
            Some(QuarantineReason::PayToDrift)
        );
        assert_eq!(
            reason_of(&t, "https://b.example/x").await,
            Some(QuarantineReason::FailStreak)
        );
        assert_eq!(
            reason_of(&t, "https://c.example/x").await,
            Some(QuarantineReason::FailStreak),
            "one clean challenge into a recovery is not a drift"
        );
        // Its first probe by this build keeps the reason the signature gave it,
        // even when that probe's 402 would read as a drift on its own.
        t.record_probe(
            "https://b.example/x",
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(
            reason_of(&t, "https://b.example/x").await,
            Some(QuarantineReason::FailStreak)
        );
    }

    #[test]
    fn the_drift_check_ignores_a_quote_reference_and_nothing_else() {
        let listing = tavily();
        // What the catalog keeps from this listing, by the import rule itself:
        // the agent-pay option is dropped, so its quote reference is never a
        // declared recipient.
        let (mut imported, _rejected) = crate::discovery_aggregator::convert_resources(
            vec![serde_json::from_value(listing.clone()).unwrap()],
            "coinbase",
        );
        let declared: Vec<String> = imported
            .remove(0)
            .accepts
            .iter()
            .map(|a| a.pay_to.to_string().to_ascii_lowercase())
            .collect();
        assert_eq!(declared, ["0xfe2d09ca270818e9736207ee27f0fa464a67ac66"]);

        let live = pay_to_from_402(None, Some(&challenge_for(&listing["accepts"])));
        assert!(live.readable);
        assert_eq!(
            live.pay_to, declared,
            "urn:x402:agent-pay:see-quote is not a recipient"
        );
        assert!(
            !pay_to_drifted(&declared, &live),
            "the seller's own challenge"
        );

        let with = |slot: usize, pay_to: &str| {
            let mut a = listing["accepts"].clone();
            a[slot]["payTo"] = json!(pay_to);
            pay_to_from_402(None, Some(&challenge_for(&a)))
        };
        // Another URN, in any case, is still not a recipient.
        assert!(!pay_to_drifted(
            &declared,
            &with(1, "URN:x402:agent-pay:other")
        ));
        // The declared address in another spelling is the declared address.
        assert!(!pay_to_drifted(
            &declared,
            &with(0, " 0XFE2D09CA270818E9736207EE27F0FA464A67AC66 ")
        ));
        // Fails closed: anything else counts, in either slot -- including
        // spellings and chains this build cannot parse.
        for slot in [0, 1] {
            for swapped in [
                "0x000000000000000000000000000000000000dEaD",
                "0X000000000000000000000000000000000000DEAD",
                "attacker-account",
                "cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu",
                "eip155:8453:0x000000000000000000000000000000000000dEaD",
            ] {
                assert!(
                    pay_to_drifted(&declared, &with(slot, swapped)),
                    "slot {slot}: {swapped}"
                );
            }
        }
        // A payTo that is not a string is no recipient we can name: it counts.
        let mut odd = listing["accepts"].clone();
        odd[1]["payTo"] = json!({ "to": "0x000000000000000000000000000000000000dEaD" });
        let live = pay_to_from_402(None, Some(&challenge_for(&odd)));
        assert!(pay_to_drifted(&declared, &live), "fails closed");
        odd[1]["payTo"] = Value::Null;
        let live = pay_to_from_402(None, Some(&challenge_for(&odd)));
        assert!(
            !pay_to_drifted(&declared, &live),
            "a null one is nothing at all"
        );
    }

    #[tokio::test]
    async fn the_observation_records_the_request_that_drew_it() {
        let registry = DiscoveryRegistry::new();
        let overlay = registry.terms();
        let url = url::Url::parse("https://x402.tavily.com/search").unwrap();
        let live = pay_to_from_402(None, Some(&challenge_for(&accepts())));
        record_observation(
            &registry,
            &overlay,
            &url,
            "http",
            ProbeMethod::Post,
            Some(402),
            live,
        )
        .await;
        let observed = overlay.get(url.as_str()).await.unwrap();
        assert_eq!(observed.context.method, "POST");
        assert_eq!(
            observed.accepts[0].amount.to_string(),
            "10000",
            "and the price it read"
        );
    }

    #[tokio::test]
    async fn the_uptime_attested_is_the_uptime_listed() {
        let t = HealthTracker::new();
        let url = "https://seller.example/x";
        assert!(t.snapshot().await.is_empty());
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
            .await;
        t.record_probe(url, ProbeClass::Fail, Some(500), 1, Some(ProbeMethod::Get))
            .await;
        t.record_probe(
            url,
            ProbeClass::AuthGated,
            Some(401),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        let state = t.snapshot().await.remove(url).unwrap();
        assert_eq!(state.probe_count, Some(3));
        assert_eq!(state.uptime_bps, Some(6666));
        assert_eq!(
            t.uptime(url).await,
            Some((6666, 3, 2)),
            "the attestation's number"
        );
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["uptimeBps"], 6666);
        assert_eq!(json["probeCount"], 3);
        assert_eq!(json["probeMethod"], "GET");
        // A record that predates all of this serializes exactly as before.
        let legacy = HealthTracker::new();
        legacy
            .record_probe(url, ProbeClass::Alive, Some(402), 1, None)
            .await;
        let json = serde_json::to_value(legacy.snapshot().await.remove(url).unwrap()).unwrap();
        assert!(json.get("probeMethod").is_none());
        assert!(json.get("quarantineReason").is_none());
    }

    // ------------------------------------------------------------------------
    // End to end
    // ------------------------------------------------------------------------

    /// The listings the router report found live but hidden or mislabelled,
    /// plus the first-party family measured the same day, end to end: the
    /// catalog import, `probe_targets`, the record the GET-only build left,
    /// one `probe_and_record`, and what the listing shows after it.
    #[tokio::test]
    async fn the_reported_listings_are_found_with_the_request_they_declare() {
        let file: Value = serde_json::from_str(REPORT_CASES).unwrap();
        let cases = file["cases"].as_array().unwrap();
        let registry = DiscoveryRegistry::new();
        let mut routes = HashMap::new();
        let mut resources = Vec::new();
        for case in cases {
            let listing = match case["capturedIn"].as_str() {
                Some(_) => captured(case["resource"].as_str().unwrap()),
                None => case["listing"].clone(),
            };
            let (mut imported, _) = crate::discovery_aggregator::convert_resources(
                vec![serde_json::from_value(listing.clone()).unwrap()],
                "coinbase",
            );
            let resource = imported.remove(0);
            routes.insert(
                resource.url.path().to_string(),
                route(
                    case["seller"]["GET"].as_u64().unwrap() as u16,
                    case["seller"]["POST"].as_u64().unwrap() as u16,
                    &listing["accepts"],
                ),
            );
            resources.push(resource.url.to_string());
            registry.register(resource).await.unwrap();
        }
        let (base, seller) = serve(routes).await;
        let lo = Loopback::serving(&base);
        let targets = registry.probe_targets().await;
        assert_eq!(targets.len(), cases.len());

        for (case, resource) in cases.iter().zip(&resources) {
            let name = case["name"].as_str().unwrap();
            let target = targets
                .iter()
                .find(|t| t.url.as_str() == resource)
                .unwrap()
                .clone();
            let url = target.url.to_string();

            // What the GET-only build left behind.
            let t = HealthTracker::new();
            match case["reported"].as_str().unwrap() {
                "quarantined" => {
                    for _ in 0..3 {
                        t.record_probe(&url, ProbeClass::Fail, Some(404), 1, None)
                            .await;
                    }
                }
                "auth_gated" => {
                    t.record_probe(&url, ProbeClass::AuthGated, Some(405), 1, None)
                        .await;
                }
                other => panic!("{name}: unexpected label {other}"),
            }

            let due_now = tracker_due(&t, &target, now_secs());
            probe_and_record(&lo, &registry, &t, target.clone()).await;
            let state = t.snapshot().await.remove(&url).unwrap();
            let expect = &case["expect"];
            assert_eq!(
                state.probe_method.as_deref(),
                expect["method"].as_str(),
                "{name}: answered by"
            );
            assert_eq!(state.http_status, Some(402), "{name}");
            let visible = state.status != HealthStatus::Quarantined;
            let expect_visible = expect["visibleAfterOneProbe"].as_bool().unwrap();
            assert_eq!(due_now, expect_visible, "{name}: re-probed this cycle");
            assert_eq!(visible, expect_visible, "{name}: visible after one probe");
            if visible {
                assert_eq!(state.status, HealthStatus::Alive, "{name}: same vocabulary");
            } else {
                assert_eq!(
                    state.quarantine_reason,
                    Some(QuarantineReason::FailStreak),
                    "{name}: and it says why"
                );
            }
            assert!(
                !tracker_due(&t, &target, now_secs()),
                "{name}: back on its schedule after one probe"
            );
            let observed = registry.terms().get(&url).await.unwrap();
            assert_eq!(
                Some(observed.context.method.as_str()),
                expect["method"].as_str(),
                "{name}: the price says which request it answers"
            );
        }
        // Tavily got `{}` and answered: its example never travelled.
        let tavily = seller.seen("/search");
        assert!(tavily
            .iter()
            .all(|s| s.body.is_empty() || s.body == EMPTY_JSON_BODY));
        // The one that declares nothing: GET, then the one fallback POST.
        assert_eq!(seller.methods("/api/phone-lookup"), ["GET", "POST"]);
        assert_eq!(
            seller.methods("/payments/access/alpha-test"),
            ["GET", "POST"]
        );
    }
}
