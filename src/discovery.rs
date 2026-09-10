//! Bazaar Discovery Registry for x402 v2.
//!
//! This module implements a persistent registry for discoverable paid API endpoints.
//! Resource providers can register their endpoints, and clients can query the registry
//! to find available paid services.
//!
//! # Resource Types
//!
//! The registry supports the following resource types:
//! - `http`: Standard HTTP API endpoints that accept x402 payments
//! - `mcp`: Model Context Protocol endpoints
//! - `a2a`: Agent-to-Agent protocol endpoints
//! - `facilitator`: x402 payment facilitator services (do not require payments themselves)
//!
//! # Architecture
//!
//! The registry uses a hybrid approach for fast reads with persistent storage:
//!
//! ```text
//! Client Request
//!       |
//!       v
//! In-Memory Cache (Arc<RwLock<HashMap>>) <-- Fast reads (~1ms)
//!       |
//!       v (on writes, async)
//! DiscoveryStore (S3/DynamoDB/Postgres) <-- Persistent storage
//! ```
//!
//! - Reads: Always from in-memory cache (fast, concurrent)
//! - Writes: Update cache immediately, persist to store asynchronously
//! - Startup: Load all resources from store into cache
//!
//! # Example
//!
//! ```rust,ignore
//! use x402_rs::discovery::DiscoveryRegistry;
//! use x402_rs::discovery_store::S3Store;
//! use x402_rs::types_v2::{DiscoveryResource, RegisterResourceRequest};
//!
//! // Create with S3 persistence
//! let store = S3Store::from_env().await?;
//! let registry = DiscoveryRegistry::with_store(store).await?;
//!
//! // Or create without persistence (in-memory only)
//! let registry = DiscoveryRegistry::new();
//!
//! // Register a resource (persisted automatically)
//! registry.register(resource).await?;
//!
//! // Query resources (from memory, fast)
//! let response = registry.list(10, 0, None).await;
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::discovery_revalidation::{NotVerifiable, RefreshReason, RevalidationState};
use crate::discovery_store::{DiscoveryStore, NoOpStore, StoreError, Version};
use crate::types_v2::{
    CurationInfo, DiscoveryFilters, DiscoveryResource, DiscoveryResponse, DiscoverySource,
    HealthState, HealthStatus, Pagination, Tier,
};

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during discovery operations.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Resource with this URL already exists
    #[error("Resource already registered: {0}")]
    AlreadyExists(String),

    /// Resource not found
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Invalid URL format
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Invalid resource type
    #[error("Invalid resource type: {0}. Expected: http, mcp, or a2a")]
    InvalidResourceType(String),

    /// No payment methods specified
    #[error("At least one payment method must be specified in 'accepts'")]
    NoPaymentMethods,

    /// Storage error
    #[error("Storage error: {0}")]
    StorageError(#[from] StoreError),
}

/// How a bulk import treats incoming resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPolicy {
    /// Full `validate_resource()` — used by `POST /discovery/register`.
    Strict,
    /// Apply the curation filter, silently dropping failures with per-rule
    /// counters — used by the aggregator and crawler.
    Filtered,
}

/// Clock skew allowed on feed-supplied `last_updated` before it is treated as
/// a future-timestamp poisoning attempt (F5).
const FUTURE_TIMESTAMP_SKEW_SECS: u64 = 300;

/// Most resources the in-memory catalog will hold.
///
/// # Why there is a number here at all
///
/// There was not one, and on 2026-09-10 that stopped being free. Fixing the
/// Coinbase feed parser (2.20.0) turned a source that had been failing entirely
/// into one that returns tens of thousands of resources: the aggregation cycle
/// went from `Total resources aggregated total=752` to `total=29133` inside the
/// same minute, and to 43 410 eighteen minutes later. The catalog followed --
/// 24 636 records and 14.5 MB in S3 at 16:48Z, **39 593 records and 98.5 MB at
/// 17:06Z** -- and every whole-catalog operation followed it: the resident
/// footprint, the snapshot the import clones, the scan `list()` does per
/// request, the URL set the health prober copies per tick.
///
/// Measured on that exact object, release build: **520 MB resident** for 39 593
/// records, 13.4 KB each. The task is provisioned with 2 GiB and one vCPU.
///
/// # The number, measured twice
///
/// 2.21.1 set this to 20 000 on an estimate of 13.4 KB resident per record. That
/// estimate was taken in a process that had also just parsed a 98 MB file, so it
/// mixed the live structures with the allocator arena the parse left behind, and
/// it was wrong in both directions at once. 20 000 did not restore the baseline:
/// production sat flat at **57 % of 2 GiB** and reads stayed at 2,7-5,3 s.
///
/// Measured properly -- one scenario per process, against objects that really
/// are that size, release build:
///
/// | records | object | resident |
/// |---:|---:|---:|
/// | 20 000 | 45 MB | 440 MB |
/// | 10 000 | 23 MB | 224 MB |
/// | 5 000 | 12 MB | 119 MB |
/// | **2 000** | **4,8 MB** | **54 MB** |
/// | 752 (the 2.19.0 baseline) | 1,8 MB | 25 MB |
///
/// Linear, at ~22 KB of process RSS per record. So 2 000 is ~54 MB, twice the
/// baseline this service ran on for months, and it leaves the whole 2 GiB for
/// everything else.
///
/// # Why trimming after the fact could not fix it
///
/// RSS is a high-water mark. Loading 20 000 records and then trimming to 2 000
/// measured 420 MB after the parse and **386 MB after the trim**: dropping 90 %
/// of the records returned 8 % of the memory, because the allocator keeps the
/// arena. Whatever peak a task reaches, it holds. That is why the memory was
/// FLAT at 57 % rather than settling, and why the fix has to be that the object
/// is small, not that we shrink it after reading it.
pub(crate) const DEFAULT_MAX_RESOURCES: usize = 2_000;

/// [`DEFAULT_MAX_RESOURCES`], overridable. `0` disables the cap.
fn max_resources() -> usize {
    crate::discovery_config::max_resources()
}

/// The `last_updated` an incoming aggregated record must beat to be worth
/// admitting, when the catalog is already full.
///
/// # Why admission has to be checked, and not just eviction
///
/// 2.21.1 admitted everything and evicted afterwards. That looks equivalent and
/// is not, because the feed republishes what we evicted. Measured over five
/// cycles of one unchanged page larger than the cap:
///
/// ```text
/// cycle 1: added=25 updated=0 skipped=0  held=10
/// cycle 2: added=15 updated=0 skipped=10 held=10
/// cycle 3: added=15 updated=0 skipped=10 held=10
/// ```
///
/// The catalog contents never change, and `added=15` forever -- the records the
/// cap dropped are no longer in the cache, so next cycle they arrive as new,
/// get inserted, and get dropped again. Three consequences, all paid every
/// cycle: the whole snapshot is serialized and uploaded although nothing
/// changed, `added` in the logs is permanently fiction, and -- the expensive one
/// -- a re-added record has no health record, so [`crate::discovery_health`]
/// treats it as never probed and probes it again. At ~9 000 re-added records per
/// cycle that is a self-inflicted probe storm, which is what the CPU bursts
/// every minute actually were.
///
/// Returns `None` when the catalog is below capacity and everything is welcome.
fn admission_threshold(cache: &HashMap<String, DiscoveryResource>, cap: usize) -> Option<u64> {
    if cap == 0 || cache.len() < cap {
        return None;
    }
    let mut dates: Vec<u64> = cache
        .values()
        .filter(|r| matches!(r.source, DiscoverySource::Aggregated))
        .map(|r| r.last_updated)
        .collect();
    if dates.is_empty() {
        // Nothing evictable: admitting more would only push us further over a
        // cap we already cannot enforce. Refuse everything new.
        return Some(u64::MAX);
    }
    dates.sort_unstable();
    // The oldest survivor: anything not newer than this would be evicted the
    // moment it was admitted.
    let first_kept = dates.len().saturating_sub(cap.min(dates.len()));
    Some(dates[first_kept])
}

/// Trim `cache` to `cap`, dropping the least defensible records first.
///
/// Eviction is by PROVENANCE before recency, and that order is the whole point.
/// A resource somebody registered with us, or that we watched a payment settle
/// for, or that we read from the origin's own document, is first-hand and
/// irreplaceable: we cannot get it back by asking a third party. An aggregated
/// copy is, by construction, a copy of something still published elsewhere --
/// dropping it costs a re-fetch, and the next cycle will offer it again.
///
/// Within the aggregated tier, the oldest `last_updated` goes first: that is the
/// registry's own write clock, so "least recently touched by us" is exactly the
/// record whose absence we are least likely to notice.
///
/// # Why this names `Aggregated` instead of asking [`provenance_rank`]
///
/// They are the same ladder and they must stay in step, but they answer
/// different questions. `provenance_rank` orders authorities so a merge can pick
/// a winner; this one asks something narrower -- *is this record replaceable* --
/// and only the bottom rung is. Evicting "whatever ranks lowest" would start
/// deleting crawled records the moment a catalog held no aggregated ones, which
/// is precisely the first-hand data the rule exists to protect. If a rung is
/// ever added between them, this is the second place to look.
///
/// Returns how many were dropped.
fn enforce_capacity(cache: &mut HashMap<String, DiscoveryResource>, cap: usize) -> usize {
    if cap == 0 || cache.len() <= cap {
        return 0;
    }
    let mut evictable: Vec<(String, u64)> = cache
        .iter()
        .filter(|(_, r)| matches!(r.source, DiscoverySource::Aggregated))
        .map(|(url, r)| (url.clone(), r.last_updated))
        .collect();
    // Oldest first.
    evictable.sort_by_key(|(_, last_updated)| *last_updated);

    let over = cache.len() - cap;
    let mut dropped = 0;
    for (url, _) in evictable.into_iter().take(over) {
        cache.remove(&url);
        dropped += 1;
    }
    if dropped < over {
        // Every remaining record is first-hand. Refusing to evict those is
        // deliberate: going over the cap is a capacity problem with a known
        // answer (a bigger task), and silently deleting the only copy of
        // somebody's listing to stay under a number is not it.
        warn!(
            held = cache.len(),
            cap = cap,
            first_hand = cache.len() - cap,
            "catalog is over capacity and everything left is first-hand; not evicting further"
        );
    }
    dropped
}

/// What an import should do with a record it collides with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportVerdict {
    /// The incoming record wins; write it.
    Replace,
    /// Same offer, same claimed date. Not a change, so not a write.
    Unchanged,
    /// The held record wins; drop the incoming one.
    Keep,
}

/// Where a record's terms came from, as a rank. Higher wins.
///
/// The ladder the annex specifies is *direct observation > verified owner
/// declaration > aggregated feed*, and it exists because a timestamp is not
/// authority. A feed that stamps its copy with today's date has written a date,
/// not learned a price.
///
/// `SelfRegistered` and `Settlement` share a rank: both are first-hand, and no
/// path feeds a settlement-sourced record into an import, so ordering them
/// against each other would be a rule with no caller.
fn provenance_rank(source: DiscoverySource) -> u8 {
    match source {
        // The owner told us directly, or we watched a payment for it settle.
        DiscoverySource::SelfRegistered | DiscoverySource::Settlement => 3,
        // The origin's own well-known document, fetched by us.
        DiscoverySource::Crawled => 2,
        // Somebody else's copy of somebody else's listing.
        DiscoverySource::Aggregated => 1,
    }
}

/// Whether an incoming import should replace the record already held.
///
/// # Why this is not `incoming.last_updated > existing.last_updated`
///
/// It used to be, and the aggregator stamped `now` on any feed entry that
/// carried no date of its own. The two together meant re-downloading unchanged,
/// months-old content was enough to outrank a record that carried a real date --
/// the fetch itself manufactured the evidence of freshness (F6).
///
/// # The order the three rules are applied in
///
/// **1. Same content, same claimed date: nothing happened.** Comparing
/// [`DiscoveryResource::content_fingerprint`] is what separates "this feed
/// republished its page" from "the terms changed". Without it every cycle of an
/// unchanged upstream counts as an update and rewrites a 15 MB snapshot to say
/// the same thing.
///
/// **2. Different authorities are not ordered by the clock.** A third party's
/// copy does not supersede the owner's own declaration because the copy carries
/// a newer date -- the date is the copier's, and it is a statement about when
/// they copied. This is the rule that was missing: every record stored before
/// this phase has no `sourceUpdatedAt` at all (0 of 24 636 in the production
/// snapshot on 2026-09-10), so *every* dated feed entry outranked *every*
/// self-registered listing, purely on a field one side did not have.
///
/// **3. Within one authority, the source's own claim orders the versions.**
/// Five cases, and only the first is a comparison:
///
/// | incoming | existing | verdict |
/// |---|---|---|
/// | dated | dated, older | the newer claim wins |
/// | dated | dated, newer | no |
/// | dated | undated | a dated claim beats an undated record |
/// | undated | dated | **no** -- this is the case that used to invert |
/// | no date between them | | the publisher of the two decides |
///
/// The last row is the one the content hash made answerable. Two entries the
/// dates cannot separate, and the content differs: if the SAME publisher sent
/// both, this is that publisher revising its own entry without moving its own
/// clock, and it is the authority on its own listing, so it lands. If two
/// different publishers of equal rank disagree with no date between them,
/// nothing here can rank them -- and taking whichever was fetched last would
/// make the record flip on every cycle, in crawl order, forever. It stays put.
///
/// Our ingestion clock decides nothing at any step.
fn import_verdict(incoming: &DiscoveryResource, existing: &DiscoveryResource) -> ImportVerdict {
    if incoming.source_updated_at == existing.source_updated_at
        && incoming.content_fingerprint() == existing.content_fingerprint()
    {
        return ImportVerdict::Unchanged;
    }

    let (incoming_rank, existing_rank) = (
        provenance_rank(incoming.source),
        provenance_rank(existing.source),
    );
    if incoming_rank != existing_rank {
        return if incoming_rank > existing_rank {
            ImportVerdict::Replace
        } else {
            ImportVerdict::Keep
        };
    }

    match (incoming.source_updated_at, existing.source_updated_at) {
        (Some(i), Some(e)) if i > e => ImportVerdict::Replace,
        (Some(i), Some(e)) if i < e => ImportVerdict::Keep,
        (Some(_), None) => ImportVerdict::Replace,
        (None, Some(_)) => ImportVerdict::Keep,
        // Equal dates, or no dates at all.
        _ => {
            if incoming.source_facilitator == existing.source_facilitator {
                ImportVerdict::Replace
            } else {
                ImportVerdict::Keep
            }
        }
    }
}

/// Why this resource's price cannot be established by probing it.
///
/// The prober issues one kind of request: an unauthenticated `GET` of the
/// listing URL. Anything else is a different purchase, and the annex is explicit
/// that we do not fire a seller's real commercial operation to find out what it
/// charges. So a resource that cannot be answered that way is reported as
/// unverifiable rather than queued forever or, worse, probed anyway.
fn not_verifiable_reason(r: &DiscoveryResource) -> Option<NotVerifiable> {
    // A URL template is not an address. Braces in a path are a declaration that
    // the real request is built from parameters we do not have.
    //
    // Both spellings, because `Url::parse` percent-encodes them: a feed that
    // published `/item/{id}` reaches us as `/item/%7Bid%7D`, and checking only
    // for the literal brace would have found none of them.
    let url = r.url.as_str();
    let templated = ['{', '}'].iter().any(|c| url.contains(*c))
        || url.contains("%7B")
        || url.contains("%7b")
        || url.contains("%7D")
        || url.contains("%7d");
    if templated {
        return Some(NotVerifiable::Unprobeable);
    }
    // MCP and A2A endpoints answer a handshake, not a payment challenge. They
    // are probed for liveness, and their price is not observable that way.
    if r.resource_type == "mcp" || r.resource_type == "a2a" {
        return Some(NotVerifiable::NotAGetResource);
    }
    None
}

/// Current Unix time in seconds (0 if the clock is before the epoch).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `list()` visibility predicate: default hides `quarantined`; `health=any`
/// shows everything; `health=<status>` filters to that exact status.
fn health_visible(health: &HashMap<String, HealthState>, url: &str, filter: Option<&str>) -> bool {
    let status = health
        .get(url)
        .map(|h| h.status)
        .unwrap_or(HealthStatus::Unknown);
    match filter {
        Some(f) if f.eq_ignore_ascii_case("any") => true,
        Some(f) => health_status_label(status).eq_ignore_ascii_case(f),
        None => status != HealthStatus::Quarantined,
    }
}

fn health_status_label(s: HealthStatus) -> &'static str {
    match s {
        HealthStatus::Unknown => "unknown",
        HealthStatus::Alive => "alive",
        HealthStatus::Degraded => "degraded",
        HealthStatus::AuthGated => "auth_gated",
        HealthStatus::Quarantined => "quarantined",
        HealthStatus::Unprobeable => "unprobeable",
    }
}

/// Secondary sort key: liveness rank (alive first).
fn health_rank(health: &HashMap<String, HealthState>, url: &str) -> u8 {
    match health
        .get(url)
        .map(|h| h.status)
        .unwrap_or(HealthStatus::Unknown)
    {
        HealthStatus::Alive => 0,
        HealthStatus::AuthGated => 1,
        HealthStatus::Degraded => 2,
        HealthStatus::Unknown => 3,
        HealthStatus::Unprobeable => 4,
        HealthStatus::Quarantined => 5,
    }
}

fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::FirstParty => "first_party",
        Tier::Vip => "vip",
        Tier::Verified => "verified",
        Tier::Listed => "listed",
    }
}

/// `tier=` filter predicate. Resources with no curation info are `listed`.
fn tier_matches(cur: &Option<CurationInfo>, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => {
            let label = cur.as_ref().map(|c| tier_label(c.tier)).unwrap_or("listed");
            label.eq_ignore_ascii_case(f)
        }
    }
}

// ============================================================================
// Discovery Registry
// ============================================================================

/// Persistent registry for discoverable paid resources.
///
/// Uses in-memory cache for fast reads with optional persistent storage
/// for durability across restarts.
///
/// Thread-safe using `Arc<RwLock>` for concurrent read access with
/// exclusive write access during registration.
pub struct DiscoveryRegistry {
    /// In-memory cache: Map of URL -> DiscoveryResource
    resources: Arc<RwLock<HashMap<String, DiscoveryResource>>>,
    /// Persistent storage backend
    store: Arc<dyn DiscoveryStore>,
    /// Liveness overlay (WS-B health prober).
    health: Arc<crate::discovery_health::HealthTracker>,
    /// Observed payment terms overlay. Separate object, one writer (the
    /// prober), and therefore structurally out of reach of any import.
    terms: Arc<crate::discovery_terms::TermsOverlay>,
    /// Demand-driven revalidation queue (P2). Every replica holds one; only the
    /// job owner drains it, and non-owners hand their requests over through it.
    revalidation: Arc<crate::discovery_revalidation::RevalidationQueue>,
    /// Curated tier manifest (WS-C).
    curation: Arc<crate::discovery_curation::CurationManifest>,
    /// On-chain reputation cache (WS-E), keyed by resource URL.
    reputation: Arc<RwLock<HashMap<String, crate::types_v2::VerificationInfo>>>,
    /// Hosted attestation evidence bodies (WS-E), keyed by sha256(url) hex.
    evidence: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// `GET /discovery/stats` cache: `(computed_at_unix, payload)`.
    stats_cache: Arc<RwLock<Option<(u64, serde_json::Value)>>>,
    /// Runtime suppression set (admin API), keyed by canonical URL. Additive to
    /// the manifest's static `suppressed[]` list.
    suppressed: Arc<RwLock<std::collections::HashSet<String>>>,
    /// Catalog writes waiting to be applied, in the order they were issued.
    writes: Arc<WriteQueue>,
    /// Version of the stored catalog this cache was last built from.
    ///
    /// Only the replicas that do NOT own the discovery jobs read it: it is how
    /// they tell "the owner published something" from "nothing moved" without
    /// re-reading a 15 MB object every minute.
    cached_version: Arc<RwLock<Version>>,
}

/// One pending catalog write.
#[derive(Debug)]
enum StoreOp {
    Save(Box<DiscoveryResource>),
    Delete(String),
}

/// Fire-and-forget catalog writes, applied in the ORDER THEY WERE ISSUED.
///
/// Persistence used to be a bare `tokio::spawn` per operation, so the order in
/// which a save and a delete reached the store was up to the scheduler. Register
/// then unregister could arrive the other way round, and the save — a
/// read-modify-write against whatever it found — would put the deleted resource
/// back. Conditional writes do not fix that: both writes are perfectly valid
/// against the base each one read.
///
/// So the queue is filled SYNCHRONOUSLY, in the same order the in-memory cache
/// was mutated, and drained by at most one task at a time.
///
/// This orders one process. Between the three ECS tasks the ordering is the
/// store's conditional write, which prevents a lost update but not a
/// cross-process resurrection; that needs a per-resource record with its own
/// conditional update, which is the evolution the audit describes and this
/// change deliberately does not attempt.
#[derive(Debug, Default)]
struct WriteQueue {
    pending: std::sync::Mutex<std::collections::VecDeque<StoreOp>>,
    /// Held for the whole drain, so two drainers cannot interleave and undo the
    /// ordering the queue exists to provide.
    draining: RwLock<()>,
}

impl WriteQueue {
    /// Enqueue, in call order. Synchronous and non-blocking on purpose: an
    /// `await` here would be a point at which two callers could swap places.
    fn push(&self, op: StoreOp) {
        match self.pending.lock() {
            Ok(mut queue) => queue.push_back(op),
            // A poisoned lock means a previous holder panicked while holding
            // it. Dropping the write is the safe answer -- the in-memory cache
            // is still correct and the next aggregation snapshot re-publishes
            // it -- and it is strictly better than panicking a request path.
            Err(_) => warn!("catalog write queue is poisoned; dropping one persistence op"),
        }
    }

    fn pop(&self) -> Option<StoreOp> {
        self.pending.lock().ok().and_then(|mut q| q.pop_front())
    }

    /// How many writes are still waiting to reach the store.
    ///
    /// A poisoned lock answers "one", not "none": the caller uses this to
    /// decide whether replacing the in-memory cache from the store is safe, and
    /// a lock nobody can read is not an answer that should authorise that.
    fn pending(&self) -> usize {
        self.pending.lock().map(|q| q.len()).unwrap_or(1)
    }

    /// Apply everything queued, one at a time, in order.
    async fn drain(&self, store: &Arc<dyn DiscoveryStore>) {
        let _one_at_a_time = self.draining.write().await;
        while let Some(op) = self.pop() {
            let result = match &op {
                StoreOp::Save(resource) => store.save(resource).await,
                StoreOp::Delete(url) => store.delete(url).await,
            };
            if let Err(e) = result {
                let what = match &op {
                    StoreOp::Save(resource) => resource.url.to_string(),
                    StoreOp::Delete(url) => url.clone(),
                };
                match e {
                    // The catalog was not damaged: every attempt was refused,
                    // none half-applied. Worth a warning rather than an error,
                    // and distinguishable in the logs from a store that is down.
                    StoreError::VersionConflict(ref detail) => warn!(
                        url = %what,
                        detail = %detail,
                        "catalog write lost every conditional attempt; the catalog is intact and \
                         the in-memory cache still holds this resource"
                    ),
                    _ => error!(url = %what, error = %e, "Failed to persist catalog write"),
                }
            }
        }
    }
}

impl Clone for DiscoveryRegistry {
    fn clone(&self) -> Self {
        Self {
            resources: Arc::clone(&self.resources),
            store: Arc::clone(&self.store),
            health: Arc::clone(&self.health),
            terms: Arc::clone(&self.terms),
            revalidation: Arc::clone(&self.revalidation),
            curation: Arc::clone(&self.curation),
            reputation: Arc::clone(&self.reputation),
            evidence: Arc::clone(&self.evidence),
            stats_cache: Arc::clone(&self.stats_cache),
            suppressed: Arc::clone(&self.suppressed),
            writes: Arc::clone(&self.writes),
            cached_version: Arc::clone(&self.cached_version),
        }
    }
}

impl Default for DiscoveryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryRegistry {
    /// Create a new empty discovery registry without persistence.
    ///
    /// Use `with_store()` for persistent storage.
    pub fn new() -> Self {
        info!("Initializing Bazaar discovery registry (no persistence)");
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
            store: Arc::new(NoOpStore::new()),
            health: Arc::new(crate::discovery_health::HealthTracker::new()),
            terms: Arc::new(crate::discovery_terms::TermsOverlay::new()),
            revalidation: Arc::new(crate::discovery_revalidation::RevalidationQueue::new()),
            curation: Arc::new(crate::discovery_curation::CurationManifest::load()),
            reputation: Arc::new(RwLock::new(HashMap::new())),
            evidence: Arc::new(RwLock::new(HashMap::new())),
            stats_cache: Arc::new(RwLock::new(None)),
            suppressed: Arc::new(RwLock::new(std::collections::HashSet::new())),
            writes: Arc::new(WriteQueue::default()),
            cached_version: Arc::new(RwLock::new(Version::Absent)),
        }
    }

    /// The liveness overlay (WS-B). Used by the health prober and by `list()`.
    pub fn health(&self) -> Arc<crate::discovery_health::HealthTracker> {
        Arc::clone(&self.health)
    }

    /// The observed-terms overlay. Used by the health prober (the only writer)
    /// and by `list()` to annotate freshness.
    pub fn terms(&self) -> Arc<crate::discovery_terms::TermsOverlay> {
        Arc::clone(&self.terms)
    }

    /// The demand-driven revalidation queue (P2).
    pub fn revalidation(&self) -> Arc<crate::discovery_revalidation::RevalidationQueue> {
        Arc::clone(&self.revalidation)
    }

    /// The curation manifest (WS-C). Used to build attestation targets.
    pub fn curation(&self) -> Arc<crate::discovery_curation::CurationManifest> {
        Arc::clone(&self.curation)
    }

    /// The on-chain reputation cache (WS-E), for the attestation task to fill.
    pub fn reputation(&self) -> Arc<RwLock<HashMap<String, crate::types_v2::VerificationInfo>>> {
        Arc::clone(&self.reputation)
    }

    /// The hosted attestation evidence store (WS-E).
    pub fn evidence(&self) -> Arc<RwLock<HashMap<String, Vec<u8>>>> {
        Arc::clone(&self.evidence)
    }

    /// Serve a hosted evidence body by its sha256(url) hex key.
    pub async fn get_evidence(&self, key: &str) -> Option<Vec<u8>> {
        self.evidence.read().await.get(key).cloned()
    }

    /// Normalize a URL to the key used by the suppression set. Falls back to a
    /// trimmed lowercase form when the URL does not parse.
    fn suppression_key(url: &str) -> String {
        match crate::discovery_security::canonical_url(url) {
            Ok(c) => c.key,
            Err(_) => url.trim().to_ascii_lowercase(),
        }
    }

    /// Hide a resource from every listing without deleting it (admin API).
    /// Returns `true` if it was not already suppressed.
    pub async fn suppress(&self, url: &str) -> bool {
        let key = Self::suppression_key(url);
        let added = self.suppressed.write().await.insert(key.clone());
        if added {
            info!(url = %key, "Resource suppressed by admin");
            self.invalidate_stats().await;
        }
        added
    }

    /// Un-suppress a resource (admin API). Returns `true` if it was suppressed.
    pub async fn release(&self, url: &str) -> bool {
        let key = Self::suppression_key(url);
        let removed = self.suppressed.write().await.remove(&key);
        if removed {
            info!(url = %key, "Resource released by admin");
            self.invalidate_stats().await;
        }
        removed
    }

    /// Snapshot of runtime-suppressed keys (taken before the resources guard).
    async fn suppressed_snapshot(&self) -> std::collections::HashSet<String> {
        self.suppressed.read().await.clone()
    }

    async fn invalidate_stats(&self) {
        *self.stats_cache.write().await = None;
    }

    /// Aggregate catalog metrics for `GET /discovery/stats`, served from a
    /// 60-second in-process cache: the computation is a full pass over the
    /// catalog, and this route is public and unauthenticated.
    pub async fn stats(&self) -> serde_json::Value {
        const TTL_SECS: u64 = 60;
        let now = now_secs();

        if let Some((at, payload)) = self.stats_cache.read().await.as_ref() {
            if now.saturating_sub(*at) < TTL_SECS {
                return payload.clone();
            }
        }

        // Snapshot the overlays before taking the resources guard (no awaits
        // while it is held).
        let health = self.health.snapshot().await;
        let suppressed = self.suppressed_snapshot().await;

        let mut by_source: HashMap<String, u64> = HashMap::new();
        let mut by_facilitator: HashMap<String, u64> = HashMap::new();
        let mut by_network: HashMap<String, u64> = HashMap::new();
        let mut by_tier: HashMap<String, u64> = HashMap::new();
        let mut by_health: HashMap<String, u64> = HashMap::new();
        let (mut total, mut visible) = (0u64, 0u64);

        {
            let resources = self.resources.read().await;
            for r in resources.values() {
                if self.curation.is_suppressed(&r.url)
                    || suppressed.contains(&Self::suppression_key(r.url.as_str()))
                {
                    continue;
                }
                total += 1;

                let status = health
                    .get(r.url.as_str())
                    .map(|h| h.status)
                    .unwrap_or(HealthStatus::Unknown);
                *by_health
                    .entry(health_status_label(status).to_string())
                    .or_insert(0) += 1;
                if status != HealthStatus::Quarantined {
                    visible += 1;
                }

                *by_source.entry(r.source.to_string()).or_insert(0) += 1;
                if let Some(sf) = r.source_facilitator.as_ref() {
                    *by_facilitator.entry(sf.clone()).or_insert(0) += 1;
                }
                for a in &r.accepts {
                    *by_network.entry(a.network.to_string()).or_insert(0) += 1;
                }

                let tier = self
                    .curation
                    .resolve(&r.url, status == HealthStatus::Alive)
                    .map(|c| tier_label(c.tier))
                    .unwrap_or("listed");
                *by_tier.entry(tier.to_string()).or_insert(0) += 1;
            }
        }

        let payload = serde_json::json!({
            "total": total,
            "visible": visible,
            "bySource": by_source,
            "bySourceFacilitator": by_facilitator,
            "byNetwork": by_network,
            "byTier": by_tier,
            "byHealth": by_health,
            "generatedAt": now,
        });
        *self.stats_cache.write().await = Some((now, payload.clone()));
        payload
    }

    /// Snapshot of every registered resource URL (for the health prober).
    pub async fn all_urls(&self) -> Vec<url::Url> {
        self.resources
            .read()
            .await
            .values()
            .map(|r| r.url.clone())
            .collect()
    }

    /// Snapshot of probe targets: `(url, resource_type, expected_pay_to)`.
    /// `expected_pay_to` is the set of recipients currently listed for the
    /// resource, so the prober can detect a payTo swap in the live 402 body.
    pub async fn probe_targets(&self) -> Vec<(url::Url, String, Vec<String>)> {
        self.resources
            .read()
            .await
            .values()
            .map(|r| {
                (
                    r.url.clone(),
                    r.resource_type.clone(),
                    r.accepts
                        .iter()
                        .map(|a| a.pay_to.to_string().to_ascii_lowercase())
                        .collect(),
                )
            })
            .collect()
    }

    /// Create a new discovery registry with persistent storage.
    ///
    /// Loads existing resources from the store on creation.
    pub async fn with_store<S: DiscoveryStore + 'static>(store: S) -> Result<Self, DiscoveryError> {
        let store_type = store.store_type();
        info!(
            store_type = store_type,
            "Initializing Bazaar discovery registry with persistence"
        );

        // Load existing resources from store, keeping the version they came at
        // so a follower can tell later whether the catalog moved.
        let snapshot = store.load_snapshot().await?;
        let count = snapshot.resources.len();

        // Populate cache
        let mut cache = HashMap::new();
        for resource in snapshot.resources {
            cache.insert(resource.url.to_string(), resource);
        }

        // Trim on the way in. A snapshot written before the cap existed is
        // bigger than the task can carry, and it is the FIRST thing a task
        // touches -- so the cap has to apply here and not only at import, or
        // every restart re-inhales the whole object and the fix never arrives.
        // This is also what makes the oversized object in S3 safe to deploy
        // against: the next snapshot this process writes is already trimmed.
        let dropped = enforce_capacity(&mut cache, max_resources());
        if dropped > 0 {
            warn!(
                store_type = store_type,
                loaded = count,
                dropped = dropped,
                held = cache.len(),
                "catalog loaded over capacity; trimmed the oldest aggregated copies"
            );
        }

        info!(
            store_type = store_type,
            loaded_count = count,
            held = cache.len(),
            "Loaded discovery resources from persistent storage"
        );

        Ok(Self {
            resources: Arc::new(RwLock::new(cache)),
            store: Arc::new(store),
            health: Arc::new(crate::discovery_health::HealthTracker::new()),
            terms: Arc::new(crate::discovery_terms::TermsOverlay::new()),
            revalidation: Arc::new(crate::discovery_revalidation::RevalidationQueue::new()),
            curation: Arc::new(crate::discovery_curation::CurationManifest::load()),
            reputation: Arc::new(RwLock::new(HashMap::new())),
            evidence: Arc::new(RwLock::new(HashMap::new())),
            stats_cache: Arc::new(RwLock::new(None)),
            suppressed: Arc::new(RwLock::new(std::collections::HashSet::new())),
            writes: Arc::new(WriteQueue::default()),
            cached_version: Arc::new(RwLock::new(snapshot.version)),
        })
    }

    /// Reload the catalog from the store, if the store moved.
    ///
    /// The read half of single ownership (A4). One replica runs the periodic
    /// jobs and publishes the catalog; every other replica follows it with this,
    /// so `/discovery/*` stays as fresh on all three as it was when all three
    /// aggregated — fresher, in fact, since a replica used to be as stale as its
    /// own last hourly cycle.
    ///
    /// Returns `Ok(Some(count))` when the cache was replaced, `Ok(None)` when
    /// there was nothing to do.
    ///
    /// # Why it refuses while local writes are queued
    ///
    /// A `POST /discovery/register` mutates the cache immediately and persists
    /// through [`WriteQueue`], which drains off the caller's path. Replacing the
    /// cache from the store in that window would drop the new resource from
    /// memory while its write is still in flight, and the caller would have had
    /// a 200 for a registration that vanished. So a pending queue means "not
    /// now"; the next refresh picks it up.
    pub async fn refresh_from_store(&self) -> Result<Option<usize>, StoreError> {
        if self.writes.pending() > 0 {
            return Ok(None);
        }

        let latest = self.store.current_version().await?;
        if latest == *self.cached_version.read().await {
            return Ok(None);
        }

        let snapshot = self.store.load_snapshot().await?;

        // Asked again: a registration can arrive while the object is in flight,
        // and the read that started before it would erase it.
        if self.writes.pending() > 0 {
            return Ok(None);
        }

        let loaded = snapshot.resources.len();
        let mut fresh: HashMap<String, DiscoveryResource> = snapshot
            .resources
            .into_iter()
            .map(|r| (r.url.to_string(), r))
            .collect();

        // Trim on the way in, for the same reason [`Self::with_store`] does:
        // this is a load of the whole object, and an object written before the
        // cap existed — or by a task running an older image — is bigger than
        // this one can carry. Without this a follower re-inhales the oversized
        // catalog every time it moves, which is precisely the memory the 2.21.2
        // cap exists to bound.
        let dropped = enforce_capacity(&mut fresh, max_resources());
        if dropped > 0 {
            warn!(
                loaded = loaded,
                dropped = dropped,
                held = fresh.len(),
                "refreshed catalog was over capacity; trimmed the oldest aggregated copies"
            );
        }

        let count = fresh.len();
        let mut cache = self.resources.write().await;
        *cache = fresh;
        drop(cache);

        *self.cached_version.write().await = snapshot.version;
        // Everything derived from the catalog is now stale.
        *self.stats_cache.write().await = None;

        Ok(Some(count))
    }

    /// Get the store type for diagnostics.
    pub fn store_type(&self) -> &'static str {
        self.store.store_type()
    }

    /// Persist a resource to the store, off the caller's path.
    ///
    /// Enqueued synchronously so the write lands AFTER anything issued before
    /// it and BEFORE anything issued after — see [`WriteQueue`] for the delete
    /// that used to come back to life without it.
    fn persist_async(&self, resource: DiscoveryResource) {
        self.writes.push(StoreOp::Save(Box::new(resource)));
        self.drain_writes();
    }

    /// Delete a resource from the store, off the caller's path.
    fn delete_from_store_async(&self, url: String) {
        self.writes.push(StoreOp::Delete(url));
        self.drain_writes();
    }

    /// Kick the drainer. Cheap when one is already running: the second task
    /// blocks on the drain lock, finds the queue empty and returns.
    fn drain_writes(&self) {
        let store = Arc::clone(&self.store);
        let writes = Arc::clone(&self.writes);
        tokio::spawn(async move { writes.drain(&store).await });
    }

    /// Register a new resource in the registry.
    ///
    /// The resource is immediately added to the in-memory cache and
    /// persisted to storage asynchronously.
    ///
    /// # Errors
    ///
    /// Returns `DiscoveryError::AlreadyExists` if a resource with the same URL
    /// is already registered. Use `update` to modify existing resources.
    pub async fn register(&self, resource: DiscoveryResource) -> Result<(), DiscoveryError> {
        // Validate resource
        self.validate_resource(&resource)?;
        let mut resource = resource;
        resource.strip_response_only();

        let url_key = resource.url.to_string();

        let mut resources = self.resources.write().await;

        if resources.contains_key(&url_key) {
            warn!(url = %url_key, "Attempted to register duplicate resource");
            return Err(DiscoveryError::AlreadyExists(url_key));
        }

        info!(
            url = %url_key,
            resource_type = %resource.resource_type,
            accepts_count = resource.accepts.len(),
            store_type = self.store.store_type(),
            "Registered new resource in discovery registry"
        );

        // Clone for persistence before moving into cache
        let resource_for_store = resource.clone();
        resources.insert(url_key, resource);

        // Release lock before async persistence
        drop(resources);

        // Persist asynchronously
        self.persist_async(resource_for_store);

        Ok(())
    }

    /// Update an existing resource in the registry.
    ///
    /// If the resource doesn't exist, it will be created (upsert behavior).
    /// The update is immediately applied to cache and persisted asynchronously.
    pub async fn update(&self, resource: DiscoveryResource) -> Result<(), DiscoveryError> {
        self.validate_resource(&resource)?;
        let mut resource = resource;
        resource.strip_response_only();

        let url_key = resource.url.to_string();

        let mut resources = self.resources.write().await;
        let existed = resources.contains_key(&url_key);

        // Clone for persistence
        let resource_for_store = resource.clone();
        resources.insert(url_key.clone(), resource);

        if existed {
            debug!(url = %url_key, "Updated existing resource in registry");
        } else {
            info!(url = %url_key, "Created new resource via update (upsert)");
        }

        // Release lock before async persistence
        drop(resources);

        // Persist asynchronously
        self.persist_async(resource_for_store);

        Ok(())
    }

    /// Remove a resource from the registry.
    ///
    /// The resource is immediately removed from cache and deleted from
    /// storage asynchronously.
    ///
    /// # Errors
    ///
    /// Returns `DiscoveryError::NotFound` if no resource with the given URL exists.
    pub async fn unregister(&self, url: &str) -> Result<DiscoveryResource, DiscoveryError> {
        let mut resources = self.resources.write().await;

        match resources.remove(url) {
            Some(resource) => {
                info!(url = %url, "Unregistered resource from discovery registry");

                // Release lock before async deletion
                drop(resources);

                // Delete from store asynchronously
                self.delete_from_store_async(url.to_string());

                Ok(resource)
            }
            None => {
                warn!(url = %url, "Attempted to unregister non-existent resource");
                Err(DiscoveryError::NotFound(url.to_string()))
            }
        }
    }

    /// Get a specific resource by URL.
    pub async fn get(&self, url: &str) -> Option<DiscoveryResource> {
        let resources = self.resources.read().await;
        resources.get(url).cloned()
    }

    /// List resources with pagination and optional filtering.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of resources to return (capped at 100)
    /// * `offset` - Number of resources to skip
    /// * `filters` - Optional filters for category, network, provider, or tag
    pub async fn list(
        &self,
        limit: u32,
        offset: u32,
        filters: Option<DiscoveryFilters>,
    ) -> DiscoveryResponse {
        // Snapshot the health overlay BEFORE taking the resources read guard —
        // the tracker is behind its own async lock, and holding the resources
        // guard across its `.await` is the guard-across-await hazard.
        let health = self.health.snapshot().await;
        // Same reason, one overlay along: the observed-terms records are behind
        // their own async lock, so they are read BEFORE the resources guard.
        let observed = self.terms.snapshot().await;
        // And the queue, for the same reason: one snapshot instead of a lock
        // acquisition per item.
        let queued = self.revalidation.pending_snapshot().await;
        let mut stale_seen: Vec<(String, RefreshReason)> = Vec::new();
        let freshness_window = crate::discovery_terms::freshness_window_secs();
        let now = now_secs();
        let reputation = self.reputation.read().await.clone();
        let suppressed = self.suppressed_snapshot().await;
        let health_filter = filters.as_ref().and_then(|f| f.health.clone());
        let tier_filter = filters.as_ref().and_then(|f| f.tier.clone());
        // Normalize the search needle once (matching is a substring scan).
        let filters = filters.map(|mut f| {
            f.q = f.q.map(|q| q.trim().to_ascii_lowercase());
            f
        });

        let resources = self.resources.read().await;

        // Cap limit at 100 to prevent abuse
        let limit = limit.min(100);

        // Filter (user filters + suppression + health visibility), then resolve
        // each survivor's curated tier for ordering + annotation.
        let mut scored: Vec<(&DiscoveryResource, Option<CurationInfo>)> = resources
            .values()
            .filter(|r| self.matches_filters(r, &filters))
            .filter(|r| {
                !self.curation.is_suppressed(&r.url)
                    && !suppressed.contains(&Self::suppression_key(r.url.as_str()))
            })
            .filter(|r| health_visible(&health, r.url.as_str(), health_filter.as_deref()))
            .map(|r| {
                let alive = health
                    .get(r.url.as_str())
                    .map(|h| h.status == HealthStatus::Alive)
                    .unwrap_or(false);
                let mut cur = self.curation.resolve(&r.url, alive);
                if let Some(c) = cur.as_mut() {
                    // The verification cache is keyed by manifest label, so the
                    // annotation joins regardless of URL variants.
                    if let Some(label) = c.label.as_deref() {
                        c.verification = reputation.get(label).cloned();
                    }
                }
                (r, cur)
            })
            .filter(|(_, cur)| tier_matches(cur, tier_filter.as_deref()))
            .collect();

        // Order: curated tier (first_party > vip > verified > listed), then
        // liveness (alive first), then last_updated descending.
        scored.sort_by(|(a, ca), (b, cb)| {
            let ta = ca
                .as_ref()
                .map(|c| c.tier.rank())
                .unwrap_or(Tier::Listed.rank());
            let tb = cb
                .as_ref()
                .map(|c| c.tier.rank())
                .unwrap_or(Tier::Listed.rank());
            ta.cmp(&tb)
                .then_with(|| {
                    health_rank(&health, a.url.as_str()).cmp(&health_rank(&health, b.url.as_str()))
                })
                .then_with(|| b.last_updated.cmp(&a.last_updated))
        });

        let total = scored.len() as u32;

        // Apply pagination, annotating each returned item with its health +
        // curation (response-only; the cached/persisted copy stays clean).
        let items: Vec<DiscoveryResource> = scored
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(r, cur)| {
                let mut c = r.clone();
                c.health = health.get(r.url.as_str()).cloned();
                c.curation = cur;
                // Price semantics are resolved here, on the response copy only,
                // for the same reason health and curation are: settleability and
                // a token's decimals are properties of THIS build and the
                // current deployment table, not of the record. Persisting them
                // would let a listing keep claiming six decimals for an asset
                // after we learned it has eighteen.
                for option in c.accepts.iter_mut() {
                    option.annotate();
                }
                // Freshness and provenance of the PRICE, which is a different
                // question from `health` and is answered from a different
                // overlay. An alive endpoint can have a price nothing has ever
                // read; a quarantined one can have a price read an hour ago.
                let seen = observed.get(r.url.as_str());
                c.content_hash = Some(r.content_fingerprint());
                let freshness =
                    crate::discovery_terms::assess_freshness(r, seen, now, freshness_window);
                c.price_freshness = Some(freshness.as_str().to_string());
                c.terms_observed_at = seen.map(|t| t.observed_at);
                c.observation_expires_at = seen.map(|t| t.observed_at + freshness_window);
                c.observed_terms = seen.cloned();

                // What a caller asking for current terms is told. A resource we
                // cannot probe says so instead of sitting in `pending` forever;
                // one that is queued says `pending`, which is the statement that
                // the amounts beside it are the PREVIOUS reading.
                let state = match not_verifiable_reason(r) {
                    Some(reason) => {
                        c.not_verifiable_reason = Some(reason.as_str().to_string());
                        RevalidationState::NotVerifiable(reason)
                    }
                    None if queued.contains(r.url.as_str()) => RevalidationState::Pending,
                    None => RevalidationState::Idle,
                };
                c.price_revalidation = Some(state.as_str().to_string());

                // A listing served from a reading that is no longer current is
                // itself the demand signal. Collected here and enqueued after
                // the guard is dropped, then drained by the job owner inside the
                // probe budget it already had -- never awaited, and never on
                // this response's critical path.
                if c.not_verifiable_reason.is_none() {
                    match freshness {
                        crate::discovery_terms::PriceFreshness::Conflict => {
                            stale_seen.push((r.url.to_string(), RefreshReason::Conflict))
                        }
                        crate::discovery_terms::PriceFreshness::Stale
                        | crate::discovery_terms::PriceFreshness::Unknown => {
                            stale_seen.push((r.url.to_string(), RefreshReason::ListingStale))
                        }
                        crate::discovery_terms::PriceFreshness::Fresh => {}
                    }
                }
                c
            })
            .collect();

        drop(resources);

        // The catalog guard is gone before anything asynchronous happens.
        // Requesting a refresh takes the queue's own lock and, on a replica that
        // does not own the periodic work, spawns a hand-off to the one that
        // does; neither belongs under the catalog guard, and neither may delay
        // this response.
        if crate::discovery_config::revalidation_enabled() && !stale_seen.is_empty() {
            let owns = crate::discovery_owner::owns_jobs();
            let mut accepted = 0;
            for (url, reason) in &stale_seen {
                if self.revalidation.request(url, *reason, now).await {
                    accepted += 1;
                    // Only a request THIS replica accepted as new work is handed
                    // over. The coalescing window is what makes many reads of
                    // one stale record into one job rather than one write per
                    // read.
                    if !owns {
                        self.revalidation.offer_to_owner(url.clone());
                    }
                }
            }
            if accepted > 0 {
                debug!(
                    seen = stale_seen.len(),
                    accepted = accepted,
                    owns_jobs = owns,
                    "queued stale listings for revalidation"
                );
            }
        }

        debug!(
            total = total,
            returned = items.len(),
            limit = limit,
            offset = offset,
            "Listed discovery resources"
        );

        DiscoveryResponse::new(items, Pagination::new(limit, offset, total))
    }

    /// Get the total count of registered resources.
    pub async fn count(&self) -> usize {
        self.resources.read().await.len()
    }

    /// Bulk import resources from an external source (aggregation).
    ///
    /// This performs an upsert: existing resources are updated, new ones are added.
    /// Only updates resources if they have a newer `last_updated` timestamp.
    ///
    /// # Arguments
    ///
    /// * `resources` - The resources to import
    /// * `skip_validation` - Skip URL/type validation (useful for aggregated resources)
    ///
    /// # Returns
    ///
    /// Tuple of (added_count, updated_count, skipped_count)
    pub async fn bulk_import(
        &self,
        resources: Vec<DiscoveryResource>,
        policy: ImportPolicy,
    ) -> Result<(usize, usize, usize), DiscoveryError> {
        use crate::discovery_security::{curation_check, FilterVerdict};

        let mut added = 0;
        let mut updated = 0;
        let mut skipped = 0;
        let mut reject_counts: HashMap<&'static str, usize> = HashMap::new();
        let now = now_secs();

        let mut cache = self.resources.write().await;
        // Computed once, against the catalog as it stands. A record that would
        // be evicted the instant it landed is refused at the door instead.
        let cap = max_resources();
        let threshold = admission_threshold(&cache, cap);

        for mut resource in resources {
            // Response-only fields are resolved when a listing is composed.
            // Nobody upstream gets to assert that their own price is fresh, or
            // that we observed terms we never observed.
            resource.strip_response_only();

            // Filter (aggregator/crawler) or strict-validate (register).
            match policy {
                ImportPolicy::Strict => {
                    if let Err(e) = self.validate_resource(&resource) {
                        debug!(url = %resource.url, error = %e, "Skipping invalid resource during strict bulk import");
                        skipped += 1;
                        continue;
                    }
                }
                ImportPolicy::Filtered => {
                    if let FilterVerdict::Reject(rule) = curation_check(&resource) {
                        *reject_counts.entry(rule).or_insert(0) += 1;
                        skipped += 1;
                        continue;
                    }
                }
            }

            // F5: reject future timestamps (poisoning) — a feed cannot pin an
            // item to the top forever or evade age-based retention.
            if resource.last_updated > now + FUTURE_TIMESTAMP_SKEW_SECS {
                *reject_counts.entry("future-timestamp").or_insert(0) += 1;
                skipped += 1;
                continue;
            }

            let url_key = resource.url.to_string();

            // Admission. Only for records we do not already hold: an update to
            // something in the catalog is not growth, and refusing it would
            // freeze the terms of everything we kept.
            if let Some(floor) = threshold {
                if !cache.contains_key(&url_key)
                    && matches!(resource.source, DiscoverySource::Aggregated)
                    && resource.last_updated <= floor
                {
                    *reject_counts.entry("over-capacity").or_insert(0) += 1;
                    skipped += 1;
                    continue;
                }
            }

            if let Some(existing) = cache.get(&url_key) {
                match import_verdict(&resource, existing) {
                    ImportVerdict::Replace => {
                        // Field-preserving merge: incoming wins for content, but
                        // provenance is protected (F4) — first_seen keeps the
                        // earliest, settlement_count the max, and a self-registered
                        // or settlement record is never downgraded to aggregated by
                        // a colliding feed item.
                        let mut merged = resource;
                        merged.first_seen = match (existing.first_seen, merged.first_seen) {
                            (Some(a), Some(b)) => Some(a.min(b)),
                            (a, b) => a.or(b),
                        };
                        merged.settlement_count =
                            match (existing.settlement_count, merged.settlement_count) {
                                (Some(a), Some(b)) => Some(a.max(b)),
                                (a, b) => a.or(b),
                            };
                        // A settlement date is ours, observed, and about an event
                        // that happened. An import carries no opinion about it, so
                        // it never clears one.
                        merged.last_settled_at =
                            match (existing.last_settled_at, merged.last_settled_at) {
                                (Some(a), Some(b)) => Some(a.max(b)),
                                (a, b) => a.or(b),
                            };
                        merged.source = match existing.source {
                            DiscoverySource::SelfRegistered | DiscoverySource::Settlement => {
                                existing.source
                            }
                            _ => merged.source,
                        };
                        cache.insert(url_key, merged);
                        updated += 1;
                    }
                    // Counted apart from a losing merge, because they mean
                    // opposite things operationally: `unchanged` is the healthy
                    // steady state of an aggregation cycle, and a rising
                    // `superseded` is a feed fighting a higher authority.
                    ImportVerdict::Unchanged => {
                        *reject_counts.entry("unchanged").or_insert(0) += 1;
                        skipped += 1;
                    }
                    ImportVerdict::Keep => {
                        *reject_counts.entry("superseded").or_insert(0) += 1;
                        skipped += 1;
                    }
                }
            } else {
                cache.insert(url_key, resource);
                added += 1;
            }
        }

        // Cap before snapshotting, so the bound applies to what gets published
        // and not just to what this process happens to hold. With admission in
        // place this is now a backstop -- it fires on the first cycle after a
        // cap change, and on records that entered by a path admission does not
        // gate -- rather than the every-cycle churn it was.
        let evicted = enforce_capacity(&mut cache, cap);
        if evicted > 0 {
            info!(
                evicted = evicted,
                held = cache.len(),
                "catalog trimmed to capacity after import"
            );
        }

        // Persist the FULL cache as one snapshot (single PUT) rather than
        // per-item read-modify-write. This avoids the S3 race where a stale
        // per-item save would re-add items the retention GC just removed.
        let changed = added + updated + evicted;
        let snapshot: Vec<DiscoveryResource> = if changed > 0 {
            cache.values().cloned().collect()
        } else {
            Vec::new()
        };
        drop(cache);

        if changed > 0 {
            // Persist synchronously so that, within the single aggregation task,
            // this write completes BEFORE the retention GC's snapshot — otherwise
            // an out-of-order spawned write could re-persist junk the GC removed.
            let n = snapshot.len();
            match self.store.save_all(&snapshot).await {
                Ok(()) => info!(count = n, "Persisted bulk import snapshot to store"),
                // Refused, not failed. Another writer moved the catalog between
                // the read and the write, and republishing this snapshot over
                // theirs would undo it -- deletions included. The next cycle
                // recomputes from a fresh read.
                Err(StoreError::VersionConflict(detail)) => warn!(
                    detail = %detail,
                    count = n,
                    "Bulk import snapshot not published: the catalog moved underneath it"
                ),
                Err(e) => error!(error = %e, "Failed to persist bulk import snapshot"),
            }
        }

        info!(
            added = added,
            updated = updated,
            skipped = skipped,
            rejects = ?reject_counts,
            "Bulk import completed"
        );

        Ok((added, updated, skipped))
    }

    /// Retention GC (WS-A): remove already-stored resources that fail the
    /// static curation rules (junk schemes, private/no-dot hosts, empty
    /// accepts, bad types, oversized fields). This is the one-time cleanup of
    /// the historical catalog plus ongoing hygiene. Deterministic on stored
    /// data (never based on fetch success), so a transient upstream outage
    /// cannot trigger a mass delete. Persists the surviving set as one snapshot
    /// (`save_all`), not N deletes. Disable with `DISCOVERY_ENABLE_RETENTION_GC=false`.
    pub async fn apply_retention(&self) -> usize {
        use crate::discovery_security::{curation_check, FilterVerdict};

        if std::env::var("DISCOVERY_ENABLE_RETENTION_GC")
            .map(|v| v.eq_ignore_ascii_case("false"))
            .unwrap_or(false)
        {
            info!("Retention GC disabled (DISCOVERY_ENABLE_RETENTION_GC=false)");
            return 0;
        }

        let mut cache = self.resources.write().await;
        let before = cache.len();
        let mut removed_by_rule: HashMap<&'static str, usize> = HashMap::new();
        cache.retain(|_url, r| match curation_check(r) {
            FilterVerdict::Accept { .. } => true,
            FilterVerdict::Reject(rule) => {
                *removed_by_rule.entry(rule).or_insert(0) += 1;
                false
            }
        });
        let removed = before - cache.len();
        let keep: Vec<DiscoveryResource> = cache.values().cloned().collect();
        drop(cache);

        if removed > 0 {
            info!(
                removed = removed,
                before = before,
                by_rule = ?removed_by_rule,
                "Retention GC removed non-conforming resources"
            );
            // Synchronous snapshot: this is the authoritative last write of the
            // aggregation cycle (see bulk_import note).
            let kept = keep.len();
            match self.store.save_all(&keep).await {
                Ok(()) => info!(kept = kept, "Retention GC snapshot persisted"),
                Err(StoreError::VersionConflict(detail)) => warn!(
                    detail = %detail,
                    kept = kept,
                    "Retention GC snapshot not published: the catalog moved underneath it. \
                     The GC is deterministic on stored data, so the next cycle removes the \
                     same set from the newer catalog"
                ),
                Err(e) => error!(error = %e, "Failed to persist retention GC snapshot"),
            }
        }
        removed
    }

    /// Check if a resource matches the given filters.
    fn matches_filters(
        &self,
        resource: &DiscoveryResource,
        filters: &Option<DiscoveryFilters>,
    ) -> bool {
        let Some(f) = filters else {
            return true;
        };

        // Filter by category
        if let Some(ref category) = f.category {
            let matches = resource
                .metadata
                .as_ref()
                .and_then(|m| m.category.as_ref())
                .map(|c| c.eq_ignore_ascii_case(category))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Filter by network
        if let Some(ref network) = f.network {
            let matches = resource
                .accepts
                .iter()
                .any(|req| req.network.to_string() == *network);
            if !matches {
                return false;
            }
        }

        // Filter by provider
        if let Some(ref provider) = f.provider {
            let matches = resource
                .metadata
                .as_ref()
                .and_then(|m| m.provider.as_ref())
                .map(|p| p.eq_ignore_ascii_case(provider))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Filter by tag
        if let Some(ref tag) = f.tag {
            let matches = resource
                .metadata
                .as_ref()
                .map(|m| m.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Filter by source (Meta-Bazaar)
        if let Some(ref source) = f.source {
            let matches = resource.source.to_string().eq_ignore_ascii_case(source);
            if !matches {
                return false;
            }
        }

        // Filter by source facilitator (Meta-Bazaar)
        if let Some(ref facilitator) = f.source_facilitator {
            let matches = resource
                .source_facilitator
                .as_ref()
                .map(|sf| sf.eq_ignore_ascii_case(facilitator))
                .unwrap_or(false);
            if !matches {
                return false;
            }
        }

        // Free-text search over url / description / provider / tags. The needle
        // is lowercased once by the caller (`list`), so this is a plain
        // case-insensitive substring scan.
        if let Some(ref needle) = f.q {
            if !needle.is_empty() {
                let url_hit = resource.url.as_str().to_ascii_lowercase().contains(needle);
                let desc_hit = resource.description.to_ascii_lowercase().contains(needle);
                let meta_hit = resource
                    .metadata
                    .as_ref()
                    .map(|m| {
                        m.provider
                            .as_ref()
                            .is_some_and(|p| p.to_ascii_lowercase().contains(needle))
                            || m.category
                                .as_ref()
                                .is_some_and(|c| c.to_ascii_lowercase().contains(needle))
                            || m.tags
                                .iter()
                                .any(|t| t.to_ascii_lowercase().contains(needle))
                    })
                    .unwrap_or(false);
                if !(url_hit || desc_hit || meta_hit) {
                    return false;
                }
            }
        }

        true
    }

    /// Track a settlement by either registering a new resource or incrementing the count.
    ///
    /// This is called after successful /settle when the resource has `discoverable=true`
    /// in the payment requirements extra field.
    ///
    /// # Behavior
    ///
    /// - If resource doesn't exist: Create a new resource with `source: Settlement`
    /// - If resource exists: Increment the `settlement_count`
    ///
    /// # Arguments
    ///
    /// * `resource` - The resource to track (created from settlement data)
    ///
    /// # Returns
    ///
    /// * `true` if a new resource was created
    /// * `false` if an existing resource was updated
    pub async fn track_settlement(
        &self,
        resource: DiscoveryResource,
    ) -> Result<bool, DiscoveryError> {
        let url_key = resource.url.to_string();

        let mut resources = self.resources.write().await;

        if let Some(existing) = resources.get_mut(&url_key) {
            // Resource exists: this is activity on it, so the settlement
            // count and `lastSettledAt` move and NOTHING else does. In
            // particular `accepts` is left exactly as the source declared it --
            // the facilitator sees one payment's requirements, not the seller's
            // price list, and one settled amount is not a universal price.
            existing.record_settlement();
            let resource_for_store = existing.clone();
            debug!(
                url = %url_key,
                settlement_count = existing.settlement_count,
                "Incremented settlement count for existing resource"
            );

            // Release lock before async persistence
            drop(resources);

            // Persist asynchronously
            self.persist_async(resource_for_store);

            Ok(false)
        } else {
            // New resource - register it
            info!(
                url = %url_key,
                resource_type = %resource.resource_type,
                "Auto-registering resource from settlement (discoverable=true)"
            );

            let resource_for_store = resource.clone();
            resources.insert(url_key, resource);

            // Release lock before async persistence
            drop(resources);

            // Persist asynchronously
            self.persist_async(resource_for_store);

            Ok(true)
        }
    }

    /// Validate a resource before registration.
    fn validate_resource(&self, resource: &DiscoveryResource) -> Result<(), DiscoveryError> {
        // Validate URL scheme
        let scheme = resource.url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(DiscoveryError::InvalidUrl(format!(
                "URL must use http or https scheme, got: {}",
                scheme
            )));
        }

        // Reject userinfo in the authority. `https://trusted.example@evil.com/`
        // parses with host=evil.com, but the userinfo segment fools a naive
        // string/prefix match (e.g. a curation tier matcher) into treating it
        // as `trusted.example`. No legitimate paid resource embeds credentials
        // in its discovery URL, so drop the whole class here.
        if !resource.url.username().is_empty() || resource.url.password().is_some() {
            return Err(DiscoveryError::InvalidUrl(
                "URL must not contain userinfo (user[:pass]@host)".to_string(),
            ));
        }

        // SSRF guard: reject IP-literal hosts in private / link-local / loopback
        // address ranges. The classic case is `169.254.169.254` (AWS instance
        // metadata) — anyone able to convince the facilitator to fetch from
        // that host can read EC2/Fargate credentials.
        //
        // `url` 2.5.x (WHATWG host parser) already normalizes alternate IPv4
        // encodings for http(s) — `http://0x7f000001/` becomes host
        // `127.0.0.1` — so `host_str().parse::<IpAddr>()` catches them. The
        // `host_as_encoded_ipv4` fallback is defense-in-depth in case that
        // behavior changes and is shared with the prober's raw-host checks.
        // A DNS name whose A-record points at a private IP cannot be caught
        // here without resolving; that gate lives in the outbound HTTP
        // connector used by the health prober (see docs/plans/bazaar/08).
        if let Some(host) = resource.url.host_str() {
            let literal = host
                .parse::<std::net::IpAddr>()
                .ok()
                .or_else(|| host_as_encoded_ipv4(host).map(std::net::IpAddr::V4));
            if let Some(ip) = literal {
                if is_disallowed_target_ip(&ip) {
                    return Err(DiscoveryError::InvalidUrl(format!(
                        "URL host {host} resolves to a non-routable, private, or link-local address"
                    )));
                }
            }
        }

        // Validate resource type
        // "facilitator" is a special type for x402 payment facilitator services
        let valid_types = ["http", "mcp", "a2a", "facilitator"];
        if !valid_types.contains(&resource.resource_type.as_str()) {
            return Err(DiscoveryError::InvalidResourceType(
                resource.resource_type.clone(),
            ));
        }

        // Validate accepts is not empty (except for facilitators, which process payments rather than requiring them)
        if resource.accepts.is_empty() && resource.resource_type != "facilitator" {
            return Err(DiscoveryError::NoPaymentMethods);
        }

        Ok(())
    }
}

/// Return `true` if the given IP must never be the target of an outbound
/// request originated from the facilitator. Covers:
/// - Loopback (127/8, ::1)
/// - Unspecified (0.0.0.0, ::)
/// - Private (RFC1918, IPv6 unique local fc00::/7)
/// - Link-local (169.254/16 — includes AWS metadata — and fe80::/10)
/// - Carrier-grade NAT (100.64/10)
/// - Benchmark (198.18/15)
/// - Multicast and reserved
///
/// Used by [`DiscoveryRegistry::validate_resource`] to block SSRF against
/// instance metadata and internal services. Shared with `discovery_security`
/// for the outbound HTTP connector guarding the crawler/aggregator/prober.
pub(crate) fn is_disallowed_target_ip(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() {
                return true;
            }
            // RFC1918
            if o[0] == 10 {
                return true;
            }
            if o[0] == 172 && (16..=31).contains(&o[1]) {
                return true;
            }
            if o[0] == 192 && o[1] == 168 {
                return true;
            }
            // Link-local (includes AWS / GCP instance metadata 169.254.169.254)
            if o[0] == 169 && o[1] == 254 {
                return true;
            }
            // Carrier-grade NAT
            if o[0] == 100 && (64..=127).contains(&o[1]) {
                return true;
            }
            // Benchmark / network testing
            if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
                return true;
            }
            // Reserved 192.0.0.0/24 (IETF protocol assignments)
            if o[0] == 192 && o[1] == 0 && o[2] == 0 {
                return true;
            }
            // 6to4 relay anycast 192.88.99.0/24
            if o[0] == 192 && o[1] == 88 && o[2] == 99 {
                return true;
            }
            // Class E / reserved 240.0.0.0/4 (includes 255.255.255.255 broadcast)
            if o[0] >= 240 {
                return true;
            }
            // Documentation (192.0.2/24, 198.51.100/24, 203.0.113/24)
            if o[0] == 192 && o[1] == 0 && o[2] == 2 {
                return true;
            }
            if o[0] == 198 && o[1] == 51 && o[2] == 100 {
                return true;
            }
            if o[0] == 203 && o[1] == 0 && o[2] == 113 {
                return true;
            }
            // 0.0.0.0/8 reserved
            if o[0] == 0 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            // Unique local fc00::/7
            let segs = v6.segments();
            if (segs[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // Link-local fe80::/10
            if (segs[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // IPv4-mapped: extract embedded v4 and re-check
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_disallowed_target_ip(&IpAddr::V4(v4));
            }
            false
        }
    }
}

/// Emulate the parts of libc `inet_aton` that the `url` crate does NOT treat
/// as IP literals, so alternate encodings of an address cannot smuggle a
/// private / metadata target past the SSRF guard in
/// [`DiscoveryRegistry::validate_resource`]. Handles 1-4 dot-separated parts,
/// each decimal / hex (`0x` prefix) / octal (`0` prefix):
///   - `2130706433`     -> 127.0.0.1  (single 32-bit value)
///   - `0x7f000001`     -> 127.0.0.1  (hex)
///   - `017700000001`   -> 127.0.0.1  (octal)
///   - `127.1`          -> 127.0.0.1  (a.d, d is 24-bit)
///
/// Returns `None` for ordinary hostnames (any label that is not fully numeric
/// in one of those bases makes the whole host bail out) and for canonical
/// dotted-decimal IPv4 (which `IpAddr::parse` already handles upstream).
pub(crate) fn host_as_encoded_ipv4(host: &str) -> Option<std::net::Ipv4Addr> {
    if host.is_empty() {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 4 {
        return None;
    }
    fn parse_part(s: &str) -> Option<u64> {
        if s.is_empty() {
            return None;
        }
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()
        } else if s.len() > 1 && s.starts_with('0') {
            u64::from_str_radix(&s[1..], 8).ok()
        } else {
            s.parse::<u64>().ok()
        }
    }
    let vals: Vec<u64> = parts
        .iter()
        .map(|p| parse_part(p))
        .collect::<Option<Vec<_>>>()?;
    // Compose per inet_aton semantics; each non-final part is one octet, the
    // final part absorbs the remaining low-order bytes.
    let addr: u64 = match vals.as_slice() {
        [a] => *a,
        [a, b] => {
            if *a > 0xff || *b > 0x00ff_ffff {
                return None;
            }
            (*a << 24) | *b
        }
        [a, b, c] => {
            if *a > 0xff || *b > 0xff || *c > 0xffff {
                return None;
            }
            (*a << 24) | (*b << 16) | *c
        }
        [a, b, c, d] => {
            if *a > 0xff || *b > 0xff || *c > 0xff || *d > 0xff {
                return None;
            }
            (*a << 24) | (*b << 16) | (*c << 8) | *d
        }
        _ => return None,
    };
    if addr > 0xffff_ffff {
        return None;
    }
    Some(std::net::Ipv4Addr::from(addr as u32))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;
    use crate::discovery_store::{Snapshot, Version};
    use crate::types::{MixedAddress, Scheme, TokenAmount};
    use crate::types_v2::{DiscoveryMetadata, PaymentRequirementsV2};
    use url::Url;

    fn create_test_resource(url: &str, category: Option<&str>) -> DiscoveryResource {
        let network = Caip2NetworkId::eip155(8453);
        let accepts = vec![PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network,
            asset: MixedAddress::Evm(
                "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                    .parse()
                    .unwrap(),
            ),
            amount: TokenAmount::from(1000000u64),
            pay_to: MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            max_timeout_seconds: 300,
            extra: None,
        }
        .into()];

        let mut resource = DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "Test resource".to_string(),
            accepts,
        );

        if let Some(cat) = category {
            resource.metadata = Some(DiscoveryMetadata {
                category: Some(cat.to_string()),
                provider: Some("Test Provider".to_string()),
                tags: vec!["test".to_string()],
            });
        }

        resource
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", Some("finance"));

        registry.register(resource.clone()).await.unwrap();

        let retrieved = registry.get("https://api.example.com/data").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().url, resource.url);
    }

    #[tokio::test]
    async fn test_free_text_search_filters() {
        let registry = DiscoveryRegistry::new();
        let mut a = create_test_resource("https://api.tenjin.blog/read/x", None);
        a.description = "Pay-per-read blogs for agents".to_string();
        let b = create_test_resource("https://other.example.com/weather", None);
        registry.register(a).await.unwrap();
        registry.register(b).await.unwrap();

        let q = |s: &str| DiscoveryFilters {
            q: Some(s.to_string()),
            ..Default::default()
        };
        // matches on URL
        let r = registry.list(10, 0, Some(q("tenjin"))).await;
        assert_eq!(r.pagination.total, 1);
        // matches on description, case-insensitively
        let r = registry.list(10, 0, Some(q("BLOGS"))).await;
        assert_eq!(r.pagination.total, 1);
        // no match
        let r = registry.list(10, 0, Some(q("nonexistent-needle"))).await;
        assert_eq!(r.pagination.total, 0);
        // empty filter set returns everything
        let r = registry.list(10, 0, None).await;
        assert_eq!(r.pagination.total, 2);
    }

    #[tokio::test]
    async fn test_suppress_and_release_hide_resource() {
        let registry = DiscoveryRegistry::new();
        let url = "https://api.example.com/hidden";
        registry
            .register(create_test_resource(url, None))
            .await
            .unwrap();
        assert_eq!(registry.list(10, 0, None).await.pagination.total, 1);

        assert!(
            registry.suppress(url).await,
            "first suppress reports change"
        );
        assert!(!registry.suppress(url).await, "second suppress is a no-op");
        assert_eq!(
            registry.list(10, 0, None).await.pagination.total,
            0,
            "suppressed resource must be hidden from listings"
        );

        assert!(registry.release(url).await, "release reports change");
        assert_eq!(registry.list(10, 0, None).await.pagination.total, 1);
    }

    #[tokio::test]
    async fn test_stats_counts_and_cache() {
        let registry = DiscoveryRegistry::new();
        registry
            .register(create_test_resource("https://api.example.com/a", None))
            .await
            .unwrap();
        registry
            .register(create_test_resource("https://api.example.com/b", None))
            .await
            .unwrap();

        let s = registry.stats().await;
        assert_eq!(s["total"], 2);
        assert_eq!(s["visible"], 2, "nothing quarantined yet");
        assert_eq!(s["bySource"]["self_registered"], 2);
        // Base is the network used by the test fixture.
        assert_eq!(s["byNetwork"]["eip155:8453"], 2);
        assert!(s["generatedAt"].as_u64().is_some());

        // A suppression invalidates the cache, so the next call recomputes.
        registry.suppress("https://api.example.com/a").await;
        let s2 = registry.stats().await;
        assert_eq!(s2["total"], 1, "suppressed resources drop out of stats");
    }

    fn junk_empty_accepts(url: &str) -> DiscoveryResource {
        DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "d".to_string(),
            vec![],
        )
    }

    /// Build an aggregated copy of the fixture with a chosen source date.
    fn aggregated(
        url: &str,
        source_updated_at: Option<u64>,
        description: &str,
    ) -> DiscoveryResource {
        let base = create_test_resource(url, None);
        let mut r = DiscoveryResource::from_aggregation(
            base.url.clone(),
            "http".to_string(),
            description.to_string(),
            base.accepts.clone(),
            "some-feed".to_string(),
            source_updated_at,
        );
        // Keep the record inside the future-timestamp guard regardless of clock.
        r.last_updated = source_updated_at.unwrap_or_else(now_secs);
        r
    }

    #[tokio::test]
    async fn an_undated_import_cannot_outrank_a_dated_record() {
        // F6, the shape it actually took: the aggregator stamped `now` on any
        // feed entry that carried no date, so re-downloading unchanged, stale
        // content was enough to win the merge. The download manufactured the
        // evidence of freshness.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.dated.example/x";
        let dated = aggregated(url, Some(now_secs() - 86_400), "the dated original");
        registry
            .bulk_import(vec![dated], ImportPolicy::Filtered)
            .await
            .unwrap();

        let undated = aggregated(url, None, "an undated re-download of older content");
        let (_added, updated, skipped) = registry
            .bulk_import(vec![undated], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(updated, 0, "an undated feed must not win on our clock");
        assert_eq!(skipped, 1);

        let held = registry.get(url).await.expect("record still held");
        assert_eq!(held.description, "the dated original");
    }

    #[tokio::test]
    async fn a_newer_source_date_still_wins_and_a_dated_claim_beats_an_undated_record() {
        let registry = DiscoveryRegistry::new();
        let url = "https://api.dates.example/x";

        // Undated first, then a dated claim: the dated one wins.
        registry
            .bulk_import(
                vec![aggregated(url, None, "undated")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        let (_a, updated, _s) = registry
            .bulk_import(
                vec![aggregated(url, Some(now_secs() - 100), "dated")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!(updated, 1, "a dated claim beats an undated record");
        assert_eq!(registry.get(url).await.unwrap().description, "dated");

        // A newer source date still wins, exactly as before this change.
        let (_a, updated, _s) = registry
            .bulk_import(
                vec![aggregated(url, Some(now_secs() - 10), "newer")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(registry.get(url).await.unwrap().description, "newer");

        // And an older source date does not.
        let (_a, updated, skipped) = registry
            .bulk_import(
                vec![aggregated(url, Some(now_secs() - 1_000), "older")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!((updated, skipped), (0, 1));
        assert_eq!(registry.get(url).await.unwrap().description, "newer");
    }

    #[tokio::test]
    async fn a_self_registered_record_is_not_replaced_by_an_undated_feed() {
        // A seller's own declaration is first-hand and dated. An aggregated copy
        // that carries no date of its own must not overwrite its terms.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.owner.example/x";
        let mut own = create_test_resource(url, None);
        own.description = "the seller's own listing".to_string();
        registry.register(own).await.unwrap();

        let (_a, updated, skipped) = registry
            .bulk_import(
                vec![aggregated(url, None, "a stale aggregated copy")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!((updated, skipped), (0, 1));
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "the seller's own listing"
        );
    }

    #[tokio::test]
    async fn a_settlement_is_activity_and_does_not_rejuvenate_the_price() {
        // F7. `track_settlement` used to move `last_updated`, which is the field
        // the listing sorts on, the field a reader judges age by, and the field
        // `merge_resource` uses to refuse an out-of-order write. A payment moved
        // all three, and a payment is not a statement about the price.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.settled.example/x";
        let mut original = create_test_resource(url, None);
        original.last_updated = 1_000;
        original.source_updated_at = Some(900);
        let declared_terms = original.accepts.clone();
        let fingerprint_before = original.content_fingerprint();
        registry.update(original).await.unwrap();

        // A settlement arrives carrying DIFFERENT terms -- one option, a
        // different amount -- exactly as the settle path would build it.
        let mut settled = create_test_resource(url, None);
        settled.accepts[0].amount = crate::types::TokenAmount::from(999u64);
        settled.source = DiscoverySource::Settlement;
        let created = registry.track_settlement(settled).await.unwrap();
        assert!(!created, "the resource already existed");

        let held = registry.get(url).await.unwrap();
        assert_eq!(
            held.last_updated, 1_000,
            "a payment is not a content update"
        );
        assert_eq!(
            held.source_updated_at,
            Some(900),
            "and it is certainly not a claim by the source"
        );
        assert_eq!(held.settlement_count, Some(1), "the activity IS recorded");
        assert!(
            held.last_settled_at.is_some(),
            "on its own date, which is the one thing a settlement dates"
        );
        assert_eq!(
            held.accepts, declared_terms,
            "one settled amount is not the seller's price list"
        );
        assert_eq!(
            held.content_fingerprint(),
            fingerprint_before,
            "nothing about the offer changed"
        );
    }

    #[tokio::test]
    async fn reimporting_an_identical_feed_is_not_a_change() {
        // A feed that republishes its page every hour has not repriced anything.
        // Counting it as an update rewrites a 15 MB snapshot to say the same
        // thing, and -- worse -- makes the record look freshly checked.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.identical.example/x";
        let entry = aggregated(url, Some(now_secs() - 3_600), "unchanged");
        let (added, _u, _s) = registry
            .bulk_import(vec![entry.clone()], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(added, 1);
        let first = registry.get(url).await.unwrap();

        let (added, updated, skipped) = registry
            .bulk_import(vec![entry], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(
            (added, updated, skipped),
            (0, 0, 1),
            "an identical re-import is skipped, not counted as an update"
        );
        let second = registry.get(url).await.unwrap();
        assert_eq!(
            second.last_updated, first.last_updated,
            "and nothing was rewritten, so no date moved"
        );

        // A real change on the same date still lands: the hash decides, and it
        // is not being used as a general "skip everything" shortcut.
        let mut changed = aggregated(url, Some(now_secs() - 3_600), "unchanged");
        changed.accepts[0].amount = crate::types::TokenAmount::from(4242u64);
        let (_a, updated, _s) = registry
            .bulk_import(vec![changed], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(updated, 1, "a changed amount is a change");
        assert_eq!(
            registry.get(url).await.unwrap().accepts[0].amount,
            crate::types::TokenAmount::from(4242u64)
        );
    }

    #[tokio::test]
    async fn an_aggregated_copy_never_outranks_the_owners_own_declaration() {
        // The case that is live in production today: every record stored before
        // this phase has NO `sourceUpdatedAt` (0 of 24 636 on 2026-09-10), so a
        // dated feed entry beat every self-registered listing on a field one
        // side did not have. Dates order versions within one authority; they do
        // not promote a third party's copy above the owner.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.ladder.example/x";
        let mut own = create_test_resource(url, None);
        own.description = "the seller's own listing".to_string();
        own.source_updated_at = None; // a record from before the field existed
        registry.update(own).await.unwrap();

        let (_a, updated, skipped) = registry
            .bulk_import(
                vec![aggregated(
                    url,
                    Some(now_secs() - 10),
                    "a freshly dated copy",
                )],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!((updated, skipped), (0, 1));
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "the seller's own listing"
        );
    }

    #[tokio::test]
    async fn the_provenance_ladder_decides_when_neither_side_is_dated() {
        // Neither carries a date, so nothing about time can decide. The ladder
        // does: the origin's own document outranks a third party's copy of it.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.undated.example/x";
        registry
            .bulk_import(
                vec![aggregated(url, None, "a third party's copy")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();

        let mut crawled = aggregated(url, None, "the origin's own document");
        crawled.source = DiscoverySource::Crawled;
        let (_a, updated, _s) = registry
            .bulk_import(vec![crawled], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(updated, 1, "the higher rung wins");
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "the origin's own document"
        );

        // ... and the copy cannot take it back, however it dates itself.
        let (_a, updated, skipped) = registry
            .bulk_import(
                vec![aggregated(url, Some(now_secs() - 5), "the copy, redated")],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!((updated, skipped), (0, 1));
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "the origin's own document"
        );
    }

    #[tokio::test]
    async fn two_undated_feeds_of_equal_rank_do_not_flip_the_record_every_cycle() {
        // Nothing separates them: same rung, no dates, different content. The
        // answer is not "whichever was fetched last" -- that makes the record
        // oscillate in crawl order for as long as both feeds publish it.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.twofeeds.example/x";
        let mut first = aggregated(url, None, "feed one says this");
        first.source_facilitator = Some("feed-one".to_string());
        registry
            .bulk_import(vec![first], ImportPolicy::Filtered)
            .await
            .unwrap();

        let mut second = aggregated(url, None, "feed two says that");
        second.source_facilitator = Some("feed-two".to_string());
        let (_a, updated, skipped) = registry
            .bulk_import(vec![second], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!((updated, skipped), (0, 1));
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "feed one says this"
        );

        // But the publisher that owns the entry can still revise it, date or no
        // date -- it is the authority on its own listing.
        let mut revised = aggregated(url, None, "feed one, revised");
        revised.source_facilitator = Some("feed-one".to_string());
        let (_a, updated, _s) = registry
            .bulk_import(vec![revised], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(
            registry.get(url).await.unwrap().description,
            "feed one, revised"
        );
    }

    #[tokio::test]
    async fn a_late_import_cannot_erase_a_direct_observation() {
        // The overlay is a different object with one writer, so this is not a
        // rule being enforced -- it is a shape that cannot be expressed. The
        // import rewrites the terms; the reading of the origin stays, and the
        // listing reports the disagreement rather than hiding it.
        use crate::discovery_terms::{
            ObservationContext, ObservationPhase, ObservedTerms, PriceFreshness, TermsProvenance,
            TermsTransport,
        };
        let registry = DiscoveryRegistry::new();
        let url = "https://api.observed.example/x";
        let listed = aggregated(url, Some(now_secs() - 7_200), "listed");
        registry
            .bulk_import(vec![listed.clone()], ImportPolicy::Filtered)
            .await
            .unwrap();

        let observed_price = listed.accepts.clone();
        registry
            .terms()
            .record(
                url,
                ObservedTerms {
                    accepts: observed_price,
                    observed_at: now_secs(),
                    context: ObservationContext::anonymous_get("http"),
                    phase: ObservationPhase::Verification,
                    provenance: TermsProvenance::OriginResponse,
                    transport: TermsTransport::Header,
                    x402_version: Some(2),
                    http_status: Some(402),
                    content_hash: Some(listed.content_fingerprint()),
                    conflict: None,
                    rejected: Default::default(),
                    truncated: false,
                },
            )
            .await;

        let listing = registry.list(10, 0, None).await;
        assert_eq!(
            listing.items[0].price_freshness.as_deref(),
            Some(PriceFreshness::Fresh.as_str())
        );
        assert!(listing.items[0].terms_observed_at.is_some());

        // Now a stale feed lands with a newer date and a different price.
        let mut late = aggregated(url, Some(now_secs() - 60), "a late feed");
        late.accepts[0].amount = crate::types::TokenAmount::from(777_777u64);
        registry
            .bulk_import(vec![late], ImportPolicy::Filtered)
            .await
            .unwrap();

        let observation = registry.terms().get(url).await;
        assert!(
            observation.is_some(),
            "an import cannot reach the observation overlay"
        );
        let listing = registry.list(10, 0, None).await;
        assert_eq!(
            listing.items[0].price_freshness.as_deref(),
            Some(PriceFreshness::Conflict.as_str()),
            "the disagreement is reported, not resolved by overwriting one side"
        );
        assert_eq!(
            listing.items[0].accepts[0].amount,
            crate::types::TokenAmount::from(777_777u64),
            "the declared listing is still what the source declared"
        );
        assert_eq!(
            listing.items[0].observed_terms.as_ref().unwrap().accepts[0].amount,
            listed.accepts[0].amount,
            "and the observation is still what the origin answered"
        );
    }

    #[tokio::test]
    async fn a_restart_keeps_the_dates_and_the_observation() {
        use crate::discovery_store::MemoryStore;
        use crate::discovery_terms::{
            ObservationContext, ObservationPhase, ObservedTerms, PriceFreshness, TermsProvenance,
            TermsTransport,
        };
        let url = "https://api.restart.example/x";
        let store = MemoryStore::new();
        let mut r = create_test_resource(url, None);
        r.last_updated = 1_000;
        r.source_updated_at = Some(900);
        r.last_settled_at = Some(1_500);
        let fingerprint = r.content_fingerprint();
        store.save(&r).await.unwrap();

        // The catalog comes back from the store...
        let registry = DiscoveryRegistry::with_store(store).await.unwrap();
        let back = registry.get(url).await.unwrap();
        assert_eq!(back.last_updated, 1_000);
        assert_eq!(back.source_updated_at, Some(900));
        assert_eq!(back.last_settled_at, Some(1_500));
        assert_eq!(back.record_version, crate::types_v2::RECORD_FORMAT_VERSION);
        assert_eq!(back.content_fingerprint(), fingerprint);

        // ... and the overlay comes back separately, which is the point of it
        // being separate.
        registry
            .terms()
            .record(
                url,
                ObservedTerms {
                    accepts: back.accepts.clone(),
                    observed_at: now_secs(),
                    context: ObservationContext::anonymous_get("http"),
                    phase: ObservationPhase::Verification,
                    provenance: TermsProvenance::OriginResponse,
                    transport: TermsTransport::Header,
                    x402_version: Some(2),
                    http_status: Some(402),
                    content_hash: Some(fingerprint),
                    conflict: None,
                    rejected: Default::default(),
                    truncated: false,
                },
            )
            .await;
        let listing = registry.list(10, 0, None).await;
        let item = &listing.items[0];
        assert_eq!(
            item.price_freshness.as_deref(),
            Some(PriceFreshness::Fresh.as_str())
        );
        assert_eq!(item.observed_terms.as_ref().unwrap().context.method, "GET");
        assert_eq!(
            item.observed_terms.as_ref().unwrap().provenance,
            TermsProvenance::OriginResponse
        );
        assert_eq!(item.last_settled_at, Some(1_500));
    }

    #[tokio::test]
    async fn a_stale_listing_queues_itself_without_blocking_the_response() {
        // Refresh-on-read: serving a listing whose reading has aged out IS the
        // demand signal. What must not happen is the response waiting for it.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.staleread.example/x";
        registry
            .register(create_test_resource(url, None))
            .await
            .unwrap();

        // Never observed, so `unknown`.
        let listed = registry.list(10, 0, None).await;
        assert_eq!(listed.items[0].price_freshness.as_deref(), Some("unknown"));
        assert!(
            registry.revalidation().is_pending(url).await,
            "the read enqueued it"
        );

        // A second read of the same page does not add a second job.
        registry.list(10, 0, None).await;
        assert_eq!(registry.revalidation().depth().await, 1);

        // And the listing says so, rather than presenting the cache as current.
        let listed = registry.list(10, 0, None).await;
        assert_eq!(
            listed.items[0].price_revalidation.as_deref(),
            Some("pending")
        );
    }

    #[tokio::test]
    async fn a_resource_we_cannot_probe_says_so_instead_of_queueing_forever() {
        // An MCP endpoint answers a JSON-RPC handshake, not a payment challenge.
        // Its price is not observable that way, and firing a POST at it to find
        // out would be starting somebody's operation to see what it costs.
        let registry = DiscoveryRegistry::new();
        let url = "https://api.mcp.example/x";
        let mut r = create_test_resource(url, None);
        r.resource_type = "mcp".to_string();
        registry.register(r).await.unwrap();

        let listed = registry.list(10, 0, None).await;
        assert_eq!(
            listed.items[0].price_revalidation.as_deref(),
            Some("not_verifiable")
        );
        assert_eq!(
            listed.items[0].not_verifiable_reason.as_deref(),
            Some("not-a-get-resource")
        );
        assert!(
            !registry.revalidation().is_pending(url).await,
            "and it is not queued for a probe that could never answer"
        );
    }

    #[tokio::test]
    async fn a_url_template_is_not_an_address_to_probe() {
        let registry = DiscoveryRegistry::new();
        let url = "https://api.tmpl.example/item/%7Bid%7D";
        registry
            .register(create_test_resource(url, None))
            .await
            .unwrap();
        let listed = registry.list(10, 0, None).await;
        assert_eq!(
            listed.items[0].not_verifiable_reason.as_deref(),
            Some("unprobeable")
        );
    }

    #[tokio::test]
    async fn a_fresh_listing_asks_for_nothing() {
        use crate::discovery_terms::{
            ObservationContext, ObservationPhase, ObservedTerms, TermsProvenance, TermsTransport,
        };
        let registry = DiscoveryRegistry::new();
        let url = "https://api.fresh.example/x";
        let r = create_test_resource(url, None);
        let fingerprint = r.content_fingerprint();
        let accepts = r.accepts.clone();
        registry.register(r).await.unwrap();
        registry
            .terms()
            .record(
                url,
                ObservedTerms {
                    accepts,
                    observed_at: now_secs(),
                    context: ObservationContext::anonymous_get("http"),
                    phase: ObservationPhase::Verification,
                    provenance: TermsProvenance::OriginResponse,
                    transport: TermsTransport::Header,
                    x402_version: Some(2),
                    http_status: Some(402),
                    content_hash: Some(fingerprint),
                    conflict: None,
                    rejected: Default::default(),
                    truncated: false,
                },
            )
            .await;

        let listed = registry.list(10, 0, None).await;
        assert_eq!(listed.items[0].price_freshness.as_deref(), Some("fresh"));
        assert_eq!(listed.items[0].price_revalidation.as_deref(), Some("idle"));
        assert!(listed.items[0].observation_expires_at.is_some());
        assert_eq!(
            registry.revalidation().depth().await,
            0,
            "a current reading is not work"
        );
    }

    #[tokio::test]
    async fn a_registrant_cannot_assert_that_its_own_price_is_fresh() {
        let registry = DiscoveryRegistry::new();
        let url = "https://api.selfclaim.example/x";
        let mut r = create_test_resource(url, None);
        r.price_freshness = Some("fresh".to_string());
        r.terms_observed_at = Some(now_secs());
        r.content_hash = Some("whatever-i-say".to_string());
        registry.register(r).await.unwrap();

        let held = registry.get(url).await.unwrap();
        assert_eq!(held.price_freshness, None);
        assert_eq!(held.terms_observed_at, None);
        assert_eq!(held.content_hash, None);

        // The listing answers it from the overlay, where there is nothing.
        let listing = registry.list(10, 0, None).await;
        assert_eq!(listing.items[0].price_freshness.as_deref(), Some("unknown"));
        assert_eq!(listing.items[0].terms_observed_at, None);
    }

    #[tokio::test]
    async fn listing_resolves_price_semantics_without_persisting_them() {
        let registry = DiscoveryRegistry::new();
        registry
            .register(create_test_resource("https://api.annotated.com/x", None))
            .await
            .unwrap();

        let listed = registry.list(10, 0, None).await;
        let option = &listed.items[0].accepts[0];
        assert_eq!(option.settleable, Some(true));
        assert_eq!(option.asset_symbol.as_deref(), Some("USDC"));
        assert_eq!(option.asset_decimals, Some(6));

        // The held copy stays clean: these are answers about this build and the
        // current deployment table, so they are resolved on every read rather
        // than frozen into the record.
        let held = registry.get("https://api.annotated.com/x").await.unwrap();
        assert_eq!(held.accepts[0].settleable, None);
        assert_eq!(held.accepts[0].asset_symbol, None);
        assert_eq!(held.accepts[0].asset_decimals, None);
    }

    // =======================================================================
    // 2026-09-10: the catalog outgrew the task
    // =======================================================================

    /// An aggregated record whose feed date and our write date agree.
    ///
    /// Both are set: the cap orders evictions by `last_updated` (our clock) and
    /// the merge decides by `source_updated_at` (the feed's claim). A fixture
    /// that moved only one of them tested the cap while the merge quietly
    /// refused everything for a different reason.
    fn aggregated_at(url: &str, last_updated: u64) -> DiscoveryResource {
        let mut r = create_test_resource(url, None);
        r.source = DiscoverySource::Aggregated;
        r.source_facilitator = Some("some-feed".to_string());
        r.last_updated = last_updated;
        r.source_updated_at = Some(last_updated);
        r
    }

    fn cache_of(resources: Vec<DiscoveryResource>) -> HashMap<String, DiscoveryResource> {
        resources
            .into_iter()
            .map(|r| (r.url.to_string(), r))
            .collect()
    }

    #[test]
    fn a_catalog_under_capacity_is_left_alone() {
        let mut cache = cache_of(vec![
            aggregated_at("https://a.example/1", 100),
            aggregated_at("https://b.example/2", 200),
        ]);
        assert_eq!(enforce_capacity(&mut cache, 10), 0);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn over_capacity_the_oldest_aggregated_copies_go_first() {
        let mut cache = cache_of(vec![
            aggregated_at("https://old.example/1", 100),
            aggregated_at("https://mid.example/2", 200),
            aggregated_at("https://new.example/3", 300),
        ]);
        assert_eq!(enforce_capacity(&mut cache, 2), 1);
        assert!(
            !cache.contains_key("https://old.example/1"),
            "the least recently touched copy is the one we least miss"
        );
        assert!(cache.contains_key("https://new.example/3"));
    }

    #[test]
    fn a_cap_never_evicts_a_first_hand_record() {
        // The eviction order is provenance BEFORE recency, and this is why: an
        // aggregated copy can be re-fetched from the source that still publishes
        // it, and a listing somebody registered with us cannot be re-fetched
        // from anywhere. Deleting the only copy of a seller's listing to stay
        // under a number is not a capacity fix.
        let mut own = create_test_resource("https://owner.example/x", None);
        own.last_updated = 1; // by far the oldest
        let mut settled = create_test_resource("https://settled.example/x", None);
        settled.source = DiscoverySource::Settlement;
        settled.last_updated = 2;
        let mut crawled = create_test_resource("https://crawled.example/x", None);
        crawled.source = DiscoverySource::Crawled;
        crawled.last_updated = 3;

        let mut cache = cache_of(vec![
            own,
            settled,
            crawled,
            aggregated_at("https://copy.example/1", 9_000),
            aggregated_at("https://copy.example/2", 9_001),
        ]);

        let dropped = enforce_capacity(&mut cache, 3);
        assert_eq!(dropped, 2, "both copies go");
        assert!(cache.contains_key("https://owner.example/x"));
        assert!(cache.contains_key("https://settled.example/x"));
        assert!(cache.contains_key("https://crawled.example/x"));
    }

    #[test]
    fn a_catalog_of_only_first_hand_records_is_never_trimmed_below_them() {
        let mut own_a = create_test_resource("https://owner.example/a", None);
        own_a.last_updated = 1;
        let mut own_b = create_test_resource("https://owner.example/b", None);
        own_b.last_updated = 2;
        let mut cache = cache_of(vec![own_a, own_b]);
        assert_eq!(enforce_capacity(&mut cache, 1), 0);
        assert_eq!(cache.len(), 2, "over capacity, but nothing is evictable");
    }

    #[test]
    fn a_cap_of_zero_disables_the_bound() {
        let mut cache = cache_of(vec![
            aggregated_at("https://a.example/1", 100),
            aggregated_at("https://b.example/2", 200),
        ]);
        assert_eq!(enforce_capacity(&mut cache, 0), 0);
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn a_snapshot_written_before_the_cap_is_trimmed_on_the_way_in() {
        // The oversized object in S3 is the FIRST thing a task touches. If the
        // cap applied only at import, every restart would re-inhale the whole
        // thing and the fix would never arrive. This is also what makes the
        // 98 MB object safe to deploy against without touching S3 by hand.
        use crate::discovery_store::MemoryStore;
        let store = MemoryStore::new();
        let fat: Vec<DiscoveryResource> = (0..30)
            .map(|i| aggregated_at(&format!("https://fat.example/{i}"), 1_000 + i))
            .collect();
        store.save_all(&fat).await.unwrap();

        std::env::set_var("DISCOVERY_MAX_RESOURCES", "10");
        let registry = DiscoveryRegistry::with_store(store).await.unwrap();
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");

        assert_eq!(registry.count().await, 10, "trimmed on load");
        // The newest survive.
        assert!(registry.get("https://fat.example/29").await.is_some());
        assert!(registry.get("https://fat.example/0").await.is_none());
    }

    #[tokio::test]
    async fn a_feed_larger_than_the_cap_stops_rewriting_the_catalog_every_cycle() {
        // 2.21.1 admitted everything and evicted afterwards, so the records it
        // dropped came back from the feed next cycle as NEW: `added` never went
        // to zero, the snapshot was republished forever although nothing
        // changed, and every re-added record looked unprobed to the health
        // prober. Measured then, over five cycles of one unchanged page:
        // added=25, then 15, 15, 15, 15.
        std::env::set_var("DISCOVERY_MAX_RESOURCES", "10");
        let registry = DiscoveryRegistry::new();
        let base = now_secs() - 3_600;
        let page: Vec<DiscoveryResource> = (0..25)
            .map(|i| aggregated_at(&format!("https://feed.example/{i}"), base + i))
            .collect();

        let (added, _u, _s) = registry
            .bulk_import(page.clone(), ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(added, 25, "the first cycle takes the page in");
        assert_eq!(registry.count().await, 10, "and the cap trims it");

        // Every later cycle of the SAME page must be a no-op.
        for cycle in 2..=4 {
            let (added, updated, _skipped) = registry
                .bulk_import(page.clone(), ImportPolicy::Filtered)
                .await
                .unwrap();
            assert_eq!(
                (added, updated),
                (0, 0),
                "cycle {cycle} re-added records the cap had already refused"
            );
            assert_eq!(registry.count().await, 10);
        }
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");
    }

    #[tokio::test]
    async fn a_genuinely_newer_entry_still_gets_in_when_the_catalog_is_full() {
        // Admission must not freeze the catalog on whatever it saw first. An
        // entry newer than the oldest survivor displaces it; that is the cap
        // working, not the cap refusing to work.
        std::env::set_var("DISCOVERY_MAX_RESOURCES", "3");
        let registry = DiscoveryRegistry::new();
        let base = now_secs() - 3_600;
        registry
            .bulk_import(
                (0..3)
                    .map(|i| aggregated_at(&format!("https://old.example/{i}"), base + i))
                    .collect(),
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();

        let (added, _u, _s) = registry
            .bulk_import(
                vec![aggregated_at("https://fresh.example/x", base + 100)],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");
        assert_eq!(added, 1, "a newer entry is admitted");
        assert_eq!(registry.count().await, 3, "and the oldest left");
        assert!(registry.get("https://fresh.example/x").await.is_some());
        assert!(registry.get("https://old.example/0").await.is_none());
    }

    #[tokio::test]
    async fn an_update_to_a_held_record_is_never_refused_by_admission() {
        // Admission gates GROWTH. Refusing an update to something we already
        // list would freeze its terms at whatever we first saw, which is the
        // opposite of what a catalog is for.
        std::env::set_var("DISCOVERY_MAX_RESOURCES", "2");
        let registry = DiscoveryRegistry::new();
        let base = now_secs() - 3_600;
        registry
            .bulk_import(
                vec![
                    aggregated_at("https://a.example/x", base + 10),
                    aggregated_at("https://b.example/x", base + 11),
                ],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();

        // Same URL, newer source date, different description: an update, and the
        // catalog is full.
        let mut revised = aggregated_at("https://a.example/x", base + 50);
        revised.description = "revised terms".to_string();
        let (_a, updated, _s) = registry
            .bulk_import(vec![revised], ImportPolicy::Filtered)
            .await
            .unwrap();
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");
        assert_eq!(updated, 1);
        assert_eq!(
            registry
                .get("https://a.example/x")
                .await
                .unwrap()
                .description,
            "revised terms"
        );
    }

    #[tokio::test]
    async fn an_import_cannot_grow_the_catalog_past_the_cap() {
        std::env::set_var("DISCOVERY_MAX_RESOURCES", "5");
        let registry = DiscoveryRegistry::new();
        let incoming: Vec<DiscoveryResource> = (0..20)
            .map(|i| aggregated_at(&format!("https://feed.example/{i}"), 1_000 + i))
            .collect();
        registry
            .bulk_import(incoming, ImportPolicy::Filtered)
            .await
            .unwrap();
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");
        assert_eq!(registry.count().await, 5);
    }

    #[tokio::test]
    async fn test_bulk_import_filtered_drops_junk() {
        let registry = DiscoveryRegistry::new();
        let good = create_test_resource("https://api.good.com/x", None);
        let empty = junk_empty_accepts("https://api.empty.com/x");
        let private = create_test_resource("http://127.0.0.1/x", None); // R2 private-ip
        let (added, _updated, skipped) = registry
            .bulk_import(vec![good, empty, private], ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(added, 1, "only the good resource should be added");
        assert_eq!(skipped, 2, "empty-accepts + private-ip must be filtered");
    }

    #[tokio::test]
    async fn test_apply_retention_removes_stored_junk() {
        use crate::discovery_store::MemoryStore;
        // Preload the store with historical data (bypassing the import filter,
        // as pre-WS-A junk in S3 would be).
        let store = MemoryStore::new();
        store
            .save(&create_test_resource("https://api.good.com/x", None))
            .await
            .unwrap();
        store
            .save(&junk_empty_accepts("https://api.empty.com/x"))
            .await
            .unwrap();
        store
            .save(&create_test_resource("http://127.0.0.1/x", None))
            .await
            .unwrap();
        let registry = DiscoveryRegistry::with_store(store).await.unwrap();
        assert_eq!(registry.count().await, 3);

        let removed = registry.apply_retention().await;
        assert_eq!(removed, 2, "empty-accepts + private-ip must be GC'd");
        assert_eq!(registry.count().await, 1);
    }

    #[tokio::test]
    async fn test_register_rejects_userinfo_url() {
        // F1: `trusted@evil.com` must not slip past validation (host is evil.com).
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.meshrelay.xyz@evil.com/x", None);
        let result = registry.register(resource).await;
        assert!(
            matches!(result, Err(DiscoveryError::InvalidUrl(_))),
            "userinfo URL must be rejected, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_register_rejects_encoded_ip_ssrf() {
        // F2: alternate encodings of 127.0.0.1 / 169.254.169.254 must be rejected.
        // Whether `Url::parse` normalizes these to a canonical IP (caught by the
        // existing literal check) or keeps them as a numeric host (caught by
        // `host_as_encoded_ipv4`), the outcome must be rejection. URLs that
        // `Url::parse` refuses outright are skipped — they never become a resource.
        let registry = DiscoveryRegistry::new();
        for host in [
            "http://2130706433/x",   // decimal 127.0.0.1
            "http://0x7f000001/x",   // hex 127.0.0.1
            "http://017700000001/x", // octal 127.0.0.1
            "http://127.1/x",        // short form 127.0.0.1
            "http://2852039166/x",   // decimal 169.254.169.254
        ] {
            let Ok(url) = Url::parse(host) else { continue };
            let mut resource = create_test_resource("https://placeholder.example/x", None);
            resource.url = url;
            let result = registry.register(resource).await;
            assert!(
                matches!(result, Err(DiscoveryError::InvalidUrl(_))),
                "encoded-IP host {host} must be rejected, got {result:?}"
            );
        }
    }

    #[test]
    fn test_host_as_encoded_ipv4() {
        use std::net::Ipv4Addr;
        assert_eq!(
            host_as_encoded_ipv4("2130706433"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            host_as_encoded_ipv4("0x7f000001"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            host_as_encoded_ipv4("017700000001"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            host_as_encoded_ipv4("127.1"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            host_as_encoded_ipv4("2852039166"),
            Some(Ipv4Addr::new(169, 254, 169, 254))
        );
        // ordinary hostnames must not be interpreted as encoded IPs
        assert_eq!(host_as_encoded_ipv4("api.meshrelay.xyz"), None);
        assert_eq!(host_as_encoded_ipv4("example.com"), None);
        assert_eq!(host_as_encoded_ipv4("123.example.com"), None);
    }

    #[test]
    fn test_is_disallowed_target_ip_extended_ranges() {
        use std::net::{IpAddr, Ipv4Addr};
        // 240.0.0.0/4 Class E + broadcast
        assert!(is_disallowed_target_ip(&IpAddr::V4(Ipv4Addr::new(
            240, 0, 0, 1
        ))));
        assert!(is_disallowed_target_ip(&IpAddr::V4(Ipv4Addr::new(
            255, 255, 255, 255
        ))));
        // 6to4 relay anycast
        assert!(is_disallowed_target_ip(&IpAddr::V4(Ipv4Addr::new(
            192, 88, 99, 1
        ))));
        // AWS/GCP metadata still blocked
        assert!(is_disallowed_target_ip(&IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
        // a normal public IP is allowed
        assert!(!is_disallowed_target_ip(&IpAddr::V4(Ipv4Addr::new(
            93, 184, 216, 34
        ))));
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        registry.register(resource.clone()).await.unwrap();

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::AlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_update_upsert() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        // Update non-existent resource (upsert)
        registry.update(resource.clone()).await.unwrap();
        assert_eq!(registry.count().await, 1);

        // Update existing resource
        let mut updated = resource.clone();
        updated.description = "Updated description".to_string();
        registry.update(updated).await.unwrap();

        let retrieved = registry.get("https://api.example.com/data").await.unwrap();
        assert_eq!(retrieved.description, "Updated description");
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = DiscoveryRegistry::new();
        let resource = create_test_resource("https://api.example.com/data", None);

        registry.register(resource).await.unwrap();
        assert_eq!(registry.count().await, 1);

        registry
            .unregister("https://api.example.com/data")
            .await
            .unwrap();
        assert_eq!(registry.count().await, 0);
    }

    #[tokio::test]
    async fn test_unregister_not_found() {
        let registry = DiscoveryRegistry::new();

        let result = registry.unregister("https://nonexistent.com").await;
        assert!(matches!(result, Err(DiscoveryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_pagination() {
        let registry = DiscoveryRegistry::new();

        // Register 5 resources
        for i in 0..5 {
            let resource = create_test_resource(
                &format!("https://api{}.example.com/data", i),
                Some("finance"),
            );
            registry.register(resource).await.unwrap();
        }

        // Get first page
        let page1 = registry.list(2, 0, None).await;
        assert_eq!(page1.items.len(), 2);
        assert_eq!(page1.pagination.total, 5);
        assert_eq!(page1.pagination.limit, 2);
        assert_eq!(page1.pagination.offset, 0);

        // Get second page
        let page2 = registry.list(2, 2, None).await;
        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.pagination.offset, 2);

        // Get last page
        let page3 = registry.list(2, 4, None).await;
        assert_eq!(page3.items.len(), 1);
    }

    #[tokio::test]
    async fn test_filter_by_category() {
        let registry = DiscoveryRegistry::new();

        registry
            .register(create_test_resource(
                "https://api1.example.com",
                Some("finance"),
            ))
            .await
            .unwrap();
        registry
            .register(create_test_resource("https://api2.example.com", Some("ai")))
            .await
            .unwrap();
        registry
            .register(create_test_resource(
                "https://api3.example.com",
                Some("finance"),
            ))
            .await
            .unwrap();

        let filters = Some(DiscoveryFilters {
            category: Some("finance".to_string()),
            ..Default::default()
        });

        let response = registry.list(10, 0, filters).await;
        assert_eq!(response.pagination.total, 2);
        assert!(response.items.iter().all(|r| r
            .metadata
            .as_ref()
            .unwrap()
            .category
            .as_ref()
            .unwrap()
            == "finance"));
    }

    #[tokio::test]
    async fn test_validation_invalid_url_scheme() {
        let registry = DiscoveryRegistry::new();

        let network = Caip2NetworkId::eip155(8453);
        let accepts = vec![PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network,
            asset: MixedAddress::Evm(
                "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                    .parse()
                    .unwrap(),
            ),
            amount: TokenAmount::from(1000000u64),
            pay_to: MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            max_timeout_seconds: 300,
            extra: None,
        }
        .into()];

        let resource = DiscoveryResource::new(
            Url::parse("ftp://invalid.com").unwrap(),
            "http".to_string(),
            "Test".to_string(),
            accepts,
        );

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::InvalidUrl(_))));
    }

    #[tokio::test]
    async fn test_validation_invalid_resource_type() {
        let registry = DiscoveryRegistry::new();

        let network = Caip2NetworkId::eip155(8453);
        let accepts = vec![PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network,
            asset: MixedAddress::Evm(
                "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                    .parse()
                    .unwrap(),
            ),
            amount: TokenAmount::from(1000000u64),
            pay_to: MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            max_timeout_seconds: 300,
            extra: None,
        }
        .into()];

        let resource = DiscoveryResource::new(
            Url::parse("https://api.example.com").unwrap(),
            "websocket".to_string(), // Invalid type
            "Test".to_string(),
            accepts,
        );

        let result = registry.register(resource).await;
        assert!(matches!(
            result,
            Err(DiscoveryError::InvalidResourceType(_))
        ));
    }

    #[tokio::test]
    async fn test_validation_no_payment_methods() {
        let registry = DiscoveryRegistry::new();

        let resource = DiscoveryResource::new(
            Url::parse("https://api.example.com").unwrap(),
            "http".to_string(),
            "Test".to_string(),
            vec![], // Empty accepts
        );

        let result = registry.register(resource).await;
        assert!(matches!(result, Err(DiscoveryError::NoPaymentMethods)));
    }

    #[tokio::test]
    async fn test_limit_capped_at_100() {
        let registry = DiscoveryRegistry::new();

        let response = registry.list(500, 0, None).await;
        assert_eq!(response.pagination.limit, 100);
    }

    #[tokio::test]
    async fn test_facilitator_resource_type() {
        let registry = DiscoveryRegistry::new();

        // Facilitator resources can have empty accepts (they process payments, not require them)
        let resource = DiscoveryResource::new(
            Url::parse("https://facilitator.example.com").unwrap(),
            "facilitator".to_string(),
            "Test Facilitator".to_string(),
            vec![], // Empty accepts is OK for facilitators
        );

        let result = registry.register(resource).await;
        assert!(result.is_ok());

        // Verify it was registered
        let response = registry.list(10, 0, None).await;
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].resource_type, "facilitator");
    }

    // ===================================================================
    // A4: the replicas that do not own the jobs follow the one that does
    // ===================================================================
    //
    // Single ownership only pays for itself if the other two replicas stay as
    // fresh as they were when all three aggregated. These cover the half that
    // makes that true, and the two ways it could quietly stop being true: a
    // read that fails, and a local registration that is still in flight.

    /// A store that answers `current_version` and `load_snapshot` on demand,
    /// and can fail either.
    #[derive(Debug, Default)]
    struct FollowedStore {
        state: std::sync::Mutex<FollowedState>,
    }

    #[derive(Debug, Default)]
    struct FollowedState {
        resources: Vec<DiscoveryResource>,
        version: u64,
        fail_head: bool,
        fail_read: bool,
        heads: u32,
        reads: u32,
    }

    impl FollowedStore {
        fn publish(&self, resources: Vec<DiscoveryResource>) {
            let mut state = self.state.lock().unwrap();
            state.resources = resources;
            state.version += 1;
        }

        fn counts(&self) -> (u32, u32) {
            let state = self.state.lock().unwrap();
            (state.heads, state.reads)
        }

        fn fail_head(&self, yes: bool) {
            self.state.lock().unwrap().fail_head = yes;
        }

        fn fail_read(&self, yes: bool) {
            self.state.lock().unwrap().fail_read = yes;
        }
    }

    #[async_trait::async_trait]
    impl DiscoveryStore for Arc<FollowedStore> {
        async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
            Ok(self.load_snapshot().await?.resources)
        }

        async fn load_snapshot(&self) -> Result<Snapshot, StoreError> {
            let mut state = self.state.lock().unwrap();
            state.reads += 1;
            if state.fail_read {
                return Err(StoreError::ReadError("injected read failure".into()));
            }
            Ok(Snapshot {
                resources: state.resources.clone(),
                version: Version::At(state.version.to_string()),
            })
        }

        async fn current_version(&self) -> Result<Version, StoreError> {
            let mut state = self.state.lock().unwrap();
            state.heads += 1;
            if state.fail_head {
                return Err(StoreError::ReadError("injected head failure".into()));
            }
            Ok(Version::At(state.version.to_string()))
        }

        async fn save_snapshot(
            &self,
            resources: &[DiscoveryResource],
            _expected: &Version,
        ) -> Result<Version, StoreError> {
            let mut state = self.state.lock().unwrap();
            state.resources = resources.to_vec();
            state.version += 1;
            Ok(Version::At(state.version.to_string()))
        }

        async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
            let mut state = self.state.lock().unwrap();
            state.resources.push(resource.clone());
            state.version += 1;
            Ok(())
        }

        async fn delete(&self, url: &str) -> Result<(), StoreError> {
            let mut state = self.state.lock().unwrap();
            state.resources.retain(|r| r.url.to_string() != url);
            state.version += 1;
            Ok(())
        }

        async fn health_check(&self) -> Result<(), StoreError> {
            Ok(())
        }

        fn store_type(&self) -> &'static str {
            "followed"
        }
    }

    /// The freshness half of A4: a replica that does not aggregate still sees
    /// what the owner published, and sees it on the refresh cadence rather than
    /// at its own next restart.
    #[tokio::test]
    async fn a_follower_picks_up_what_the_owner_published() {
        let store = Arc::new(FollowedStore::default());
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);

        let follower = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();
        assert_eq!(follower.count().await, 1);

        // The owner runs a cycle and publishes two more.
        store.publish(vec![
            create_test_resource("https://a.example.com/x", None),
            create_test_resource("https://b.example.com/x", None),
            create_test_resource("https://c.example.com/x", None),
        ]);

        assert_eq!(follower.refresh_from_store().await.unwrap(), Some(3));
        assert_eq!(follower.count().await, 3);

        // ...and a delete propagates too, which a merge-based follower would
        // have missed: the owner's snapshot is authoritative about removals.
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);
        assert_eq!(follower.refresh_from_store().await.unwrap(), Some(1));
        assert_eq!(follower.count().await, 1);
    }

    /// The cheap question is asked every time; the expensive one only when the
    /// answer changed. Without this the follower would pull a 15 MB object
    /// every minute on every replica, which is worse than the duplicated work
    /// it replaced.
    #[tokio::test]
    async fn an_unchanged_catalog_is_not_re_read() {
        let store = Arc::new(FollowedStore::default());
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);

        let follower = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();
        let (_, reads_after_boot) = store.counts();

        for _ in 0..5 {
            assert_eq!(follower.refresh_from_store().await.unwrap(), None);
        }

        let (heads, reads) = store.counts();
        assert_eq!(heads, 5, "every refresh asks the cheap question");
        assert_eq!(
            reads, reads_after_boot,
            "an unchanged catalog must not be downloaded again"
        );
    }

    /// A read that failed says nothing about the catalog. The same rule as A3,
    /// on the other side of it: a failed refresh must leave the copy this task
    /// already serves, not empty it.
    #[tokio::test]
    async fn a_failed_refresh_keeps_the_catalog_it_already_has() {
        let store = Arc::new(FollowedStore::default());
        store.publish(vec![
            create_test_resource("https://a.example.com/x", None),
            create_test_resource("https://b.example.com/x", None),
        ]);

        let follower = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();
        assert_eq!(follower.count().await, 2);

        store.fail_head(true);
        assert!(follower.refresh_from_store().await.is_err());
        assert_eq!(
            follower.count().await,
            2,
            "a failed HEAD emptied the catalog"
        );

        store.fail_head(false);
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);
        store.fail_read(true);
        assert!(follower.refresh_from_store().await.is_err());
        assert_eq!(
            follower.count().await,
            2,
            "a failed GET emptied the catalog"
        );

        // ...and it recovers on the next attempt, without a restart.
        store.fail_read(false);
        assert_eq!(follower.refresh_from_store().await.unwrap(), Some(1));
    }

    /// A registration this task accepted must not disappear because a refresh
    /// landed between the 200 and the write reaching the store. The queue is
    /// drained off the caller's path, so "something is queued" means "not now".
    #[tokio::test]
    async fn a_refresh_never_drops_a_registration_still_in_flight() {
        let store = Arc::new(FollowedStore::default());
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);

        let registry = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();

        // Queue a write without letting the drain run, which is exactly the
        // window between `register` mutating the cache and its store write
        // landing.
        registry
            .writes
            .push(StoreOp::Save(Box::new(create_test_resource(
                "https://just-registered.example.com/x",
                None,
            ))));

        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);
        assert_eq!(
            registry.refresh_from_store().await.unwrap(),
            None,
            "a refresh must stand down while a local write is still in flight"
        );

        // Once the queue drains, refreshing is safe again.
        registry.writes.drain(&registry.store).await;
        assert!(registry.refresh_from_store().await.unwrap().is_some());
    }

    /// The 2.21.2 cap applies to a refresh, not only to a boot load.
    ///
    /// A follower re-reads the whole object every time the owner publishes. If
    /// that path did not trim, a task would re-inhale an oversized catalog on
    /// every refresh — the exact memory the cap exists to bound, reached by the
    /// one code path that runs every minute instead of once per restart. The
    /// oversized object is not hypothetical: it is what is in S3 right now,
    /// written before the cap existed.
    #[tokio::test]
    async fn a_refresh_trims_an_oversized_catalog_the_same_way_a_boot_load_does() {
        std::env::set_var("DISCOVERY_MAX_RESOURCES", "3");

        let store = Arc::new(FollowedStore::default());
        store.publish(vec![create_test_resource(
            "https://seed.example.com/x",
            None,
        )]);
        let follower = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();

        // The owner publishes more than this task may hold. Aggregated copies,
        // which is what `enforce_capacity` is allowed to evict.
        let mut oversized = Vec::new();
        for i in 0..10 {
            let mut r = create_test_resource(&format!("https://big{i}.example.com/x"), None);
            r.source = DiscoverySource::Aggregated;
            r.last_updated = 1_000 + i as u64;
            oversized.push(r);
        }
        store.publish(oversized);

        let held = follower
            .refresh_from_store()
            .await
            .unwrap()
            .expect("the catalog moved");
        std::env::remove_var("DISCOVERY_MAX_RESOURCES");

        assert_eq!(
            held, 3,
            "a refresh loaded {held} records against a cap of 3"
        );
        assert_eq!(follower.count().await, 3);
    }

    /// A task that TAKES the role over must not publish a snapshot computed
    /// from the catalog it had at boot: that would undo everything the previous
    /// owner did. The conditional write refuses it, and the refresher is what
    /// makes the next attempt succeed.
    #[tokio::test]
    async fn a_new_owner_publishes_from_the_catalog_it_refreshed_to() {
        let store = Arc::new(FollowedStore::default());
        store.publish(vec![create_test_resource("https://a.example.com/x", None)]);

        let successor = DiscoveryRegistry::with_store(Arc::clone(&store))
            .await
            .unwrap();

        // The previous owner ran several cycles while this task only served
        // reads.
        for host in ["b", "c", "d"] {
            store.publish(vec![
                create_test_resource("https://a.example.com/x", None),
                create_test_resource(&format!("https://{host}.example.com/x"), None),
            ]);
        }

        // It takes the role over; the refresher has kept it current.
        assert!(successor.refresh_from_store().await.unwrap().is_some());
        assert_eq!(successor.count().await, 2);

        let (added, _, _) = successor
            .bulk_import(
                vec![create_test_resource("https://new.example.com/x", None)],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        assert_eq!(added, 1);
        // Everything the previous owner published is still there.
        assert_eq!(successor.count().await, 3);
    }
}
