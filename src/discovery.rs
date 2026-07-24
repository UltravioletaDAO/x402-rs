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

use crate::discovery_store::{DiscoveryStore, NoOpStore, StoreError};
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
    /// Curated tier manifest (WS-C).
    curation: Arc<crate::discovery_curation::CurationManifest>,
    /// On-chain reputation cache (WS-E), keyed by resource URL.
    reputation: Arc<RwLock<HashMap<String, crate::types_v2::VerificationInfo>>>,
    /// Hosted attestation evidence bodies (WS-E), keyed by sha256(url) hex.
    evidence: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl Clone for DiscoveryRegistry {
    fn clone(&self) -> Self {
        Self {
            resources: Arc::clone(&self.resources),
            store: Arc::clone(&self.store),
            health: Arc::clone(&self.health),
            curation: Arc::clone(&self.curation),
            reputation: Arc::clone(&self.reputation),
            evidence: Arc::clone(&self.evidence),
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
            curation: Arc::new(crate::discovery_curation::CurationManifest::load()),
            reputation: Arc::new(RwLock::new(HashMap::new())),
            evidence: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// The liveness overlay (WS-B). Used by the health prober and by `list()`.
    pub fn health(&self) -> Arc<crate::discovery_health::HealthTracker> {
        Arc::clone(&self.health)
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

    /// Snapshot of every registered resource URL (for the health prober).
    pub async fn all_urls(&self) -> Vec<url::Url> {
        self.resources
            .read()
            .await
            .values()
            .map(|r| r.url.clone())
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

        // Load existing resources from store
        let existing = store.load_all().await?;
        let count = existing.len();

        // Populate cache
        let mut cache = HashMap::new();
        for resource in existing {
            cache.insert(resource.url.to_string(), resource);
        }

        info!(
            store_type = store_type,
            loaded_count = count,
            "Loaded discovery resources from persistent storage"
        );

        Ok(Self {
            resources: Arc::new(RwLock::new(cache)),
            store: Arc::new(store),
            health: Arc::new(crate::discovery_health::HealthTracker::new()),
            curation: Arc::new(crate::discovery_curation::CurationManifest::load()),
            reputation: Arc::new(RwLock::new(HashMap::new())),
            evidence: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get the store type for diagnostics.
    pub fn store_type(&self) -> &'static str {
        self.store.store_type()
    }

    /// Persist a resource to the store asynchronously.
    ///
    /// This spawns a background task to avoid blocking the caller.
    fn persist_async(&self, resource: DiscoveryResource) {
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            if let Err(e) = store.save(&resource).await {
                error!(
                    url = %resource.url,
                    error = %e,
                    "Failed to persist resource to store"
                );
            }
        });
    }

    /// Delete a resource from the store asynchronously.
    fn delete_from_store_async(&self, url: String) {
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            if let Err(e) = store.delete(&url).await {
                error!(url = %url, error = %e, "Failed to delete resource from store");
            }
        });
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
        let reputation = self.reputation.read().await.clone();
        let health_filter = filters.as_ref().and_then(|f| f.health.clone());
        let tier_filter = filters.as_ref().and_then(|f| f.tier.clone());

        let resources = self.resources.read().await;

        // Cap limit at 100 to prevent abuse
        let limit = limit.min(100);

        // Filter (user filters + suppression + health visibility), then resolve
        // each survivor's curated tier for ordering + annotation.
        let mut scored: Vec<(&DiscoveryResource, Option<CurationInfo>)> = resources
            .values()
            .filter(|r| self.matches_filters(r, &filters))
            .filter(|r| !self.curation.is_suppressed(&r.url))
            .filter(|r| health_visible(&health, r.url.as_str(), health_filter.as_deref()))
            .map(|r| {
                let alive = health
                    .get(r.url.as_str())
                    .map(|h| h.status == HealthStatus::Alive)
                    .unwrap_or(false);
                let mut cur = self.curation.resolve(&r.url, alive);
                if let Some(c) = cur.as_mut() {
                    c.verification = reputation.get(r.url.as_str()).cloned();
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
                c
            })
            .collect();

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

        for resource in resources {
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

            if let Some(existing) = cache.get(&url_key) {
                if resource.last_updated > existing.last_updated {
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
                    merged.source = match existing.source {
                        DiscoverySource::SelfRegistered | DiscoverySource::Settlement => {
                            existing.source
                        }
                        _ => merged.source,
                    };
                    cache.insert(url_key, merged);
                    updated += 1;
                } else {
                    skipped += 1;
                }
            } else {
                cache.insert(url_key, resource);
                added += 1;
            }
        }

        // Persist the FULL cache as one snapshot (single PUT) rather than
        // per-item read-modify-write. This avoids the S3 race where a stale
        // per-item save would re-add items the retention GC just removed.
        let changed = added + updated;
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
            if let Err(e) = self.store.save_all(&snapshot).await {
                error!(error = %e, "Failed to persist bulk import snapshot");
            } else {
                info!(count = n, "Persisted bulk import snapshot to store");
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
            if let Err(e) = self.store.save_all(&keep).await {
                error!(error = %e, "Failed to persist retention GC snapshot");
            } else {
                info!(kept = kept, "Retention GC snapshot persisted");
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
            // Resource exists - increment settlement count
            existing.increment_settlement_count();
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
        }];

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

    fn junk_empty_accepts(url: &str) -> DiscoveryResource {
        DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "d".to_string(),
            vec![],
        )
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
        }];

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
        }];

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
}
