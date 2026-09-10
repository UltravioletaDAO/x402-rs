//! Demand-driven revalidation of catalog prices.
//!
//! # The budget is the design
//!
//! On 2026-09-10 this service spent four hours down because background work
//! scaled with a catalog that had grown 39x. The fix (2.21.2) capped the catalog
//! at 2 000 records and the prober at 2 requests a second — 120 probes a tick —
//! and production came back to 1-3 % CPU and 0,03 s reads.
//!
//! So this phase adds refresh **inside that budget, never on top of it**. A
//! revalidation request does not schedule an extra probe; it changes *which*
//! probe the next tick spends its allowance on. The periodic sweep gets whatever
//! the demand queue does not use, and a reserved share it can always use
//! ([`long_tail_share`]) so a busy resource cannot starve a quiet one forever.
//!
//! That is the whole safety argument, and it is why the acceptance test for this
//! phase is a number that did NOT move: probes per tick.
//!
//! # Who runs it
//!
//! A4 gave the periodic work a single owner, elected by lease. The queue is
//! drained by that owner and by nobody else. But demand arrives on whichever
//! replica the load balancer picked, so a non-owner has to be able to hand a
//! request over.
//!
//! It does that through a **persisted set in the lease table** — the same table,
//! the same key schema, the same IAM statement — and not through a new
//! mechanism. One item, one DynamoDB string set, `ADD` semantics:
//!
//! * `ADD` of a value already in the set is a no-op, so **the deduplication is
//!   the storage engine's**, fleet-wide, with no lock and no scan. Ten replicas
//!   asking for the same stale record ten thousand times produce one entry.
//! * The owner claims a batch with a read and a `DELETE` of exactly the values
//!   it took, so anything added in between survives the claim.
//! * The item carries a TTL, so a queue nobody drains disappears instead of
//!   growing.
//!
//! Every part of this is best-effort. A request that cannot be recorded is lost,
//! and losing it costs a slower refresh — the periodic sweep still comes round.
//! Nothing here may fail a listing, and nothing here may fail a payment.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::discovery_config as cfg;

/// Why a resource is being revalidated. Ordered: higher is more urgent.
///
/// The order is a claim about what a wrong price costs, not about how recently
/// we heard something. A buyer about to sign is the only case where a stale
/// number turns into a wrong payment inside the next few seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefreshReason {
    /// The periodic sweep would have got to it eventually.
    Periodic,
    /// A listing was served whose observation had aged out.
    ListingStale,
    /// The catalog record changed after the last reading, so the reading
    /// describes a revision that no longer exists.
    RevisionChanged,
    /// A current reading disagrees with what the catalog publishes.
    Conflict,
    /// The owner of the resource told us its price moved.
    OwnerNotified,
    /// Somebody is about to buy this.
    PurchaseIntent,
}

impl RefreshReason {
    /// Kebab identifier for logs and metrics. Bounded vocabulary.
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshReason::Periodic => "periodic",
            RefreshReason::ListingStale => "listing-stale",
            RefreshReason::RevisionChanged => "revision-changed",
            RefreshReason::Conflict => "conflict",
            RefreshReason::OwnerNotified => "owner-notified",
            RefreshReason::PurchaseIntent => "purchase-intent",
        }
    }

    fn weight(self) -> u32 {
        match self {
            RefreshReason::Periodic => 0,
            RefreshReason::ListingStale => 10,
            RefreshReason::RevisionChanged => 25,
            RefreshReason::Conflict => 40,
            RefreshReason::OwnerNotified => 60,
            RefreshReason::PurchaseIntent => 100,
        }
    }
}

/// What a caller asking for current terms should be told.
///
/// The point of this type is that "we are working on it" and "this is current"
/// are different answers, and the cache must never be served as the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevalidationState {
    /// Nothing is pending; what you see is what we last read.
    Idle,
    /// A refresh is queued or running. The terms shown are the previous reading.
    Pending,
    /// We cannot check this one by observation. See [`NotVerifiable`].
    NotVerifiable(NotVerifiable),
}

/// Why a resource's price cannot be verified by probing it.
///
/// A resource we cannot check is not a resource that is broken, and it is not
/// one that is free. It is one whose price we have no standing to assert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotVerifiable {
    /// The purchase is a POST, or is parameterised. An unauthenticated GET of
    /// the same URL is a different request and may carry a different price --
    /// and firing the real one blind would be starting somebody's commercial
    /// operation to see what it costs.
    NotAGetResource,
    /// The origin wants credentials before it will quote. The prober has none
    /// and will not acquire any.
    AuthGated,
    /// The URL is one the SSRF connector refuses (template, private, bad port).
    Unprobeable,
}

impl NotVerifiable {
    pub fn as_str(self) -> &'static str {
        match self {
            NotVerifiable::NotAGetResource => "not-a-get-resource",
            NotVerifiable::AuthGated => "auth-gated",
            NotVerifiable::Unprobeable => "unprobeable",
        }
    }
}

impl RevalidationState {
    pub fn as_str(self) -> &'static str {
        match self {
            RevalidationState::Idle => "idle",
            RevalidationState::Pending => "pending",
            RevalidationState::NotVerifiable(_) => "not_verifiable",
        }
    }
}

/// One resource waiting to be revalidated.
#[derive(Debug, Clone)]
struct Pending {
    url: String,
    /// Strongest reason seen while this entry has been waiting.
    reason: RefreshReason,
    /// How many distinct requests coalesced into this entry. Demand, bounded so
    /// a hot resource cannot outscore every reason there is.
    demand: u32,
    first_requested_at: u64,
}

impl Pending {
    /// Higher runs first.
    ///
    /// Reason dominates and demand breaks ties, deliberately: ten thousand
    /// people reading a listing is evidence of interest, not of the price being
    /// wrong, and one buyer about to sign outranks all of them.
    fn score(&self, now: u64) -> u64 {
        let demand = self.demand.min(cfg::revalidation_demand_cap()) as u64;
        let waited = now.saturating_sub(self.first_requested_at) / 60;
        u64::from(self.reason.weight()) * 1_000 + demand * 10 + waited.min(100)
    }
}

/// Per-host politeness and failure state.
#[derive(Debug, Default, Clone)]
struct HostState {
    /// Probes issued to this host in the current tick.
    spent_this_tick: usize,
    /// Unix seconds before which this host must not be touched again.
    hold_until: u64,
    /// Consecutive refusals (429 / 5xx / transport), for the backoff schedule.
    strikes: u32,
}

/// The demand queue.
pub struct RevalidationQueue {
    /// Resources waiting, keyed by URL.
    pending: RwLock<HashMap<String, Pending>>,
    /// URL -> last time we accepted a request for it. The coalescing window.
    recent: RwLock<HashMap<String, u64>>,
    /// Per-host budget and backoff.
    hosts: RwLock<HashMap<String, HostState>>,
    /// Cross-replica hand-off, when DynamoDB is configured.
    shared: RwLock<Option<SharedQueue>>,
}

impl Default for RevalidationQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl RevalidationQueue {
    pub fn new() -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            recent: RwLock::new(HashMap::new()),
            hosts: RwLock::new(HashMap::new()),
            shared: RwLock::new(None),
        }
    }

    /// Attach the cross-replica hand-off.
    pub async fn configure_shared(&self, client: aws_sdk_dynamodb::Client, table: String) {
        *self.shared.write().await = Some(SharedQueue { client, table });
    }

    /// Ask for `url` to be revalidated.
    ///
    /// Returns whether the request was accepted as new work. `false` means an
    /// identical request is already in flight for this window, which is the
    /// common case and the point: many reads of one stale record produce one
    /// job.
    ///
    /// Never blocks on the network. The cross-replica hand-off is spawned.
    pub async fn request(&self, url: &str, reason: RefreshReason, now: u64) -> bool {
        let window = cfg::revalidation_window_secs();
        {
            let recent = self.recent.read().await;
            if let Some(at) = recent.get(url) {
                if now.saturating_sub(*at) < window {
                    // Already asked for inside the window. Still record the
                    // stronger reason and the extra demand, because that is
                    // ordering information and it is free.
                    drop(recent);
                    let mut pending = self.pending.write().await;
                    if let Some(entry) = pending.get_mut(url) {
                        entry.reason = entry.reason.max(reason);
                        entry.demand = entry.demand.saturating_add(1);
                    }
                    return false;
                }
            }
        }

        {
            let mut pending = self.pending.write().await;
            if pending.len() >= cfg::revalidation_queue_cap() && !pending.contains_key(url) {
                // Bounded. A queue that grows without limit is the same failure
                // as a catalog that grows without limit, one layer along.
                return false;
            }
            let entry = pending.entry(url.to_string()).or_insert_with(|| Pending {
                url: url.to_string(),
                reason,
                demand: 0,
                first_requested_at: now,
            });
            entry.reason = entry.reason.max(reason);
            entry.demand = entry.demand.saturating_add(1);
        }
        self.recent.write().await.insert(url.to_string(), now);
        true
    }

    /// Hand a request to whichever replica owns the periodic work.
    ///
    /// Fire-and-forget on purpose: this runs off a public read path, and a
    /// listing must never wait on DynamoDB. A request that does not make it
    /// costs a slower refresh, and the periodic sweep still comes round.
    /// # Why this takes a batch and not a URL
    ///
    /// It took a URL, and one listing page can carry a hundred stale records, so
    /// one public read produced a hundred spawned tasks and a hundred DynamoDB
    /// writes. Worse, it did not self-limit: once the shared set reaches its cap
    /// the conditional write starts FAILING, and a hundred failing writes cost
    /// exactly what a hundred succeeding ones do.
    ///
    /// A string set takes many values in one `ADD`, so a page is one write. Same
    /// lesson as the rest of 2026-09-10: work on a read path must not scale with
    /// the size of the catalog.
    pub fn offer_to_owner(self: &Arc<Self>, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        let queue = Arc::clone(self);
        tokio::spawn(async move {
            let guard = queue.shared.read().await;
            let Some(shared) = guard.as_ref() else {
                return;
            };
            for chunk in chunks_for_offer(&urls) {
                if let Err(e) = shared.offer_many(&chunk).await {
                    debug!(
                        count = chunk.len(),
                        error = %e,
                        "could not hand revalidation requests to the owner"
                    );
                    // One refusal is enough: the rest of this page would be
                    // refused for the same reason, and retrying it here is the
                    // amplification just removed.
                    return;
                }
            }
        });
    }

    /// Take everything the other replicas asked for and fold it in.
    ///
    /// Owner-only. Called once per tick, before the batch is chosen.
    pub async fn absorb_shared(&self, now: u64) -> usize {
        let claimed = {
            let guard = self.shared.read().await;
            match guard.as_ref() {
                Some(shared) => match shared.claim(cfg::revalidation_claim_max()).await {
                    Ok(urls) => urls,
                    Err(e) => {
                        debug!(error = %e, "could not claim revalidation requests");
                        Vec::new()
                    }
                },
                None => Vec::new(),
            }
        };
        let mut folded = 0;
        for url in claimed {
            if self.request(&url, RefreshReason::ListingStale, now).await {
                folded += 1;
            }
        }
        folded
    }

    /// Choose up to `budget` resources to probe now.
    ///
    /// Highest score first, subject to the per-host allowance and to any host
    /// currently in backoff. Chosen entries leave the queue: a probe that fails
    /// re-enters through the ordinary path rather than being retried in place,
    /// which is what keeps a permanently broken origin from occupying the front
    /// of the queue forever.
    pub async fn take_batch(&self, budget: usize, now: u64) -> Vec<(String, RefreshReason)> {
        if budget == 0 {
            return Vec::new();
        }
        let per_host = cfg::revalidation_per_host_per_tick();
        let mut hosts = self.hosts.write().await;
        for state in hosts.values_mut() {
            state.spent_this_tick = 0;
        }

        let mut pending = self.pending.write().await;
        let mut ranked: Vec<Pending> = pending.values().cloned().collect();
        ranked.sort_by_key(|p| std::cmp::Reverse(p.score(now)));

        let mut chosen = Vec::new();
        for entry in ranked {
            if chosen.len() >= budget {
                break;
            }
            let host = host_of(&entry.url);
            let state = hosts.entry(host).or_default();
            if state.hold_until > now {
                continue;
            }
            if state.spent_this_tick >= per_host {
                continue;
            }
            state.spent_this_tick += 1;
            pending.remove(&entry.url);
            chosen.push((entry.url, entry.reason));
        }
        chosen
    }

    /// Record that an origin refused us, and hold off its host.
    ///
    /// `retry_after` is the origin's own instruction and wins when it is present
    /// and sane. Otherwise the schedule doubles with jitter, so a host that
    /// starts failing does not collect every replica's retry at the same instant.
    pub async fn note_refusal(&self, url: &str, retry_after: Option<Duration>, now: u64) {
        let mut hosts = self.hosts.write().await;
        let state = hosts.entry(host_of(url)).or_default();
        state.strikes = state.strikes.saturating_add(1);
        let base = match retry_after {
            Some(d) if d.as_secs() > 0 => d.as_secs().min(cfg::revalidation_max_backoff_secs()),
            _ => {
                let step = cfg::revalidation_base_backoff_secs()
                    .saturating_mul(1u64 << state.strikes.min(6));
                step.min(cfg::revalidation_max_backoff_secs())
            }
        };
        let jitter = {
            use rand::Rng as _;
            rand::thread_rng().gen_range(0..=base / 4 + 1)
        };
        state.hold_until = now + base + jitter;
        debug!(
            host = %host_of(url),
            strikes = state.strikes,
            hold_secs = base + jitter,
            from_origin = retry_after.is_some(),
            "holding off a host that refused"
        );
    }

    /// Record that an origin answered normally, clearing its backoff.
    pub async fn note_success(&self, url: &str) {
        let mut hosts = self.hosts.write().await;
        if let Some(state) = hosts.get_mut(&host_of(url)) {
            state.strikes = 0;
            state.hold_until = 0;
        }
    }

    /// Whether a refresh is queued for `url`.
    pub async fn is_pending(&self, url: &str) -> bool {
        self.pending.read().await.contains_key(url)
    }

    /// A snapshot of everything queued, for annotating a listing without taking
    /// the queue lock per item.
    pub async fn pending_snapshot(&self) -> std::collections::HashSet<String> {
        self.pending.read().await.keys().cloned().collect()
    }

    pub async fn depth(&self) -> usize {
        self.pending.read().await.len()
    }

    /// Drop coalescing entries that have aged out, so the map does not grow with
    /// every URL ever requested.
    pub async fn evict_expired(&self, now: u64) {
        let window = cfg::revalidation_window_secs();
        self.recent
            .write()
            .await
            .retain(|_, at| now.saturating_sub(*at) < window * 4);
        let cap = cfg::revalidation_queue_cap();
        let mut pending = self.pending.write().await;
        if pending.len() > cap {
            let mut ranked: Vec<(String, u64)> = pending
                .iter()
                .map(|(u, p)| (u.clone(), p.score(now)))
                .collect();
            ranked.sort_by_key(|(_, s)| *s);
            let over = pending.len() - cap;
            for (url, _) in ranked.into_iter().take(over) {
                pending.remove(&url);
            }
        }
    }
}

/// Split one tick's probe allowance between demand and the periodic sweep.
///
/// The invariant this whole phase rests on, in one function so it can be
/// asserted rather than believed: **the two halves add up to the allowance that
/// already existed.** Demand-driven refresh changes which resources a tick
/// probes; it never increases how many.
///
/// The reserved half is what stops a busy resource from holding the entire
/// budget forever. Starvation of the long tail would not show up as an incident
/// -- it would show up as a catalog whose quiet corners silently stopped being
/// checked, which is worse.
pub fn split_budget(max_per_tick: usize, long_tail_percent: u64) -> (usize, usize) {
    let pct = long_tail_percent.min(100) as usize;
    let reserved = max_per_tick * pct / 100;
    (max_per_tick.saturating_sub(reserved), reserved)
}

/// Most URLs to put in one `ADD`.
///
/// A DynamoDB item is capped at 400 KB and a catalog URL runs to a couple of
/// hundred bytes, so a hundred per write is an order of magnitude inside the
/// limit while turning a page of listings into one round trip.
const MAX_URLS_PER_OFFER: usize = 100;

/// Split a batch into writes, dropping duplicates.
fn chunks_for_offer(urls: &[String]) -> Vec<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<String> = urls
        .iter()
        .filter(|u| seen.insert((*u).clone()))
        .cloned()
        .collect();
    unique
        .chunks(MAX_URLS_PER_OFFER)
        .map(|c| c.to_vec())
        .collect()
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// The cross-replica hand-off: one DynamoDB item holding a string set.
struct SharedQueue {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

/// Partition key of the shared queue item, in the lease table.
const QUEUE_KEY: &str = "discovery-revalidate#pending";

impl SharedQueue {
    /// Add one URL to the shared set.
    ///
    /// `ADD` on a string set is idempotent, which is the deduplication: a value
    /// already there costs one write and changes nothing. The condition bounds
    /// the item so a stampede cannot grow it towards DynamoDB's 400 KB limit.
    async fn offer_many(&self, urls: &[String]) -> Result<(), String> {
        use aws_sdk_dynamodb::types::AttributeValue;
        if urls.is_empty() {
            return Ok(());
        }
        let ttl = now_secs() + cfg::revalidation_shared_ttl_secs();
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(QUEUE_KEY.to_string()))
            .update_expression("ADD pending :url SET expires_at = :ttl")
            .condition_expression("attribute_not_exists(pending) OR size(pending) < :cap")
            .expression_attribute_values(":url", AttributeValue::Ss(urls.to_vec()))
            .expression_attribute_values(":ttl", AttributeValue::N(ttl.to_string()))
            .expression_attribute_values(
                ":cap",
                AttributeValue::N(cfg::revalidation_shared_cap().to_string()),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Take up to `max` URLs, removing exactly those.
    ///
    /// Read then conditional remove OF THE VALUES TAKEN, not a delete of the
    /// item: anything another replica added between the two calls stays.
    async fn claim(&self, max: usize) -> Result<Vec<String>, String> {
        use aws_sdk_dynamodb::types::AttributeValue;
        let got = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(QUEUE_KEY.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let urls: Vec<String> = got
            .item()
            .and_then(|i| i.get("pending"))
            .and_then(|v| v.as_ss().ok())
            .map(|ss| ss.iter().take(max).cloned().collect())
            .unwrap_or_default();

        if urls.is_empty() {
            return Ok(Vec::new());
        }

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(QUEUE_KEY.to_string()))
            .update_expression("DELETE pending :taken")
            .expression_attribute_values(":taken", AttributeValue::Ss(urls.clone()))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Ok(urls)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse a `Retry-After` header's delta-seconds form.
///
/// Two deliberate narrowings. A value we cannot read is `None` rather than zero:
/// "the origin said something we did not understand" must not become "come back
/// immediately". And the HTTP-date form is not parsed at all -- `None` falls
/// through to the exponential schedule, which waits LONGER than a date would,
/// so the failure direction is politeness rather than a stampede. Adding a date
/// parser means adding a dependency, and it would buy a shorter wait.
pub fn parse_retry_after(raw: &str) -> Option<Duration> {
    let secs = raw.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(
        secs.min(cfg::revalidation_max_backoff_secs()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://a.example/one";
    const B: &str = "https://a.example/two";
    const C: &str = "https://other.example/three";

    fn queue() -> RevalidationQueue {
        RevalidationQueue::new()
    }

    #[tokio::test]
    async fn many_reads_of_one_stale_record_make_one_job() {
        // The acceptance criterion of this phase, stated as a test. A listing
        // page served a thousand times enqueues the same resource a thousand
        // times; what the prober must see is one.
        let q = queue();
        let now = 1_000;
        assert!(q.request(A, RefreshReason::ListingStale, now).await);
        for i in 1..1_000 {
            assert!(
                !q.request(A, RefreshReason::ListingStale, now + i % 60)
                    .await,
                "a repeat inside the window must not be new work"
            );
        }
        assert_eq!(q.depth().await, 1);
        let batch = q.take_batch(10, now).await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, A);
    }

    #[tokio::test]
    async fn the_window_expires_so_a_resource_can_be_asked_for_again() {
        let q = queue();
        assert!(q.request(A, RefreshReason::ListingStale, 1_000).await);
        q.take_batch(10, 1_000).await;
        let later = 1_000 + cfg::revalidation_window_secs() + 1;
        assert!(
            q.request(A, RefreshReason::ListingStale, later).await,
            "coalescing is a window, not a permanent ban"
        );
    }

    #[tokio::test]
    async fn a_repeat_still_raises_the_reason_and_the_demand() {
        // Deduplication must not lose ordering information: the second caller
        // may be about to buy.
        let q = queue();
        assert!(q.request(A, RefreshReason::ListingStale, 1_000).await);
        assert!(!q.request(A, RefreshReason::PurchaseIntent, 1_010).await);
        let pending = q.pending.read().await;
        let entry = pending.get(A).unwrap();
        assert_eq!(entry.reason, RefreshReason::PurchaseIntent);
        assert_eq!(entry.demand, 2);
    }

    #[tokio::test]
    async fn a_buyer_outranks_a_thousand_readers() {
        let q = queue();
        let now = 1_000;
        q.request(B, RefreshReason::ListingStale, now).await;
        for _ in 0..500 {
            q.request(B, RefreshReason::ListingStale, now).await;
        }
        q.request(C, RefreshReason::PurchaseIntent, now).await;

        let batch = q.take_batch(1, now).await;
        assert_eq!(
            batch[0].0, C,
            "demand is interest; a purchase is a wrong payment in the next second"
        );
    }

    #[tokio::test]
    async fn one_host_cannot_take_the_whole_batch() {
        // Politeness toward a mega-host: two listings of the same origin in one
        // tick, however many of its resources are stale.
        let q = queue();
        let now = 1_000;
        for i in 0..20 {
            q.request(
                &format!("https://a.example/{i}"),
                RefreshReason::ListingStale,
                now,
            )
            .await;
        }
        q.request(C, RefreshReason::ListingStale, now).await;

        let batch = q.take_batch(50, now).await;
        let from_a = batch
            .iter()
            .filter(|(u, _)| u.contains("a.example"))
            .count();
        assert_eq!(from_a, cfg::revalidation_per_host_per_tick());
        assert!(
            batch.iter().any(|(u, _)| u == C),
            "and another host is not blocked by the busy one"
        );
    }

    #[tokio::test]
    async fn an_origin_that_refuses_is_not_asked_again_this_tick() {
        // A 429 must not produce an avalanche: the host is held off, and the
        // budget goes to somebody else instead of being spent on retries.
        let q = queue();
        let now = 1_000;
        q.note_refusal(A, Some(Duration::from_secs(120)), now).await;
        q.request(A, RefreshReason::ListingStale, now).await;
        q.request(C, RefreshReason::ListingStale, now).await;

        let batch = q.take_batch(10, now).await;
        assert!(
            !batch.iter().any(|(u, _)| u == A),
            "a host in backoff is skipped"
        );
        assert!(batch.iter().any(|(u, _)| u == C));

        // ... and it comes back when the hold expires.
        q.request(A, RefreshReason::ListingStale, now + 200).await;
        let later = q.take_batch(10, now + 200).await;
        assert!(later.iter().any(|(u, _)| u == A));
    }

    #[tokio::test]
    async fn the_origins_own_retry_after_is_respected_over_our_schedule() {
        let q = queue();
        let now = 1_000;
        q.note_refusal(A, Some(Duration::from_secs(30)), now).await;
        let hold = q.hosts.read().await.get("a.example").unwrap().hold_until;
        assert!(
            (1_030..=1_040).contains(&hold),
            "30 s asked for, plus jitter, not our 60 s default: got {hold}"
        );
    }

    #[tokio::test]
    async fn repeated_refusals_back_off_further_each_time() {
        let q = queue();
        q.note_refusal(A, None, 1_000).await;
        let first = q.hosts.read().await.get("a.example").unwrap().hold_until - 1_000;
        q.note_refusal(A, None, 1_000).await;
        let second = q.hosts.read().await.get("a.example").unwrap().hold_until - 1_000;
        assert!(second > first, "{second} should exceed {first}");
    }

    #[tokio::test]
    async fn a_successful_probe_clears_the_backoff() {
        let q = queue();
        q.note_refusal(A, None, 1_000).await;
        q.note_success(A).await;
        let state = q.hosts.read().await.get("a.example").cloned().unwrap();
        assert_eq!(state.strikes, 0);
        assert_eq!(state.hold_until, 0);
    }

    #[tokio::test]
    async fn the_queue_is_bounded() {
        let q = queue();
        let cap = cfg::revalidation_queue_cap();
        for i in 0..(cap + 200) {
            q.request(
                &format!("https://h{i}.example/x"),
                RefreshReason::ListingStale,
                1_000,
            )
            .await;
        }
        assert!(
            q.depth().await <= cap,
            "a queue that grows without limit is the same failure as a catalog that does"
        );
    }

    #[tokio::test]
    async fn taking_a_batch_removes_it_from_the_queue() {
        // A probe that fails re-enters through the ordinary path. Leaving it at
        // the front would let one permanently broken origin hold the queue.
        let q = queue();
        q.request(A, RefreshReason::ListingStale, 1_000).await;
        assert!(q.is_pending(A).await);
        let batch = q.take_batch(10, 1_000).await;
        assert_eq!(batch.len(), 1);
        assert!(!q.is_pending(A).await);
        assert_eq!(q.depth().await, 0);
    }

    #[test]
    fn retry_after_reads_delta_seconds_and_refuses_anything_else() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("  30 "), Some(Duration::from_secs(30)));
        // An HTTP date is not parsed: `None` falls through to the exponential
        // schedule, which waits longer. Failing toward politeness is the point.
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("soon"), None);
        // And an absurd value is clamped rather than believed.
        assert_eq!(
            parse_retry_after("99999999"),
            Some(Duration::from_secs(cfg::revalidation_max_backoff_secs()))
        );
    }

    #[test]
    fn a_page_of_stale_listings_is_one_write_not_a_hundred() {
        // The read path must not scale with the catalog. Handing a page over one
        // record at a time was a hundred spawned tasks and a hundred writes per
        // public read, and it did not self-limit: at the shared cap those become
        // a hundred FAILING writes, which cost the same.
        let page: Vec<String> = (0..100)
            .map(|i| format!("https://h{i}.example/x"))
            .collect();
        let chunks = chunks_for_offer(&page);
        assert_eq!(chunks.len(), 1, "one page, one round trip");
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn a_batch_beyond_one_item_is_split_rather_than_refused() {
        let many: Vec<String> = (0..250)
            .map(|i| format!("https://h{i}.example/x"))
            .collect();
        let chunks = chunks_for_offer(&many);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 250);
    }

    #[test]
    fn duplicates_inside_a_batch_never_reach_the_wire() {
        let dupes: Vec<String> = std::iter::repeat(A.to_string()).take(50).collect();
        let chunks = chunks_for_offer(&dupes);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1);
    }

    #[test]
    fn an_empty_batch_is_no_write_at_all() {
        assert!(chunks_for_offer(&[]).is_empty());
    }

    #[test]
    fn reasons_are_ordered_by_what_a_wrong_price_costs() {
        assert!(RefreshReason::PurchaseIntent > RefreshReason::OwnerNotified);
        assert!(RefreshReason::OwnerNotified > RefreshReason::Conflict);
        assert!(RefreshReason::Conflict > RefreshReason::RevisionChanged);
        assert!(RefreshReason::RevisionChanged > RefreshReason::ListingStale);
        assert!(RefreshReason::ListingStale > RefreshReason::Periodic);
    }

    #[test]
    fn the_two_halves_of_the_budget_are_the_budget() {
        // 2.21.2 took production from 57 % memory and 5 s reads back to 18 % and
        // 0,03 s by cutting the prober to 120 probes a tick. This phase must
        // spend that same 120, differently -- never 121.
        for total in [0usize, 1, 7, 120, 1_200] {
            for pct in [0u64, 40, 100] {
                let (demand, reserved) = split_budget(total, pct);
                assert_eq!(
                    demand + reserved,
                    total,
                    "budget {total} at {pct}% split into {demand}+{reserved}"
                );
            }
        }
    }

    #[test]
    fn the_reserved_share_is_never_the_whole_budget_at_the_default() {
        let (demand, reserved) =
            split_budget(cfg::health_budget_per_tick(), cfg::long_tail_share());
        assert!(demand > 0, "demand must be able to spend something");
        assert!(reserved > 0, "and the long tail must never be starved");
    }

    #[test]
    fn an_absurd_share_cannot_overspend_the_budget() {
        let (demand, reserved) = split_budget(120, 250);
        assert_eq!(reserved, 120);
        assert_eq!(demand, 0);
    }

    #[tokio::test]
    async fn a_zero_budget_takes_nothing() {
        let q = queue();
        q.request(A, RefreshReason::PurchaseIntent, 1_000).await;
        assert!(q.take_batch(0, 1_000).await.is_empty());
        assert!(q.is_pending(A).await, "and nothing is lost");
    }
}
