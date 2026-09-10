//! Discovery Store abstraction for persistent storage.
//!
//! This module provides a trait-based abstraction for storing Bazaar discovery
//! resources, with implementations for:
//! - In-memory (for testing)
//! - S3 (production MVP)
//!
//! Future implementations can include DynamoDB, PostgreSQL, etc.
//!
//! # Architecture
//!
//! ```text
//! DiscoveryRegistry (in-memory cache for fast reads)
//!        |
//!        v
//! DiscoveryStore (trait) <-- S3Store, MemoryStore, DynamoStore, etc.
//!        |
//!        v
//! Persistent Storage (S3, DynamoDB, PostgreSQL)
//! ```
//!
//! The registry maintains an in-memory cache for fast reads, while the store
//! handles persistence. On startup, the registry loads all resources from the
//! store. On writes, the registry updates both memory and store.

use async_trait::async_trait;
use aws_sdk_s3::error::ProvideErrorMetadata;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::types_v2::DiscoveryResource;

// ============================================================================
// Error Types
// ============================================================================

/// Errors that can occur during store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Failed to connect to storage backend
    #[error("Storage connection failed: {0}")]
    ConnectionFailed(String),

    /// Failed to serialize/deserialize data
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Failed to read from storage
    #[error("Read error: {0}")]
    ReadError(String),

    /// Failed to write to storage
    #[error("Write error: {0}")]
    WriteError(String),

    /// Storage not configured
    #[error("Storage not configured: {0}")]
    NotConfigured(String),

    /// The object moved between the read and the write.
    ///
    /// Distinct from [`StoreError::WriteError`] on purpose: a conflict means
    /// somebody else's write landed first and ours was refused ON PURPOSE, so
    /// the catalog is intact and the right response is to redo the
    /// read-modify-write, not to alarm about a storage failure.
    #[error("Version conflict: {0}")]
    VersionConflict(String),
}

// ============================================================================
// Discovery Store Trait
// ============================================================================

/// Which version of the catalog a read saw.
///
/// The distinction between "there is no object yet" and "the read failed" is
/// the whole of A3. Both used to collapse into an empty `Vec` via
/// `load_all().await.unwrap_or_default()`, and the write that followed replaced
/// the entire catalog with it — so one failed GET could publish an empty
/// catalog over a full one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    /// The object does not exist. A KNOWN base: it is safe to create over.
    Absent,
    /// The object exists at this version (an S3 ETag, or a counter in the
    /// in-memory store). A write must present it or be refused.
    At(String),
}

/// A read of the whole catalog, together with the version it was read at.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub resources: Vec<DiscoveryResource>,
    pub version: Version,
}

/// How many times a read-modify-write retries a version conflict before giving
/// up.
///
/// Bounded. A conflict means somebody else wrote, so retrying re-reads their
/// result and re-applies our own small change on top; three tasks contending
/// resolve in two rounds. An unbounded retry would turn a hot catalog into a
/// livelock.
const MAX_RMW_ATTEMPTS: u32 = 5;

/// Trait for persistent storage of discovery resources.
///
/// Implementations should be thread-safe and handle their own connection pooling.
/// All methods are async to support network-based storage backends.
#[async_trait]
pub trait DiscoveryStore: Send + Sync + std::fmt::Debug {
    /// Load all resources from storage.
    ///
    /// Called on startup to populate the in-memory cache.
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError>;

    /// Load all resources together with the version they were read at.
    ///
    /// The default is for stores that have no version of their own; they answer
    /// [`Version::Absent`], which their [`DiscoveryStore::save_snapshot`]
    /// ignores.
    async fn load_snapshot(&self) -> Result<Snapshot, StoreError> {
        Ok(Snapshot {
            resources: self.load_all().await?,
            version: Version::Absent,
        })
    }

    /// Replace the whole catalog, but only if it is still at `expected`.
    ///
    /// Returns the new version. Returns [`StoreError::VersionConflict`] when
    /// somebody else wrote first — which is a refusal, not a failure: the
    /// catalog is intact and the caller's snapshot is simply stale.
    async fn save_snapshot(
        &self,
        resources: &[DiscoveryResource],
        expected: &Version,
    ) -> Result<Version, StoreError>;

    /// Save a resource to storage.
    ///
    /// This should be idempotent - saving the same resource twice should not fail.
    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError>;

    /// Delete a resource from storage.
    ///
    /// Should not fail if the resource doesn't exist.
    async fn delete(&self, url: &str) -> Result<(), StoreError>;

    /// Publish `resources` AS the catalog: whatever is not in the list is gone.
    ///
    /// Used by the aggregation cycle, which computes the whole desired set from
    /// its own cache — the bulk import and the retention GC both do this, and
    /// the GC's entire purpose is that the result is SMALLER.
    ///
    /// Conditional on the version read immediately before, and a conflict is
    /// returned rather than retried. That is deliberate and is what makes
    /// "a delete does not come back on a late flush" true: a snapshot computed
    /// from a stale cache is refused outright, where a retry would re-apply it
    /// over the newer catalog and resurrect exactly what was removed. The
    /// caller redoes its cycle from a fresh read.
    async fn save_all(&self, resources: &[DiscoveryResource]) -> Result<(), StoreError> {
        let base = self.load_snapshot().await?;
        self.save_snapshot(resources, &base.version).await?;
        Ok(())
    }

    /// Check if the store is healthy and accessible.
    async fn health_check(&self) -> Result<(), StoreError>;

    /// Get the store type name for logging.
    fn store_type(&self) -> &'static str;
}

/// Read the catalog, apply `change`, write it back conditionally, and redo the
/// whole thing if somebody else got there first.
///
/// `change` runs against a FRESH read on every attempt. That is the point: it
/// re-applies our small edit on top of the winner's catalog, rather than
/// re-publishing a base that is now stale.
///
/// A free function rather than a method so the loop can be exercised against a
/// store that fails and conflicts on demand. Every fault this has to survive --
/// a failed read, a lost race, a writer that keeps winning -- is invisible from
/// the outside of an S3 client.
pub async fn read_modify_write<S, F>(store: &S, what: &str, mut change: F) -> Result<(), StoreError>
where
    S: DiscoveryStore + ?Sized,
    F: FnMut(&mut Vec<DiscoveryResource>),
{
    let mut last_conflict = String::new();
    for attempt in 0..MAX_RMW_ATTEMPTS {
        // `?`, not `unwrap_or_default()`. A read we could not complete tells us
        // nothing about the catalog, and a write built on nothing erases it.
        let base = store.load_snapshot().await?;
        let mut catalog = base.resources;
        change(&mut catalog);

        match store.save_snapshot(&catalog, &base.version).await {
            Ok(_) => return Ok(()),
            Err(StoreError::VersionConflict(code)) => {
                last_conflict = code;
                // Jittered, so contending tasks do not re-collide in step.
                let backoff = {
                    use rand::Rng as _;
                    let base_ms = 5u64 << attempt.min(4);
                    base_ms + rand::thread_rng().gen_range(0..=base_ms / 2)
                };
                debug!(
                    what,
                    attempt = attempt + 1,
                    backoff_ms = backoff,
                    "catalog moved under a read-modify-write; retrying against the new base"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }
            Err(e) => return Err(e),
        }
    }
    // Bounded: report rather than spin. The catalog is intact -- every attempt
    // was REFUSED, none was half-applied -- and the caller logs it.
    Err(StoreError::VersionConflict(format!(
        "{what} lost {MAX_RMW_ATTEMPTS} conditional writes in a row (last: {last_conflict})"
    )))
}

/// Fold one resource into a catalog, without walking a version backwards.
///
/// A save that was issued before a newer one, but arrives after it — a retried
/// read-modify-write, a late fire-and-forget task — must not replace the newer
/// record with its own. `last_updated` is the registry's own ordering, so a
/// strictly older one is dropped.
///
/// Returns whether the catalog changed.
pub fn merge_resource(catalog: &mut Vec<DiscoveryResource>, incoming: &DiscoveryResource) -> bool {
    let url = incoming.url.to_string();
    match catalog.iter_mut().find(|r| r.url.to_string() == url) {
        Some(existing) => {
            if existing.last_updated > incoming.last_updated {
                debug!(
                    url = %url,
                    stored = existing.last_updated,
                    incoming = incoming.last_updated,
                    "refusing to overwrite a newer catalog entry with an older one"
                );
                return false;
            }
            *existing = incoming.clone();
            true
        }
        None => {
            catalog.push(incoming.clone());
            true
        }
    }
}

// ============================================================================
// In-Memory Store (for testing)
// ============================================================================

/// In-memory store implementation for testing.
///
/// Does not persist data across restarts.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Arc<RwLock<MemoryState>>,
}

/// The catalog and its version, under one lock.
///
/// Versioned like the S3 object, so a test can exercise the same conditional
/// write path without an AWS account — and so `save_all` means the same thing
/// in both: REPLACE, not merge. It used to mean "merge" here (the default trait
/// body called `save` per resource), which made the retention GC a no-op
/// against this store: the whole point of that call is that what is missing
/// from the list is deleted.
#[derive(Debug, Default)]
struct MemoryState {
    resources: Vec<DiscoveryResource>,
    /// `0` means the object does not exist yet, matching [`Version::Absent`].
    version: u64,
}

impl MemoryState {
    fn version(&self) -> Version {
        if self.version == 0 {
            Version::Absent
        } else {
            Version::At(self.version.to_string())
        }
    }
}

impl MemoryStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MemoryState::default())),
        }
    }
}

#[async_trait]
impl DiscoveryStore for MemoryStore {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        Ok(self.inner.read().await.resources.clone())
    }

    async fn load_snapshot(&self) -> Result<Snapshot, StoreError> {
        let state = self.inner.read().await;
        Ok(Snapshot {
            resources: state.resources.clone(),
            version: state.version(),
        })
    }

    async fn save_snapshot(
        &self,
        resources: &[DiscoveryResource],
        expected: &Version,
    ) -> Result<Version, StoreError> {
        let mut state = self.inner.write().await;
        let current = state.version();
        if &current != expected {
            return Err(StoreError::VersionConflict(format!(
                "catalog is at {current:?}, write expected {expected:?}"
            )));
        }
        state.resources = resources.to_vec();
        state.version += 1;
        Ok(state.version())
    }

    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
        let mut state = self.inner.write().await;
        let mut catalog = std::mem::take(&mut state.resources);
        merge_resource(&mut catalog, resource);
        state.resources = catalog;
        state.version += 1;
        Ok(())
    }

    async fn delete(&self, url: &str) -> Result<(), StoreError> {
        let mut state = self.inner.write().await;
        state.resources.retain(|r| r.url.to_string() != url);
        state.version += 1;
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "memory"
    }
}

// ============================================================================
// S3 Store
// ============================================================================

/// S3-based persistent store for discovery resources.
///
/// Stores all resources as a single JSON file in S3 for simplicity.
/// This is optimized for small to medium registries (< 1000 resources).
///
/// # Configuration
///
/// Requires the following environment variables:
/// - `DISCOVERY_S3_BUCKET`: S3 bucket name
/// - `DISCOVERY_S3_KEY`: S3 object key (default: "bazaar/resources.json")
/// - `AWS_REGION`: AWS region (or uses default from environment)
///
/// # Thread Safety
///
/// Uses a local cache to minimize S3 reads. Writes are atomic (single PUT).
#[derive(Debug)]
pub struct S3Store {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
    /// Serializes this process's own read-modify-write cycles.
    ///
    /// The ETag makes concurrent writers safe; this makes them cheap. Without
    /// it, three requests arriving together on the same task each read the same
    /// base, two lose the conditional write and both retry — correct, but three
    /// round trips of contention this process could simply have avoided.
    ///
    /// It is NOT a substitute for the conditional write: there are three tasks,
    /// and a mutex is local to one.
    writes: tokio::sync::Mutex<()>,
}

impl S3Store {
    /// Create a new S3 store with explicit configuration.
    pub fn new(client: aws_sdk_s3::Client, bucket: String, key: String) -> Self {
        info!(
            bucket = %bucket,
            key = %key,
            "Initialized S3 discovery store"
        );
        Self {
            client,
            bucket,
            key,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    /// Create a new S3 store from environment variables.
    ///
    /// # Environment Variables
    ///
    /// - `DISCOVERY_S3_BUCKET` (required): S3 bucket name
    /// - `DISCOVERY_S3_KEY` (optional): Object key, defaults to "bazaar/resources.json"
    pub async fn from_env() -> Result<Self, StoreError> {
        let bucket = std::env::var("DISCOVERY_S3_BUCKET").map_err(|_| {
            StoreError::NotConfigured("DISCOVERY_S3_BUCKET environment variable not set".into())
        })?;

        let key = std::env::var("DISCOVERY_S3_KEY")
            .unwrap_or_else(|_| "bazaar/resources.json".to_string());

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);

        Ok(Self::new(client, bucket, key))
    }

    /// Serialize resources to JSON bytes.
    fn serialize(resources: &[DiscoveryResource]) -> Result<Vec<u8>, StoreError> {
        serde_json::to_vec_pretty(resources)
            .map_err(|e| StoreError::SerializationError(e.to_string()))
    }

    /// Deserialize resources from JSON bytes.
    fn deserialize(data: &[u8]) -> Result<Vec<DiscoveryResource>, StoreError> {
        serde_json::from_slice(data).map_err(|e| StoreError::SerializationError(e.to_string()))
    }
}

#[async_trait]
impl DiscoveryStore for S3Store {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        Ok(self.load_snapshot().await?.resources)
    }

    async fn load_snapshot(&self) -> Result<Snapshot, StoreError> {
        debug!(bucket = %self.bucket, key = %self.key, "Loading resources from S3");

        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .send()
            .await;

        match result {
            Ok(output) => {
                // Read the version BEFORE the body: `collect()` consumes
                // `output`, and an ETag taken afterwards would have to come
                // from somewhere else.
                let version = output
                    .e_tag()
                    .filter(|tag| !tag.is_empty())
                    .map(|tag| Version::At(tag.to_string()))
                    // S3 always returns an ETag on a successful GET. If one
                    // ever did not, the write path would attempt
                    // create-if-absent against an object that exists and be
                    // REFUSED -- visibly, and without overwriting anything.
                    // That is the right way round to fail.
                    .unwrap_or(Version::Absent);

                let body = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError::ReadError(e.to_string()))?;

                let resources = Self::deserialize(&body.into_bytes())?;
                info!(
                    count = resources.len(),
                    "Loaded discovery resources from S3"
                );
                Ok(Snapshot { resources, version })
            }
            Err(sdk_err) => {
                let service_err = sdk_err.into_service_error();
                // The ONLY error that means "empty catalog". Everything else --
                // AccessDenied, NoSuchBucket, a throttle, a timeout -- is a
                // failed READ, and a failed read must never become an empty
                // catalog that the next write then publishes.
                if service_err.is_no_such_key() {
                    info!("No existing discovery data in S3, starting fresh");
                    return Ok(Snapshot {
                        resources: Vec::new(),
                        version: Version::Absent,
                    });
                }
                error!(error = %service_err, "Failed to load from S3");
                Err(StoreError::ReadError(service_err.to_string()))
            }
        }
    }

    async fn save_snapshot(
        &self,
        resources: &[DiscoveryResource],
        expected: &Version,
    ) -> Result<Version, StoreError> {
        debug!(
            bucket = %self.bucket,
            key = %self.key,
            count = resources.len(),
            ?expected,
            "Saving resources to S3"
        );

        let body = Self::serialize(resources)?;

        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .body(body.into())
            .content_type("application/json");

        // S3 conditional writes. `If-Match` refuses the PUT unless the object
        // is still the one we read; `If-None-Match: *` refuses it unless the
        // object still does not exist. Either way a concurrent writer loses one
        // of the two, instead of both silently overwriting each other.
        request = match expected {
            Version::At(etag) => request.if_match(etag.clone()),
            Version::Absent => request.if_none_match("*"),
        };

        match request.send().await {
            Ok(output) => {
                let version = output
                    .e_tag()
                    .filter(|tag| !tag.is_empty())
                    .map(|tag| Version::At(tag.to_string()))
                    .unwrap_or(Version::Absent);
                info!(count = resources.len(), "Saved discovery resources to S3");
                Ok(version)
            }
            Err(sdk_err) => {
                let code = sdk_err.code().unwrap_or_default().to_string();
                let service_err = sdk_err.into_service_error();
                if is_conditional_refusal(&code) {
                    warn!(
                        code = %code,
                        ?expected,
                        "S3 refused the catalog write: it moved since we read it"
                    );
                    return Err(StoreError::VersionConflict(code));
                }
                Err(StoreError::WriteError(service_err.to_string()))
            }
        }
    }

    /// Add or replace ONE resource, without touching the rest of the catalog.
    ///
    /// Read-modify-write, and every part of that is now load-bearing:
    ///
    /// * The read is `?`, not `unwrap_or_default()`. A failed GET used to
    ///   become an empty catalog which the PUT below then published over a full
    ///   one — one transient S3 error was enough to erase the Bazaar.
    /// * The merge refuses to walk `last_updated` backwards, so a retry that
    ///   re-reads a newer record does not overwrite it with the older one it
    ///   set out with.
    /// * The write is conditional on the version read, and a conflict re-runs
    ///   the whole cycle against the winner's catalog. That is what makes two
    ///   concurrent registrations BOTH survive, where the previous full
    ///   overwrite kept whichever landed last.
    /// * The mutex serializes this process's own writers, so three requests
    ///   arriving together cost one round of conflicts rather than three.
    async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
        let _serialized = self.writes.lock().await;
        read_modify_write(self, "save", |catalog| {
            merge_resource(catalog, resource);
        })
        .await
    }

    async fn delete(&self, url: &str) -> Result<(), StoreError> {
        let _serialized = self.writes.lock().await;
        read_modify_write(self, "delete", |catalog| {
            catalog.retain(|r| r.url.to_string() != url);
        })
        .await
    }

    async fn save_all(&self, resources: &[DiscoveryResource]) -> Result<(), StoreError> {
        // Deliberately NOT the retry loop. See the trait's doc comment: a
        // snapshot is authoritative about deletions, so re-applying a stale one
        // over a newer catalog resurrects what was removed. One attempt against
        // the version read immediately before, and a conflict is reported.
        let _serialized = self.writes.lock().await;
        let base = self.load_snapshot().await?;
        match self.save_snapshot(resources, &base.version).await {
            Ok(_) => Ok(()),
            Err(StoreError::VersionConflict(code)) => {
                warn!(
                    code = %code,
                    count = resources.len(),
                    "catalog snapshot not published: it was computed from a base that has since \
                     moved. Republishing it would undo the writes that moved it, including \
                     deletions. The next aggregation cycle recomputes it from a fresh read"
                );
                Err(StoreError::VersionConflict(code))
            }
            Err(e) => Err(e),
        }
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        // Try to head the bucket to check connectivity
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| StoreError::ConnectionFailed(e.to_string()))?;
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "s3"
    }
}

/// Whether S3 refused a PUT because of the condition we attached.
///
/// `PreconditionFailed` is `If-Match` against a version that moved, or
/// `If-None-Match: *` against an object that now exists.
/// `ConditionalRequestConflict` is S3 telling us another conditional write to
/// the same key was in flight. Both mean "somebody else wrote"; neither means
/// the catalog is damaged.
fn is_conditional_refusal(code: &str) -> bool {
    matches!(code, "PreconditionFailed" | "ConditionalRequestConflict")
}

// ============================================================================
// No-Op Store (for when persistence is disabled)
// ============================================================================

/// No-op store that doesn't persist anything.
///
/// Use this when persistence is not configured or not needed.
#[derive(Debug, Default)]
pub struct NoOpStore;

impl NoOpStore {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DiscoveryStore for NoOpStore {
    async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
        Ok(Vec::new())
    }

    async fn save_snapshot(
        &self,
        _resources: &[DiscoveryResource],
        _expected: &Version,
    ) -> Result<Version, StoreError> {
        Ok(Version::Absent)
    }

    async fn save(&self, _resource: &DiscoveryResource) -> Result<(), StoreError> {
        Ok(())
    }

    async fn delete(&self, _url: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn health_check(&self) -> Result<(), StoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "noop"
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;
    use crate::types::{MixedAddress, Scheme, TokenAmount};
    use crate::types_v2::PaymentRequirementsV2;
    use url::Url;

    fn create_test_resource(url: &str) -> DiscoveryResource {
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

        DiscoveryResource::new(
            Url::parse(url).unwrap(),
            "http".to_string(),
            "Test resource".to_string(),
            accepts,
        )
    }

    #[tokio::test]
    async fn test_memory_store_save_and_load() {
        let store = MemoryStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].url.to_string(), "https://api.example.com/data");
    }

    #[tokio::test]
    async fn test_memory_store_delete() {
        let store = MemoryStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 1);

        store.delete("https://api.example.com/data").await.unwrap();
        assert_eq!(store.load_all().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_memory_store_save_all() {
        let store = MemoryStore::new();

        let resources = vec![
            create_test_resource("https://api1.example.com/data"),
            create_test_resource("https://api2.example.com/data"),
            create_test_resource("https://api3.example.com/data"),
        ];

        store.save_all(&resources).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[tokio::test]
    async fn test_noop_store() {
        let store = NoOpStore::new();

        let resource = create_test_resource("https://api.example.com/data");
        store.save(&resource).await.unwrap();

        // NoOp store doesn't persist anything
        let loaded = store.load_all().await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_store_health_check() {
        let memory_store = MemoryStore::new();
        assert!(memory_store.health_check().await.is_ok());

        let noop_store = NoOpStore::new();
        assert!(noop_store.health_check().await.is_ok());
    }

    // =======================================================================
    // A3: a failed read must not become an empty catalog
    // =======================================================================
    //
    // `S3Store::save` and `S3Store::delete` were
    // `load_all().await.unwrap_or_default()` followed by an unconditional PUT
    // of the whole object. Two consequences, both of them data loss:
    //
    //   * a GET that failed for ANY reason -- a throttle, a timeout, an IAM
    //     change -- became an empty `Vec`, and the PUT then published it over
    //     the real catalog;
    //   * two processes doing that at once each wrote the catalog they read,
    //     so whichever landed second silently erased the other's registration.
    //
    // The store below is the same conditional-write contract S3 implements,
    // plus the ability to fail a read and to let somebody else win a race.

    /// A versioned catalog with faults on demand.
    #[derive(Debug, Default)]
    struct FlakyStore {
        state: std::sync::Mutex<FlakyState>,
    }

    #[derive(Debug, Default)]
    struct FlakyState {
        resources: Vec<DiscoveryResource>,
        version: u64,
        /// Reads still to fail before one succeeds.
        fail_reads: u32,
        /// Writes still to refuse as conflicts, as a rival writer would cause.
        conflict_writes: u32,
        /// Counters, so a test can say how many attempts were made.
        reads: u32,
        writes: u32,
    }

    impl FlakyStore {
        fn with_resources(resources: Vec<DiscoveryResource>) -> Self {
            Self {
                state: std::sync::Mutex::new(FlakyState {
                    resources,
                    version: 1,
                    ..FlakyState::default()
                }),
            }
        }

        fn fail_next_reads(&self, n: u32) {
            self.state.lock().unwrap().fail_reads = n;
        }

        fn conflict_next_writes(&self, n: u32) {
            self.state.lock().unwrap().conflict_writes = n;
        }

        fn urls(&self) -> Vec<String> {
            let mut urls: Vec<String> = self
                .state
                .lock()
                .unwrap()
                .resources
                .iter()
                .map(|r| r.url.to_string())
                .collect();
            urls.sort();
            urls
        }

        fn writes(&self) -> u32 {
            self.state.lock().unwrap().writes
        }
    }

    #[async_trait]
    impl DiscoveryStore for FlakyStore {
        async fn load_all(&self) -> Result<Vec<DiscoveryResource>, StoreError> {
            Ok(self.load_snapshot().await?.resources)
        }

        async fn load_snapshot(&self) -> Result<Snapshot, StoreError> {
            let mut state = self.state.lock().unwrap();
            state.reads += 1;
            if state.fail_reads > 0 {
                state.fail_reads -= 1;
                return Err(StoreError::ReadError("injected read failure".into()));
            }
            Ok(Snapshot {
                resources: state.resources.clone(),
                version: Version::At(state.version.to_string()),
            })
        }

        async fn save_snapshot(
            &self,
            resources: &[DiscoveryResource],
            expected: &Version,
        ) -> Result<Version, StoreError> {
            let mut state = self.state.lock().unwrap();
            state.writes += 1;
            // A rival writer landed between our read and our write, exactly as
            // S3 reports with `PreconditionFailed`.
            if state.conflict_writes > 0 {
                state.conflict_writes -= 1;
                state.version += 1;
                return Err(StoreError::VersionConflict("PreconditionFailed".into()));
            }
            if &Version::At(state.version.to_string()) != expected {
                return Err(StoreError::VersionConflict("PreconditionFailed".into()));
            }
            state.resources = resources.to_vec();
            state.version += 1;
            Ok(Version::At(state.version.to_string()))
        }

        async fn save(&self, resource: &DiscoveryResource) -> Result<(), StoreError> {
            read_modify_write(self, "save", |catalog| {
                merge_resource(catalog, resource);
            })
            .await
        }

        async fn delete(&self, url: &str) -> Result<(), StoreError> {
            read_modify_write(self, "delete", |catalog| {
                catalog.retain(|r| r.url.to_string() != url);
            })
            .await
        }

        async fn health_check(&self) -> Result<(), StoreError> {
            Ok(())
        }

        fn store_type(&self) -> &'static str {
            "flaky"
        }
    }

    /// THE regression. A read that failed used to become an empty catalog, and
    /// the write that followed published it.
    #[tokio::test]
    async fn a_failed_read_never_writes() {
        let store = FlakyStore::with_resources(vec![
            create_test_resource("https://a.example.com/x"),
            create_test_resource("https://b.example.com/x"),
        ]);
        store.fail_next_reads(MAX_RMW_ATTEMPTS + 1);

        let err = store
            .save(&create_test_resource("https://c.example.com/x"))
            .await
            .expect_err("a save built on a failed read must not succeed");
        assert!(matches!(err, StoreError::ReadError(_)), "{err:?}");
        assert_eq!(
            store.writes(),
            0,
            "nothing may be written on an unknown base"
        );
        assert_eq!(store.urls().len(), 2, "the catalog is untouched");
    }

    /// The same for a delete: an unreadable catalog is not an empty one.
    #[tokio::test]
    async fn a_failed_read_never_deletes_the_catalog() {
        let store = FlakyStore::with_resources(vec![
            create_test_resource("https://a.example.com/x"),
            create_test_resource("https://b.example.com/x"),
        ]);
        store.fail_next_reads(1);

        assert!(store.delete("https://a.example.com/x").await.is_err());
        assert_eq!(store.writes(), 0);
        assert_eq!(store.urls().len(), 2);
    }

    /// Two registrations racing: both must survive. The previous full overwrite
    /// kept whichever landed last and dropped the other without a trace.
    #[tokio::test]
    async fn two_concurrent_registrations_both_survive() {
        let store = Arc::new(FlakyStore::default());
        // The first write of each save loses, as it would against a rival that
        // committed in between; the retry re-reads and re-applies on top.
        store.conflict_next_writes(1);

        let a = Arc::clone(&store);
        let b = Arc::clone(&store);
        let first = tokio::spawn(async move {
            a.save(&create_test_resource("https://first.example.com/x"))
                .await
        });
        let second = tokio::spawn(async move {
            b.save(&create_test_resource("https://second.example.com/x"))
                .await
        });
        first.await.unwrap().expect("first save");
        second.await.unwrap().expect("second save");

        assert_eq!(
            store.urls(),
            vec![
                "https://first.example.com/x".to_string(),
                "https://second.example.com/x".to_string()
            ],
            "a conflict must re-apply on the winner's catalog, not replace it"
        );
    }

    /// A conflicted write is re-applied on top of the base that won, so nothing
    /// already stored is lost.
    #[tokio::test]
    async fn a_conflicted_write_rebases_instead_of_replacing() {
        let store = FlakyStore::with_resources(vec![create_test_resource(
            "https://existing.example.com/x",
        )]);
        store.conflict_next_writes(2);

        store
            .save(&create_test_resource("https://new.example.com/x"))
            .await
            .expect("the retry succeeds");

        assert_eq!(
            store.urls(),
            vec![
                "https://existing.example.com/x".to_string(),
                "https://new.example.com/x".to_string()
            ]
        );
        assert_eq!(store.writes(), 3, "two refusals and one commit");
    }

    /// Contention has a floor. Losing every attempt reports a conflict rather
    /// than spinning, and the catalog is intact because every attempt was
    /// REFUSED rather than half-applied.
    #[tokio::test]
    async fn endless_contention_gives_up_without_damage() {
        let store = FlakyStore::with_resources(vec![create_test_resource(
            "https://existing.example.com/x",
        )]);
        store.conflict_next_writes(MAX_RMW_ATTEMPTS + 5);

        let err = store
            .save(&create_test_resource("https://new.example.com/x"))
            .await
            .expect_err("every attempt lost");
        assert!(matches!(err, StoreError::VersionConflict(_)), "{err:?}");
        assert_eq!(store.writes(), MAX_RMW_ATTEMPTS);
        assert_eq!(
            store.urls(),
            vec!["https://existing.example.com/x".to_string()]
        );
    }

    /// A snapshot computed from a base that has since moved is REFUSED, not
    /// retried. Retrying would republish a catalog that still contains what the
    /// newer one deleted -- which is precisely "the delete came back".
    #[tokio::test]
    async fn a_late_snapshot_flush_does_not_resurrect_a_deleted_resource() {
        let doomed = create_test_resource("https://doomed.example.com/x");
        let store = MemoryStore::new();
        store.save(&doomed).await.unwrap();
        store
            .save(&create_test_resource("https://keeper.example.com/x"))
            .await
            .unwrap();

        // An aggregation cycle reads the catalog and starts computing.
        let stale = store.load_snapshot().await.unwrap();
        assert_eq!(stale.resources.len(), 2);

        // Meanwhile the resource is deleted.
        store.delete("https://doomed.example.com/x").await.unwrap();

        // The cycle now flushes what it computed. It must not land.
        let err = store
            .save_snapshot(&stale.resources, &stale.version)
            .await
            .expect_err("a stale snapshot must be refused");
        assert!(matches!(err, StoreError::VersionConflict(_)), "{err:?}");

        let urls: Vec<String> = store
            .load_all()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.to_string())
            .collect();
        assert_eq!(urls, vec!["https://keeper.example.com/x".to_string()]);
    }

    /// ...and `save_all`, which is what the aggregation cycle actually calls,
    /// behaves the same way rather than quietly retrying.
    #[tokio::test]
    async fn save_all_reports_a_conflict_instead_of_clobbering() {
        let store =
            FlakyStore::with_resources(vec![create_test_resource("https://keeper.example.com/x")]);
        store.conflict_next_writes(1);

        let err = store
            .save_all(&[create_test_resource("https://stale.example.com/x")])
            .await
            .expect_err("a conflicted snapshot is refused");
        assert!(matches!(err, StoreError::VersionConflict(_)), "{err:?}");
        assert_eq!(
            store.urls(),
            vec!["https://keeper.example.com/x".to_string()],
            "the newer catalog stands"
        );
    }

    /// A merge must not walk a version backwards. A retried read-modify-write
    /// re-reads a record that may be NEWER than the one it set out to write.
    #[tokio::test]
    async fn a_merge_never_replaces_a_newer_record_with_an_older_one() {
        let url = "https://api.example.com/data";
        let mut newer = create_test_resource(url);
        newer.last_updated = 2_000;
        let mut older = create_test_resource(url);
        older.last_updated = 1_000;
        older.description = "the older copy".to_string();

        let mut catalog = vec![newer.clone()];
        assert!(
            !merge_resource(&mut catalog, &older),
            "an older record must not be applied"
        );
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].last_updated, 2_000);
        assert_ne!(catalog[0].description, "the older copy");

        // The other direction still applies, and an equal timestamp is treated
        // as an idempotent re-save rather than a rollback.
        assert!(merge_resource(&mut catalog, &newer));
        let mut newest = create_test_resource(url);
        newest.last_updated = 3_000;
        assert!(merge_resource(&mut catalog, &newest));
        assert_eq!(catalog[0].last_updated, 3_000);
    }

    /// `save_all` means REPLACE in every store, which is what the retention GC
    /// depends on. It used to mean "merge" for `MemoryStore` (the default trait
    /// body called `save` per resource), so a GC against it removed nothing.
    #[tokio::test]
    async fn save_all_publishes_deletions() {
        let store = MemoryStore::new();
        store
            .save(&create_test_resource("https://gone.example.com/x"))
            .await
            .unwrap();
        store
            .save(&create_test_resource("https://kept.example.com/x"))
            .await
            .unwrap();

        store
            .save_all(&[create_test_resource("https://kept.example.com/x")])
            .await
            .unwrap();

        let urls: Vec<String> = store
            .load_all()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.to_string())
            .collect();
        assert_eq!(urls, vec!["https://kept.example.com/x".to_string()]);
    }

    /// An object that does not exist yet is a KNOWN base, and the one case
    /// where creating over it is right. A failed read is not.
    #[tokio::test]
    async fn an_absent_object_is_a_known_base() {
        let store = MemoryStore::new();
        let snapshot = store.load_snapshot().await.unwrap();
        assert_eq!(snapshot.version, Version::Absent);
        assert!(snapshot.resources.is_empty());

        store
            .save_snapshot(
                &[create_test_resource("https://first.example.com/x")],
                &Version::Absent,
            )
            .await
            .expect("creating over an absent object is allowed");

        // ...and not twice: the object exists now.
        let err = store
            .save_snapshot(
                &[create_test_resource("https://second.example.com/x")],
                &Version::Absent,
            )
            .await
            .expect_err("create-if-absent must not overwrite");
        assert!(matches!(err, StoreError::VersionConflict(_)), "{err:?}");
    }

    /// The two codes S3 uses to refuse a conditional write, and nothing else.
    #[test]
    fn only_a_conditional_refusal_counts_as_a_conflict() {
        assert!(is_conditional_refusal("PreconditionFailed"));
        assert!(is_conditional_refusal("ConditionalRequestConflict"));
        for other in [
            "AccessDenied",
            "NoSuchBucket",
            "SlowDown",
            "InternalError",
            "RequestTimeout",
            "",
        ] {
            assert!(
                !is_conditional_refusal(other),
                "{other} is a failure, not a refusal, and must not be retried as one"
            );
        }
    }
}
