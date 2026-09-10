//! Observed payment terms: what the origin actually answered, and when.
//!
//! # Why this is not part of the resource record
//!
//! A catalog entry is a *claim* about someone else's price, made by whoever
//! published it. An observation is a *measurement* of the origin's live 402.
//! They are different statements with different authors, and the moment they are
//! stored in the same field the second one can be erased by a re-import of the
//! first -- which is exactly the failure this phase exists to remove.
//!
//! So observations live in their own overlay (`bazaar/terms.json`), on the same
//! discipline the liveness overlay (WS-B) already uses:
//!
//! * **One writer.** The health prober is the only component that observes a
//!   402, so it is the only component that writes here. No lease, no contention
//!   between the aggregator and the prober over one object.
//! * **Structurally un-clobberable.** An import writes `bazaar/resources.json`.
//!   It cannot reach this file, so "a stale feed overwrote a direct observation"
//!   stops being a rule we enforce and becomes a shape we cannot express.
//! * **Rollback-safe.** A build that predates this module neither reads nor
//!   writes this object, so rolling back leaves the observations untouched and
//!   rolling forward finds them where it left them.
//!
//! # What an observation is not
//!
//! It is not a quote, it is not a guarantee, and it is not a price for a request
//! other than the one that was made. The prober issues an unauthenticated `GET`
//! on the listing URL; that is the whole context, and it is recorded as such.
//! A `POST` with parameters is a different request and may legitimately cost
//! something else -- which is why [`ObservationContext`] is stored next to the
//! terms rather than left implicit.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::discovery_price::CatalogPaymentOption;
use crate::types_v2::{DiscoveryResource, DiscoverySource};

/// Format version of the persisted overlay.
///
/// The object is written as `{"version": N, "records": {...}}`. Version 1 is the
/// first shape there has ever been; a bare map with no envelope is read as
/// version 1 too, so an overlay written by a build between this module landing
/// and the envelope existing would still load.
pub const TERMS_OVERLAY_VERSION: u32 = 1;

/// Most payment options kept from a single observation.
///
/// A 402 with more options than this is either unusual or hostile, and the
/// overlay is written whole on every flush: one seller must not be able to
/// decide how large everyone else's write is. Real challenges carry one to four.
const MAX_OBSERVED_OPTIONS: usize = 6;

/// Most observations retained, oldest evicted first.
///
/// Sizing: an observation serializes to roughly 300-600 bytes with the option
/// cap above, so 2 000 bounds this object around 1 MB.
///
/// It tracks the catalog's own cap ([`crate::discovery::DEFAULT_MAX_RESOURCES`])
/// deliberately. The overlay is keyed by catalog URL and pruned against the live
/// set, so it can never hold more readings than there are resources; a larger
/// number here would be unreachable, and after 2026-09-10 a stray 20 000 in the
/// configuration is worse than unreachable -- it reads as a catalog scale this
/// service is provisioned for, which is the whole thing that incident disproved.
fn max_records() -> usize {
    std::env::var("DISCOVERY_TERMS_MAX_RECORDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2_000)
}

/// Minimum seconds between two flushes of the overlay to S3.
///
/// The liveness overlay is flushed every prober tick (60s). This one is bigger
/// and moves far more slowly -- a price observed twice in five minutes is the
/// same observation -- so it gets its own, slower debounce rather than riding
/// the tick.
fn persist_interval_secs() -> u64 {
    std::env::var("DISCOVERY_TERMS_PERSIST_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300)
}

/// How long an observation is treated as current.
///
/// Defaults to the healthy re-probe cadence, deliberately: freshness has to mean
/// "observed within the policy we actually run", not an aspiration. Probing a
/// healthy resource every seven days and then calling everything older than a
/// day stale would mark the entire catalog stale on the first read and say
/// nothing. Adaptive cadence is the next phase's work; when it lands, this
/// window follows it.
pub fn freshness_window_secs() -> u64 {
    std::env::var("DISCOVERY_TERMS_FRESH_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(7 * 24 * 3600)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ============================================================================
// Provenance, phase, context
// ============================================================================

/// Where a set of terms came from, ordered by how directly it was witnessed.
///
/// The ladder is the answer to "which of these two do I believe", and it is
/// deliberately NOT the timestamp each side publishes about itself. A feed that
/// stamps its own copy with today's date has not thereby learned anything about
/// the seller's price; it has only written a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermsProvenance {
    /// A third party's copy of somebody else's listing.
    AggregatedFeed,
    /// The origin's own published document, fetched by us.
    OwnerDocument,
    /// The owner told this registry directly.
    OwnerDeclared,
    /// We made the request and read the answer.
    OriginResponse,
}

impl TermsProvenance {
    /// Rank in the ladder. Higher wins.
    pub fn rank(self) -> u8 {
        match self {
            TermsProvenance::AggregatedFeed => 1,
            TermsProvenance::OwnerDocument => 2,
            TermsProvenance::OwnerDeclared => 3,
            TermsProvenance::OriginResponse => 4,
        }
    }
}

/// Which half of a payment an observation describes.
///
/// It matters for `upto` and it matters nowhere else, which is precisely why it
/// has to be recorded: in the verification phase the amount is the ceiling the
/// buyer authorizes, and in the settlement phase it can be the amount actually
/// charged. Comparing one against the other reports a price change that never
/// happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationPhase {
    /// Read from a payment challenge, before any payment.
    Verification,
    /// Read from a completed settlement.
    Settlement,
}

/// Which transport carried the challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermsTransport {
    /// The `PAYMENT-REQUIRED` (or `X-PAYMENT-REQUIRED`) response header.
    Header,
    /// The response body.
    Body,
}

/// The request an observation answers.
///
/// Stored rather than assumed. "The price of this resource" is not a
/// well-formed question: an unauthenticated GET of a listing URL and an
/// authenticated POST carrying parameters are two different purchases, and only
/// the first is one this prober can make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationContext {
    /// HTTP method used.
    pub method: String,
    /// Catalog resource type the probe was shaped for (`http`, `mcp`, ...).
    pub resource_type: String,
    /// Whether credentials were attached. Always false here: the prober never
    /// authenticates, so a variant behind a login stays unverified rather than
    /// being reported as dead or as free.
    pub authenticated: bool,
}

impl ObservationContext {
    /// An unauthenticated GET of the listing URL -- the only context the health
    /// prober can produce.
    pub fn anonymous_get(resource_type: &str) -> Self {
        Self {
            method: "GET".to_string(),
            resource_type: resource_type.to_string(),
            authenticated: false,
        }
    }

    /// Stable identifier for the context, for grouping observations later.
    pub fn key(&self) -> String {
        format!(
            "{} {} {}",
            self.method,
            self.resource_type,
            if self.authenticated { "auth" } else { "anon" }
        )
    }
}

// ============================================================================
// The observation
// ============================================================================

/// The terms one transport of one challenge declared.
///
/// Kept as evidence when the two transports disagree, rather than merged into
/// the other: a synthesized offer combining a header's network with a body's
/// amount is an offer nobody made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportReading {
    pub transport: TermsTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x402_version: Option<u64>,
    pub accepts: Vec<CatalogPaymentOption>,
}

/// One reading of an origin's live payment terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedTerms {
    /// The payment options the origin advertised, from the winning transport.
    ///
    /// `extra` is deliberately not carried here. The declared listing is where a
    /// scheme's parameters live; this record exists to date and compare a price,
    /// and copying every seller's decoration into an object that is rewritten
    /// whole would multiply it by an order of magnitude for no reader.
    pub accepts: Vec<CatalogPaymentOption>,

    /// When the observation was made (Unix seconds). Never a fetch time
    /// borrowed from something else, and never invented.
    pub observed_at: u64,

    /// The request this answers.
    pub context: ObservationContext,

    /// Verification or settlement. See [`ObservationPhase`].
    pub phase: ObservationPhase,

    /// How directly the terms were witnessed.
    pub provenance: TermsProvenance,

    /// Which transport the winning reading came from.
    pub transport: TermsTransport,

    /// Protocol version the challenge declared, when it declared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x402_version: Option<u64>,

    /// Status code of the response that carried the challenge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,

    /// Fingerprint of the catalog record as it stood when this was observed.
    ///
    /// Not for authenticity -- a hash proves neither origin nor currency. It
    /// answers one question: has the listing been revised since we last looked
    /// at the origin? If it has, the observation describes an older revision and
    /// the resource is due for revalidation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    /// The other transport's reading, kept when the two disagreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<TransportReading>,

    /// Options in the challenge we could not read, counted by cause.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rejected: BTreeMap<String, usize>,

    /// Whether the challenge carried more options than we kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl ObservedTerms {
    /// Apply the storage bounds: cap the option count and drop `extra`.
    pub fn bounded(mut self) -> Self {
        if self.accepts.len() > MAX_OBSERVED_OPTIONS {
            self.accepts.truncate(MAX_OBSERVED_OPTIONS);
            self.truncated = true;
        }
        for option in self.accepts.iter_mut() {
            option.extra = None;
            option.strip_response_only();
        }
        if let Some(conflict) = self.conflict.as_mut() {
            conflict.accepts.truncate(MAX_OBSERVED_OPTIONS);
            for option in conflict.accepts.iter_mut() {
                option.extra = None;
                option.strip_response_only();
            }
        }
        self
    }
}

// ============================================================================
// Freshness
// ============================================================================

/// How much the catalog knows about a resource's current price.
///
/// Independent of `health` on purpose. A resource can be perfectly alive and
/// have an unverified price (nothing has read its challenge), and a resource can
/// have a freshly observed price and be quarantined for a payTo swap. Collapsing
/// the two is how "it answers 402" came to be read as "the listed price is
/// right".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PriceFreshness {
    /// Observed within the freshness window and agreeing with the listing.
    /// Not a guarantee: it is a statement about a past reading, not a quote.
    Fresh,
    /// Observed, but the reading has aged out of the window or the listing has
    /// been revised since. Revalidation is due.
    Stale,
    /// Never observed. The listing may be perfectly correct; nothing has checked.
    Unknown,
    /// A current observation disagrees with the listing on a comparable option.
    Conflict,
}

impl PriceFreshness {
    pub fn as_str(self) -> &'static str {
        match self {
            PriceFreshness::Fresh => "fresh",
            PriceFreshness::Stale => "stale",
            PriceFreshness::Unknown => "unknown",
            PriceFreshness::Conflict => "conflict",
        }
    }
}

/// Whether two payment options describe the same purchase.
///
/// Same scheme, network, asset and recipient. Anything else is not the same
/// commercial offer -- an identical number in a different currency, on a
/// different chain, or to a different payee is not the same price, and reporting
/// a change between them would be noise.
fn same_offer(a: &CatalogPaymentOption, b: &CatalogPaymentOption) -> bool {
    a.scheme == b.scheme
        && a.network == b.network
        && a.asset
            .to_string()
            .eq_ignore_ascii_case(&b.asset.to_string())
        && a.pay_to
            .to_string()
            .eq_ignore_ascii_case(&b.pay_to.to_string())
}

/// Whether a live observation contradicts what the catalog publishes.
///
/// Two ways to disagree, and both are real:
///
/// 1. A comparable option exists on both sides and the amounts differ.
/// 2. The origin advertises options and NONE of them is comparable to anything
///    the listing declares -- the catalog is describing an offer the origin no
///    longer makes.
///
/// Only the verification phase is compared. A settlement observation reports
/// what was charged, which for `upto` is legitimately below the listed ceiling.
pub fn terms_disagree(declared: &[CatalogPaymentOption], observed: &ObservedTerms) -> bool {
    if observed.phase != ObservationPhase::Verification {
        return false;
    }
    if observed.accepts.is_empty() || declared.is_empty() {
        return false;
    }
    let mut any_comparable = false;
    for d in declared {
        for o in observed.accepts.iter().filter(|o| same_offer(d, o)) {
            any_comparable = true;
            if d.amount != o.amount {
                return true;
            }
        }
    }
    !any_comparable
}

/// Where a stored record's terms came from, on the same ladder.
pub fn record_provenance(source: DiscoverySource) -> TermsProvenance {
    match source {
        // The owner posted these terms to this registry.
        DiscoverySource::SelfRegistered => TermsProvenance::OwnerDeclared,
        // Terms we watched settle. First-hand, and about a real payment.
        DiscoverySource::Settlement => TermsProvenance::OriginResponse,
        // The origin's own well-known document, fetched by us.
        DiscoverySource::Crawled => TermsProvenance::OwnerDocument,
        // A third party's copy.
        DiscoverySource::Aggregated => TermsProvenance::AggregatedFeed,
    }
}

/// Classify what the catalog knows about `resource`'s price.
///
/// The order of these four questions is the whole behaviour:
///
/// 1. **Has the reading aged out?** Then it is not current belief, and it
///    reports `stale` rather than asserting a conflict on evidence nobody has
///    rechecked.
/// 2. **Did the ORIGIN revise the listing after the reading?** Then the reading
///    describes a revision that no longer exists. A seller repricing is ordinary
///    commerce and must not be published as a contradiction for the days until
///    the next probe: it is `stale`, and revalidation is due. This is the annex's
///    rule that a new checked revision from the owner makes the previous
///    observation obsolete -- and obsolete is not deleted.
/// 3. **Do they disagree?** Then `conflict`, with both sides returned. Note this
///    is reached when a *third party's copy* moved and our direct reading
///    contradicts it: an aggregator changing its copy is not the seller
///    speaking, and a direct reading outranks a copy on the ladder.
/// 4. Otherwise the reading stands, `fresh`, for exactly what it is: a past
///    reading of one request context, and never a quote.
pub fn assess_freshness(
    resource: &DiscoveryResource,
    observed: Option<&ObservedTerms>,
    now: u64,
    window: u64,
) -> PriceFreshness {
    let Some(o) = observed else {
        return PriceFreshness::Unknown;
    };
    // We looked, and the challenge carried no option we could read. That dates
    // the LOOK, not the price: `termsObservedAt` is set and the price stays
    // unverified. Treating an empty reading as agreement would report every
    // unparseable challenge as a confirmed price, which is the same class of
    // mistake as a hijack check that passes because it saw nothing.
    if o.accepts.is_empty() {
        return PriceFreshness::Unknown;
    }
    if now.saturating_sub(o.observed_at) > window {
        return PriceFreshness::Stale;
    }
    let revised = o
        .content_hash
        .as_deref()
        .is_some_and(|hash| hash != resource.content_fingerprint());
    let from_the_origin =
        record_provenance(resource.source).rank() >= TermsProvenance::OwnerDocument.rank();
    if revised && from_the_origin {
        return PriceFreshness::Stale;
    }
    if terms_disagree(&resource.accepts, o) {
        return PriceFreshness::Conflict;
    }
    if revised {
        return PriceFreshness::Stale;
    }
    PriceFreshness::Fresh
}

// ============================================================================
// The overlay
// ============================================================================

struct S3Overlay {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
}

/// Persisted envelope. See [`TERMS_OVERLAY_VERSION`].
#[derive(Debug, Serialize, Deserialize)]
struct TermsSnapshot {
    version: u32,
    records: HashMap<String, serde_json::Value>,
}

/// In-memory observed-terms records plus optional S3 persistence.
///
/// Deliberately shaped like [`crate::discovery_health::HealthTracker`]: same
/// lock discipline, same best-effort persistence, same rule that a failure here
/// never touches a payment or a listing.
pub struct TermsOverlay {
    records: RwLock<HashMap<String, ObservedTerms>>,
    overlay: RwLock<Option<S3Overlay>>,
    dirty: AtomicBool,
    last_persist: AtomicU64,
}

impl Default for TermsOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl TermsOverlay {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            overlay: RwLock::new(None),
            dirty: AtomicBool::new(false),
            last_persist: AtomicU64::new(0),
        }
    }

    /// Attach an S3 overlay and load whatever is already there.
    ///
    /// Records are decoded one at a time. A single entry this build can no
    /// longer parse -- a network name that was dropped, say -- must cost us that
    /// entry and nothing else; failing the whole object would throw away every
    /// observation because of one.
    pub async fn configure_s3(&self, bucket: String, key: String) {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);
        match client.get_object().bucket(&bucket).key(&key).send().await {
            Ok(obj) => {
                if let Ok(bytes) = obj.body.collect().await {
                    let data = bytes.into_bytes();
                    let (loaded, version, dropped) = decode_overlay(&data);
                    let n = loaded.len();
                    *self.records.write().await = loaded;
                    info!(
                        count = n,
                        dropped = dropped,
                        format_version = version,
                        "Loaded observed-terms overlay from S3"
                    );
                }
            }
            Err(e) => {
                debug!(error = %e, "No existing observed-terms overlay (starting empty)");
            }
        }
        *self.overlay.write().await = Some(S3Overlay {
            client,
            bucket,
            key,
        });
    }

    /// Record one observation, evicting the oldest if the overlay is full.
    pub async fn record(&self, url: &str, terms: ObservedTerms) {
        let terms = terms.bounded();
        let cap = max_records();
        let mut records = self.records.write().await;
        if !records.contains_key(url) && records.len() >= cap {
            // Oldest observation first: the point of the cap is to bound the
            // object, and the least useful thing in it is the reading nobody has
            // refreshed in longest.
            if let Some(oldest) = records
                .iter()
                .min_by_key(|(_, t)| t.observed_at)
                .map(|(u, _)| u.clone())
            {
                records.remove(&oldest);
            }
        }
        records.insert(url.to_string(), terms);
        drop(records);
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Response-facing snapshot, taken BEFORE the caller locks the registry.
    ///
    /// This is why it exists as a separate call rather than a lookup during
    /// composition: the overlay is behind its own async lock, and awaiting it
    /// while holding the resource guard is the guard-across-await hazard the
    /// listing path already avoids for health and curation.
    pub async fn snapshot(&self) -> HashMap<String, ObservedTerms> {
        self.records.read().await.clone()
    }

    /// Drop observations for URLs the catalog no longer holds.
    ///
    /// The same hygiene the liveness overlay gained in 2.21.1, and for the same
    /// reason: this object is written whole, so a reading kept for a resource
    /// that left the catalog is bytes uploaded forever for a record nothing can
    /// read. The cap above would eventually push them out, but only by evicting
    /// live readings first, which is the wrong thing to spend the budget on.
    ///
    /// **An empty catalog prunes nothing.** When the S3 read fails at startup
    /// `main` does not stop -- it falls back to an empty in-memory registry and
    /// keeps serving -- so an empty keep-set means "we could not read the
    /// catalog", never "the catalog is empty". Same rule as the store's, and the
    /// same rule the liveness overlay follows.
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

    /// One resource's latest observation.
    pub async fn get(&self, url: &str) -> Option<ObservedTerms> {
        self.records.read().await.get(url).cloned()
    }

    /// Flush to S3 if dirty and the debounce interval has elapsed.
    ///
    /// Best-effort throughout: an overlay we could not write costs us the
    /// freshness annotation on a restart, and nothing else. It must never be
    /// able to fail a probe, a listing or a payment.
    pub async fn persist(&self) {
        if !self.dirty.load(Ordering::SeqCst) {
            return;
        }
        let now = now_secs();
        let last = self.last_persist.load(Ordering::SeqCst);
        if last != 0 && now.saturating_sub(last) < persist_interval_secs() {
            return;
        }
        let guard = self.overlay.read().await;
        let Some(overlay) = guard.as_ref() else {
            return;
        };
        let body = {
            let records = self.records.read().await;
            let encoded: HashMap<String, serde_json::Value> = records
                .iter()
                .filter_map(|(u, t)| serde_json::to_value(t).ok().map(|v| (u.clone(), v)))
                .collect();
            match serde_json::to_vec(&TermsSnapshot {
                version: TERMS_OVERLAY_VERSION,
                records: encoded,
            }) {
                Ok(b) => b,
                Err(e) => {
                    error!(error = %e, "Observed-terms overlay serialize failed");
                    return;
                }
            }
        };
        // Cleared before the PUT: a write that fails leaves the overlay dirty
        // for the next flush rather than silently dropping the interval.
        match overlay
            .client
            .put_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .body(body.into())
            .content_type("application/json")
            .send()
            .await
        {
            Ok(_) => {
                self.dirty.store(false, Ordering::SeqCst);
                self.last_persist.store(now, Ordering::SeqCst);
            }
            Err(e) => error!(error = %e, "Failed to persist observed-terms overlay"),
        }
    }
}

/// Decode an overlay object, tolerating the pre-envelope shape.
///
/// Returns the records, the format version they were read at, and how many
/// entries were dropped because this build could not parse them.
fn decode_overlay(data: &[u8]) -> (HashMap<String, ObservedTerms>, u32, usize) {
    let (raw, version) = match serde_json::from_slice::<TermsSnapshot>(data) {
        Ok(snapshot) => (snapshot.records, snapshot.version),
        Err(_) => match serde_json::from_slice::<HashMap<String, serde_json::Value>>(data) {
            // A bare map is the shape an overlay would have without the
            // envelope. Reading it as version 1 costs nothing and means the
            // envelope can be introduced without a migration step.
            Ok(records) => (records, 1),
            Err(e) => {
                warn!(error = %e, "Observed-terms overlay parse failed; starting empty");
                return (HashMap::new(), 0, 0);
            }
        },
    };
    let mut out = HashMap::with_capacity(raw.len());
    let mut dropped = 0usize;
    for (url, value) in raw {
        match serde_json::from_value::<ObservedTerms>(value) {
            Ok(terms) => {
                out.insert(url, terms);
            }
            Err(e) => {
                dropped += 1;
                debug!(url = %url, error = %e, "Dropping unreadable observed-terms record");
            }
        }
    }
    (out, version, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery_price::{normalize_declared_option, DeclaredPaymentOption};
    use url::Url;

    fn option(scheme: &str, amount: &str, pay_to: &str) -> CatalogPaymentOption {
        normalize_declared_option(DeclaredPaymentOption {
            scheme: Some(scheme.to_string()),
            network: Some("base".to_string()),
            asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string()),
            amount: Some(serde_json::json!(amount)),
            pay_to: Some(pay_to.to_string()),
            max_timeout_seconds: Some(60),
            ..Default::default()
        })
        .expect("fixture option must normalize")
    }

    fn on_network(network: &str) -> CatalogPaymentOption {
        normalize_declared_option(DeclaredPaymentOption {
            scheme: Some("exact".to_string()),
            network: Some(network.to_string()),
            asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string()),
            amount: Some(serde_json::json!("10000")),
            pay_to: Some("0xe4dc963c56979E0260fc146b87eE24F18220e545".to_string()),
            max_timeout_seconds: Some(60),
            ..Default::default()
        })
        .expect("fixture option must normalize")
    }

    fn resource(accepts: Vec<CatalogPaymentOption>) -> DiscoveryResource {
        DiscoveryResource::new(
            Url::parse("https://api.example.com/x").unwrap(),
            "http".to_string(),
            "a resource".to_string(),
            accepts,
        )
    }

    fn observation(accepts: Vec<CatalogPaymentOption>, observed_at: u64) -> ObservedTerms {
        ObservedTerms {
            accepts,
            observed_at,
            context: ObservationContext::anonymous_get("http"),
            phase: ObservationPhase::Verification,
            provenance: TermsProvenance::OriginResponse,
            transport: TermsTransport::Header,
            x402_version: Some(2),
            http_status: Some(402),
            content_hash: None,
            conflict: None,
            rejected: BTreeMap::new(),
            truncated: false,
        }
    }

    const PAYEE: &str = "0xe4dc963c56979E0260fc146b87eE24F18220e545";

    #[test]
    fn an_unobserved_resource_is_unknown_not_fresh() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        assert_eq!(
            assess_freshness(&r, None, 1_000, 3_600),
            PriceFreshness::Unknown
        );
    }

    #[test]
    fn a_reading_that_produced_no_legible_option_does_not_verify_a_price() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        let mut o = observation(vec![], 1_000);
        o.rejected.insert("amount-not-an-integer".to_string(), 1);
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Unknown,
            "we dated the look, not the price"
        );
    }

    #[test]
    fn an_agreeing_observation_inside_the_window_is_fresh() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        let o = observation(vec![option("exact", "10000", PAYEE)], 1_000);
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_500, 3_600),
            PriceFreshness::Fresh
        );
    }

    #[test]
    fn a_different_amount_on_the_same_offer_is_a_conflict() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        let o = observation(vec![option("exact", "30000", PAYEE)], 1_000);
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_500, 3_600),
            PriceFreshness::Conflict
        );
    }

    #[test]
    fn the_same_amount_on_a_different_network_is_not_a_price_change() {
        // Same number, different chain. Not the same commercial offer, so this
        // is a listing the origin no longer makes -- reported as a conflict --
        // and never as "the price moved from X to X".
        let r = resource(vec![on_network("base")]);
        let o = observation(vec![on_network("polygon")], 1_000);
        assert!(terms_disagree(&r.accepts, &o));
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Conflict
        );
    }

    #[test]
    fn an_expired_observation_reports_stale_rather_than_asserting_a_conflict() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        let o = observation(vec![option("exact", "30000", PAYEE)], 1_000);
        assert_eq!(
            assess_freshness(&r, Some(&o), 100_000, 3_600),
            PriceFreshness::Stale,
            "a reading nobody has rechecked is not current belief"
        );
    }

    #[test]
    fn a_revised_listing_makes_the_previous_observation_stale() {
        let r = resource(vec![option("exact", "10000", PAYEE)]);
        let mut o = observation(vec![option("exact", "10000", PAYEE)], 1_000);
        o.content_hash = Some("a-fingerprint-from-an-older-revision".to_string());
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Stale
        );
        // ... and the same observation against the revision it was taken from
        // is fresh again.
        o.content_hash = Some(r.content_fingerprint());
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Fresh
        );
    }

    #[test]
    fn the_seller_repricing_is_not_published_as_a_contradiction() {
        // The owner republished at a new price. Our last reading was of the
        // previous revision, so it is not evidence about this one. Calling that
        // a conflict would flag every legitimate reprice as a disagreement for
        // as long as it takes the prober to come back.
        let r = resource(vec![option("exact", "20000", PAYEE)]);
        let mut o = observation(vec![option("exact", "10000", PAYEE)], 1_000);
        o.content_hash = Some("the fingerprint of the older revision".to_string());
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Stale
        );
    }

    #[test]
    fn an_aggregators_copy_moving_does_not_excuse_a_disagreement() {
        // Same shape, different author. A third party changing its copy is not
        // the seller speaking, and a direct reading of the origin outranks a
        // copy of it -- so this stays a conflict rather than becoming "the
        // listing was revised, our reading is old news".
        let mut r = resource(vec![option("exact", "20000", PAYEE)]);
        r.source = DiscoverySource::Aggregated;
        let mut o = observation(vec![option("exact", "10000", PAYEE)], 1_000);
        o.content_hash = Some("the fingerprint of the older copy".to_string());
        assert_eq!(
            assess_freshness(&r, Some(&o), 1_100, 3_600),
            PriceFreshness::Conflict
        );
    }

    #[test]
    fn a_settlement_observation_is_never_compared_against_an_upto_ceiling() {
        // upto: the listing declares a ceiling of 0.10 and the settlement
        // charged 0.03. Compatible by definition, and reporting it as a price
        // change is the false drift the annex names.
        let r = resource(vec![option("upto", "100000", PAYEE)]);
        let mut o = observation(vec![option("upto", "30000", PAYEE)], 1_000);
        o.phase = ObservationPhase::Settlement;
        assert!(!terms_disagree(&r.accepts, &o));
    }

    #[test]
    fn the_ladder_ranks_a_direct_reading_above_a_third_partys_copy() {
        assert!(TermsProvenance::OriginResponse.rank() > TermsProvenance::OwnerDeclared.rank());
        assert!(TermsProvenance::OwnerDeclared.rank() > TermsProvenance::OwnerDocument.rank());
        assert!(TermsProvenance::OwnerDocument.rank() > TermsProvenance::AggregatedFeed.rank());
    }

    #[tokio::test]
    async fn an_observation_survives_a_round_trip_through_the_overlay_format() {
        let overlay = TermsOverlay::new();
        let mut o = observation(vec![option("exact", "10000", PAYEE)], 1_700_000_000);
        o.rejected.insert("amount-negative".to_string(), 2);
        overlay.record("https://api.example.com/x", o.clone()).await;

        let records = overlay.snapshot().await;
        let encoded: HashMap<String, serde_json::Value> = records
            .iter()
            .map(|(u, t)| (u.clone(), serde_json::to_value(t).unwrap()))
            .collect();
        let body = serde_json::to_vec(&TermsSnapshot {
            version: TERMS_OVERLAY_VERSION,
            records: encoded,
        })
        .unwrap();

        let (loaded, version, dropped) = decode_overlay(&body);
        assert_eq!(version, TERMS_OVERLAY_VERSION);
        assert_eq!(dropped, 0);
        let back = loaded
            .get("https://api.example.com/x")
            .expect("record kept");
        assert_eq!(back.observed_at, 1_700_000_000);
        assert_eq!(back.context.method, "GET");
        assert_eq!(back.phase, ObservationPhase::Verification);
        assert_eq!(back.provenance, TermsProvenance::OriginResponse);
        assert_eq!(back.rejected.get("amount-negative"), Some(&2));
    }

    #[tokio::test]
    async fn observations_are_dropped_for_resources_that_left_the_catalog() {
        let overlay = TermsOverlay::new();
        for url in ["https://a.example/x", "https://gone.example/x"] {
            overlay
                .record(
                    url,
                    observation(vec![option("exact", "10000", PAYEE)], 1_000),
                )
                .await;
        }
        let live: std::collections::HashSet<String> =
            ["https://a.example/x".to_string()].into_iter().collect();
        assert_eq!(overlay.retain_urls(&live).await, 1);
        assert!(overlay.get("https://a.example/x").await.is_some());
        assert!(overlay.get("https://gone.example/x").await.is_none());
    }

    #[tokio::test]
    async fn an_unreadable_catalog_never_empties_the_observations() {
        // `main` falls back to an EMPTY registry when the S3 read fails and
        // keeps serving. Without this guard one transient GET would delete every
        // reading of every origin's live terms.
        let overlay = TermsOverlay::new();
        overlay
            .record(
                "https://a.example/x",
                observation(vec![option("exact", "10000", PAYEE)], 1_000),
            )
            .await;
        let nothing: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(overlay.retain_urls(&nothing).await, 0);
        assert!(overlay.get("https://a.example/x").await.is_some());
    }

    #[test]
    fn an_overlay_without_the_envelope_still_loads() {
        let o = observation(vec![option("exact", "10000", PAYEE)], 1_700_000_000);
        let bare = serde_json::json!({ "https://api.example.com/x": o });
        let (loaded, version, dropped) = decode_overlay(&serde_json::to_vec(&bare).unwrap());
        assert_eq!(version, 1, "a bare map reads as the first format version");
        assert_eq!(dropped, 0);
        assert!(loaded.contains_key("https://api.example.com/x"));
    }

    #[test]
    fn one_unreadable_record_costs_only_itself() {
        let good = observation(vec![option("exact", "10000", PAYEE)], 1_700_000_000);
        let body = serde_json::json!({
            "version": TERMS_OVERLAY_VERSION,
            "records": {
                "https://good.example/x": good,
                "https://bad.example/x": {"observedAt": "not a number"},
            }
        });
        let (loaded, _v, dropped) = decode_overlay(&serde_json::to_vec(&body).unwrap());
        assert_eq!(dropped, 1);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("https://good.example/x"));
    }

    #[tokio::test]
    async fn an_observation_is_bounded_before_it_is_stored() {
        let overlay = TermsOverlay::new();
        let many: Vec<CatalogPaymentOption> = (0..20)
            .map(|i| {
                let mut o = option("exact", "10000", PAYEE);
                o.extra = Some(serde_json::json!({"name": "USD Coin", "version": "2", "i": i}));
                o
            })
            .collect();
        overlay
            .record("https://api.example.com/x", observation(many, 1_000))
            .await;
        let stored = overlay.get("https://api.example.com/x").await.unwrap();
        assert_eq!(stored.accepts.len(), MAX_OBSERVED_OPTIONS);
        assert!(stored.truncated);
        assert!(
            stored.accepts.iter().all(|o| o.extra.is_none()),
            "the overlay dates and compares a price; it is not a second catalog"
        );
    }
}
