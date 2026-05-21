//! Idempotency-Key cache for the `POST /settle` endpoint.
//!
//! Mid-rollout integrators that don't have their own retry-dedup logic
//! (Faremeter handles this client-side; other consumers may not) can leak
//! double-settlement risk on the wire: a network blip between facilitator
//! and client mid-`/settle` plus a naive client retry would normally drive
//! a second on-chain submission. EIP-3009's nonce protection prevents the
//! second submission from actually moving funds (the nonce would be
//! consumed), but the second request still burns RPC quota and may return
//! a confusing error to a caller that doesn't realise the first one
//! succeeded.
//!
//! Stripe-style `Idempotency-Key` header support fixes this: clients tag
//! retries with the same opaque key, and we return the original response
//! verbatim from this cache instead of re-running the settlement.
//!
//! # DynamoDB Schema
//!
//! Table: `idempotency_records` (configurable via `IDEMPOTENCY_TABLE_NAME`)
//!
//! | Attribute       | Type | Description |
//! |-----------------|------|-------------|
//! | idempotency_key | S    | Partition key — opaque caller-supplied string |
//! | request_hash    | S    | SHA-256 hex of the canonical request body |
//! | response_json   | S    | Full SettleResponse JSON to replay |
//! | expires_at      | N    | TTL — Unix seconds (~24h after create) |
//!
//! TTL on `expires_at` lets DynamoDB sweep old records automatically.
//!
//! # Race conditions
//!
//! On-chain replay is already prevented by EIP-3009 nonces, so the cache
//! is a *correctness optimisation* for the response surface rather than a
//! safety primitive. Two concurrent requests with the same key may both
//! find the cache empty and both proceed; the second on-chain submission
//! will fail at nonce-consumption time and the caller sees the failure.
//! A future hardening pass can use conditional writes to serialise the
//! cache lookup.

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

/// Default DDB table name when `IDEMPOTENCY_TABLE_NAME` is unset.
pub const DEFAULT_IDEMPOTENCY_TABLE_NAME: &str = "idempotency_records";

// Global store handle. The /settle handler is generic over the facilitator
// type and routed inside `verify_settle_routes<A>`, where axum's `Handler`
// trait elaboration rejected an `Extension<Arc<dyn IdempotencyStore>>` for
// reasons that resisted Clone/Send/Sync + 'static annotations. Sidestep that
// rabbit hole by following the same pattern as `chain::*::GLOBAL_NONCE_STORE`:
// a process-wide OnceCell populated from `main.rs` at startup.
static GLOBAL_IDEMPOTENCY_STORE: OnceCell<Arc<dyn IdempotencyStore + Send + Sync>> =
    OnceCell::new();

/// Install the process-wide idempotency store. Called once from `main.rs`
/// before the HTTP server starts accepting requests.
pub fn set_global_idempotency_store(store: Arc<dyn IdempotencyStore + Send + Sync>) {
    if GLOBAL_IDEMPOTENCY_STORE.set(store).is_err() {
        warn!("Global idempotency store already initialised; ignoring re-init");
    }
}

/// Helper for the `/settle` handler: look up a record by key on the process
/// global store.
///
/// The handler dispatches this through `tokio::spawn(...).await` so that the
/// outer handler future is `Send` — calling the `dyn IdempotencyStore`
/// method directly from a generic axum handler tripped Handler-trait
/// elaboration on `Pin<Box<dyn Future + Send + 'a>>` from `#[async_trait]`
/// methods, even though the trait object is `Send + Sync` by construction.
pub async fn lookup_record(
    key: String,
) -> Result<Option<IdempotencyRecord>, IdempotencyStoreError> {
    let store = global_idempotency_store();
    store.get(&key).await
}

/// Helper for the `/settle` handler: store a freshly-computed response.
///
/// See [`lookup_record`] for why the caller invokes this through
/// `tokio::spawn`.
pub async fn store_record(record: IdempotencyRecord) -> Result<(), IdempotencyStoreError> {
    let store = global_idempotency_store();
    store.put(record).await
}

/// Return the configured idempotency store. Falls back to the noop store on
/// the cold path (test binaries, unit-test environments) so callers can
/// unconditionally `.get()` / `.put()` without having to branch on
/// "is this initialised?". This is intentionally synchronous so we don't
/// hold a non-`Send` future across the `/settle` handler's await points
/// (axum's `Handler` trait requires the handler future to be `Send`).
pub fn global_idempotency_store() -> Arc<dyn IdempotencyStore + Send + Sync> {
    if let Some(store) = GLOBAL_IDEMPOTENCY_STORE.get() {
        return store.clone();
    }
    // Cold path: nothing was installed (test binary, ad-hoc binary, etc.).
    // We don't attempt to install a default into the OnceCell here because
    // `OnceCell::set` may race with another thread doing the same; just hand
    // back a freshly-allocated noop. Calls that need stable state should
    // call `set_global_idempotency_store` exactly once at startup.
    Arc::new(NoopIdempotencyStore)
}

/// TTL applied to every record (24 hours, per the F4 plan).
pub const IDEMPOTENCY_TTL_SECONDS: u64 = 24 * 60 * 60;

/// Maximum response payload we are willing to cache. Real `SettleResponse`
/// payloads are < 1 KiB; this cap exists so a future bug that tries to
/// cache a giant blob does not blow up DynamoDB item-size limits (400 KiB).
pub const MAX_RESPONSE_JSON_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IdempotencyStoreError {
    #[error("Idempotency storage connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Idempotency record read failed: {0}")]
    ReadError(String),

    #[error("Idempotency record write failed: {0}")]
    WriteError(String),

    #[error("Idempotency record exceeds maximum size: {0} bytes")]
    PayloadTooLarge(usize),

    #[error("Idempotency store not configured")]
    NotConfigured,
}

/// A cached settlement response keyed by the caller-supplied
/// `Idempotency-Key` header value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    pub idempotency_key: String,
    pub request_hash: String,
    pub response_json: String,
    pub expires_at: u64,
}

#[async_trait]
pub trait IdempotencyStore: Send + Sync + std::fmt::Debug {
    /// Look up a previously cached response by idempotency key.
    async fn get(&self, key: &str) -> Result<Option<IdempotencyRecord>, IdempotencyStoreError>;

    /// Cache a settlement response. Overwrites any prior record with the
    /// same key — this is intentional: a "second" client retry that wins
    /// the race after the first one finished is allowed to refresh the
    /// stored payload.
    async fn put(&self, record: IdempotencyRecord) -> Result<(), IdempotencyStoreError>;

    /// Get the store type name for logging.
    fn store_type(&self) -> &'static str;
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compute the canonical request hash. The hash is over the *raw* request
/// body the client sent — we don't try to normalise JSON because the
/// goal is "same bytes from client = same response from us", and a
/// pedantic JSON re-encoding would only mask client bugs (e.g. caller
/// changing the body between retries should yield a hash mismatch).
pub fn hash_request_body(raw_body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw_body);
    let digest = hasher.finalize();
    hex::encode(digest)
}

// ============================================================================
// No-op store (used when no env var is configured)
// ============================================================================

/// Trivial store that reports nothing cached and silently swallows writes.
///
/// Used in local development and CI where the DDB table is not provisioned.
/// `Idempotency-Key` retries simply re-run the settlement, which is exactly
/// the pre-F4 behaviour, so no integration is broken by leaving the table
/// unconfigured.
#[derive(Debug, Default)]
pub struct NoopIdempotencyStore;

#[async_trait]
impl IdempotencyStore for NoopIdempotencyStore {
    async fn get(&self, _key: &str) -> Result<Option<IdempotencyRecord>, IdempotencyStoreError> {
        Ok(None)
    }

    async fn put(&self, _record: IdempotencyRecord) -> Result<(), IdempotencyStoreError> {
        Ok(())
    }

    fn store_type(&self) -> &'static str {
        "noop"
    }
}

// ============================================================================
// DynamoDB store
// ============================================================================

#[derive(Debug)]
pub struct DynamoIdempotencyStore {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
}

impl DynamoIdempotencyStore {
    pub fn new(client: aws_sdk_dynamodb::Client, table_name: String) -> Self {
        info!(table_name = %table_name, "Initialized DynamoDB idempotency store");
        Self { client, table_name }
    }

    /// Build a store from `IDEMPOTENCY_TABLE_NAME` (falling back to
    /// [`DEFAULT_IDEMPOTENCY_TABLE_NAME`]) and the ambient AWS config.
    pub async fn from_env() -> Result<Self, IdempotencyStoreError> {
        let table_name = std::env::var("IDEMPOTENCY_TABLE_NAME")
            .unwrap_or_else(|_| DEFAULT_IDEMPOTENCY_TABLE_NAME.to_string());

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_dynamodb::Client::new(&config);
        Ok(Self::new(client, table_name))
    }
}

#[async_trait]
impl IdempotencyStore for DynamoIdempotencyStore {
    async fn get(&self, key: &str) -> Result<Option<IdempotencyRecord>, IdempotencyStoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let now = current_timestamp();
        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("idempotency_key", AttributeValue::S(key.to_string()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|e| {
                let svc = e.into_service_error();
                error!(error = %svc, key = %key, "DynamoDB idempotency get_item failed");
                IdempotencyStoreError::ReadError(svc.to_string())
            })?;

        let Some(item) = result.item else {
            return Ok(None);
        };

        let expires_at = item
            .get("expires_at")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| {
                IdempotencyStoreError::ReadError(
                    "idempotency record missing expires_at".to_string(),
                )
            })?;

        // DDB TTL is eventually consistent; the item may still be returned
        // after its expiry. Treat post-expiry hits as cache miss so the
        // settlement re-runs cleanly.
        if expires_at <= now {
            debug!(key = %key, "Idempotency record expired");
            return Ok(None);
        }

        let request_hash = item
            .get("request_hash")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| {
                IdempotencyStoreError::ReadError(
                    "idempotency record missing request_hash".to_string(),
                )
            })?;

        let response_json = item
            .get("response_json")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or_else(|| {
                IdempotencyStoreError::ReadError(
                    "idempotency record missing response_json".to_string(),
                )
            })?;

        Ok(Some(IdempotencyRecord {
            idempotency_key: key.to_string(),
            request_hash,
            response_json,
            expires_at,
        }))
    }

    async fn put(&self, record: IdempotencyRecord) -> Result<(), IdempotencyStoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        if record.response_json.len() > MAX_RESPONSE_JSON_BYTES {
            warn!(
                key = %record.idempotency_key,
                size = record.response_json.len(),
                "Idempotency response payload too large to cache; skipping"
            );
            return Err(IdempotencyStoreError::PayloadTooLarge(
                record.response_json.len(),
            ));
        }

        let result = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item(
                "idempotency_key",
                AttributeValue::S(record.idempotency_key.clone()),
            )
            .item(
                "request_hash",
                AttributeValue::S(record.request_hash.clone()),
            )
            .item(
                "response_json",
                AttributeValue::S(record.response_json.clone()),
            )
            .item(
                "expires_at",
                AttributeValue::N(record.expires_at.to_string()),
            )
            .send()
            .await;

        match result {
            Ok(_) => {
                debug!(
                    key = %record.idempotency_key,
                    expires_at = record.expires_at,
                    "Stored idempotency record (DynamoDB)"
                );
                Ok(())
            }
            Err(err) => {
                let service_err = err.into_service_error();
                error!(
                    error = %service_err,
                    key = %record.idempotency_key,
                    "DynamoDB idempotency put_item failed"
                );
                Err(IdempotencyStoreError::WriteError(service_err.to_string()))
            }
        }
    }

    fn store_type(&self) -> &'static str {
        "dynamodb"
    }
}

// ============================================================================
// Factory
// ============================================================================

/// Build the configured idempotency store.
///
/// Falls back to [`NoopIdempotencyStore`] when `IDEMPOTENCY_TABLE_NAME` is
/// unset or DDB initialisation fails, which preserves the pre-F4 behaviour
/// of "no cache, re-run on retry".
pub async fn create_idempotency_store() -> Arc<dyn IdempotencyStore + Send + Sync> {
    match std::env::var("IDEMPOTENCY_TABLE_NAME") {
        Ok(table_name) if !table_name.is_empty() => {
            match DynamoIdempotencyStore::from_env().await {
                Ok(store) => {
                    info!(table_name = %table_name, "Using DynamoDB idempotency store");
                    Arc::new(store) as Arc<dyn IdempotencyStore + Send + Sync>
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to initialize DynamoDB idempotency store; falling back to noop"
                    );
                    Arc::new(NoopIdempotencyStore)
                }
            }
        }
        _ => {
            info!(
                "IDEMPOTENCY_TABLE_NAME unset — Idempotency-Key header retries will re-run settlement"
            );
            Arc::new(NoopIdempotencyStore)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_request_body_is_deterministic() {
        let body = b"{\"foo\": \"bar\"}";
        assert_eq!(hash_request_body(body), hash_request_body(body));
    }

    #[test]
    fn hash_request_body_changes_with_input() {
        assert_ne!(hash_request_body(b"a"), hash_request_body(b"b"));
    }

    #[tokio::test]
    async fn noop_store_returns_none_and_swallows_writes() {
        let store = NoopIdempotencyStore;
        let got = store.get("anything").await.expect("noop get cannot fail");
        assert!(got.is_none());
        let record = IdempotencyRecord {
            idempotency_key: "k".to_string(),
            request_hash: "h".to_string(),
            response_json: "{}".to_string(),
            expires_at: 0,
        };
        store.put(record).await.expect("noop put cannot fail");
    }
}
