//! Durable storage for sealed evidence.
//!
//! The store only ever sees ciphertext. Privacy is a property of the envelope
//! (`super::envelope`), not of the backend, which is why the same bytes can go
//! to S3, IPFS or Arweave and the guarantee does not change. That is deliberate:
//! it means a deployment can migrate backends -- or a seller can pick a
//! different one per route -- without touching the protocol.
//!
//! # Retention is a promise, not a lock
//!
//! `retention` states how long retrieval is *guaranteed*. It is not a claim that
//! the bytes are destroyed the instant it lapses, and nothing here should be
//! read as one. Anchoring is publishing; see the security notes in the spec.

use async_trait::async_trait;
use thiserror::Error;

use super::types::{DurablePointer, Retention, StorageBackend};

#[derive(Debug, Error)]
pub enum StoreError {
    /// The backend was unreachable or refused the operation. Retryable.
    #[error("evidence store unavailable: {0}")]
    Unavailable(String),
    #[error("evidence not found")]
    NotFound,
    /// Past `retentionUntil`. Distinct from `NotFound` because "it expired" and
    /// "it never existed" are different answers to a dispute.
    #[error("evidence expired")]
    Expired,
    #[error("backend {0} is not configured on this deployment")]
    NotConfigured(StorageBackend),
    #[error("pointer does not belong to this store: {0}")]
    ForeignPointer(String),
}

impl StoreError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, StoreError::Unavailable(_))
    }
}

/// A place sealed evidence can live.
#[async_trait]
pub trait EvidenceStore: Send + Sync + std::fmt::Debug {
    fn backend(&self) -> StorageBackend;

    /// Store `blob` under `payment_id`.
    async fn put(
        &self,
        payment_id: &str,
        blob: &[u8],
        retention: Retention,
    ) -> Result<StoredObject, StoreError>;

    /// Delete a stored object, by the reference `put` handed back.
    ///
    /// This is what makes retention real on a backend that has no lifecycle
    /// rule doing it for us. S3 expires by bucket policy and ignores this;
    /// Pinata does not expire anything on its own, so without this the
    /// `retentionUntil` in a receipt we SIGNED would never come true.
    async fn delete(&self, _reference: &str) -> Result<(), StoreError> {
        Ok(())
    }

    /// Retrieve the sealed blob a pointer refers to.
    async fn get(&self, pointer: &DurablePointer) -> Result<Vec<u8>, StoreError>;

    /// The pointer this store WOULD issue, without writing anything.
    ///
    /// Lets the caller reserve the registry slot before uploading any bytes.
    /// The write order is load-bearing: see the note in `Dx402Service::anchor`.
    ///
    /// `blob` is passed because some backends name by CONTENT rather than by
    /// payment -- an IPFS CID is a hash of the bytes, so it cannot be derived
    /// from the paymentId alone. Backends that name by payment ignore it.
    fn pointer_for(&self, payment_id: &str, blob: &[u8]) -> DurablePointer;
}

/// Where a stored object landed, and how to address it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    /// What a buyer dereferences.
    pub pointer: DurablePointer,
    /// Which store actually took the bytes.
    ///
    /// A different question from [`EvidenceStore::backend`], which a composed
    /// store answers with its PRIMARY whatever happens. After a fallback write
    /// the two disagree, and the record was keeping the wrong one: `ipfs` for
    /// evidence sitting in S3.
    pub backend: StorageBackend,
    /// Backend-specific handle for deletion, when the backend needs one.
    ///
    /// A private IPFS pointer names the PAYMENT, not the object, so it cannot
    /// be turned back into something Pinata will delete. Without persisting
    /// this, retention on that backend is a promise with no mechanism.
    pub reference: Option<String>,
}

impl StoredObject {
    /// A write the backend needs no handle to undo.
    ///
    /// There is deliberately no `From<DurablePointer>` any more. A conversion
    /// that cannot know the backend is what made dropping that fact the path of
    /// least resistance at the one call site obliged to keep it.
    pub fn new(pointer: DurablePointer, backend: StorageBackend) -> Self {
        Self {
            pointer,
            backend,
            reference: None,
        }
    }
}

/// S3-backed store. The default: no external dependency, no per-file cost, and
/// retention enforced by a bucket lifecycle rule rather than by us.
#[derive(Debug, Clone)]
pub struct S3EvidenceStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    /// Public base URL for reads, so a pointer is dereferenceable by a buyer who
    /// has no AWS credentials.
    public_base: String,
}

impl S3EvidenceStore {
    pub fn new(client: aws_sdk_s3::Client, bucket: String, public_base: String) -> Self {
        Self {
            client,
            bucket,
            public_base: public_base.trim_end_matches('/').to_string(),
        }
    }

    pub async fn from_env(bucket: String, public_base: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::new(aws_sdk_s3::Client::new(&config), bucket, public_base)
    }

    fn object_key(payment_id: &str) -> String {
        format!("evidence/{payment_id}.dx402")
    }

    /// The pointer a buyer receives, which addresses the *payment*, not the
    /// bucket layout.
    ///
    /// The bucket stays private and the pointer resolves through the
    /// facilitator's own `GET /dx402/blob/{paymentId}`. Serving ciphertext this
    /// way costs one hop and buys two things: no publicly-readable bucket to
    /// misconfigure, and freedom to reorganise the S3 key layout later without
    /// breaking pointers that buyers already hold — and those pointers are meant
    /// to still work in a year.
    ///
    /// It is safe precisely because the bytes behind it are sealed to the payer:
    /// an unauthenticated GET hands out ciphertext nobody else can open.
    fn pointer_for_payment_id(&self, payment_id: &str) -> DurablePointer {
        DurablePointer(format!("s3+{}/{}", self.public_base, payment_id))
    }

    /// Recover the object key from a pointer we previously issued.
    ///
    /// Rejects anything not under our own `public_base`. Without that check this
    /// would dereference attacker-chosen URLs through our credentials, which is
    /// an SSRF.
    fn key_from_pointer(&self, pointer: &DurablePointer) -> Result<String, StoreError> {
        let raw = pointer.as_str();
        let rest = raw
            .strip_prefix("s3+")
            .ok_or_else(|| StoreError::ForeignPointer(raw.to_string()))?;
        let payment_id = rest
            .strip_prefix(&self.public_base)
            .map(|p| p.trim_start_matches('/'))
            .filter(|p| !p.is_empty() && !p.contains('/'))
            .ok_or_else(|| StoreError::ForeignPointer(raw.to_string()))?;
        Ok(Self::object_key(payment_id))
    }
}

#[async_trait]
impl EvidenceStore for S3EvidenceStore {
    fn pointer_for(&self, payment_id: &str, _blob: &[u8]) -> DurablePointer {
        self.pointer_for_payment_id(payment_id)
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    async fn put(
        &self,
        payment_id: &str,
        blob: &[u8],
        retention: Retention,
    ) -> Result<StoredObject, StoreError> {
        let key = Self::object_key(payment_id);
        let mut req = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(blob.to_vec().into())
            .content_type("application/octet-stream");

        // Tag rather than set an per-object expiry: lifecycle rules on the
        // bucket do the deleting, which keeps the policy in one auditable place
        // instead of scattered across millions of objects.
        req = req.tagging(format!("dx402-retention={retention}"));

        req.send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("s3 put_object: {e}")))?;

        Ok(StoredObject::new(
            self.pointer_for_payment_id(payment_id),
            StorageBackend::S3,
        ))
    }

    async fn get(&self, pointer: &DurablePointer) -> Result<Vec<u8>, StoreError> {
        let key = self.key_from_pointer(pointer)?;
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let msg = format!("{e}");
                // A lifecycle-expired object and a never-existent one both come
                // back as NoSuchKey. We cannot tell them apart from here, so we
                // report the honest, narrower answer.
                if msg.contains("NoSuchKey") || msg.contains("NotFound") {
                    StoreError::NotFound
                } else {
                    StoreError::Unavailable(format!("s3 get_object: {e}"))
                }
            })?;

        let bytes = out
            .body
            .collect()
            .await
            .map_err(|e| StoreError::Unavailable(format!("s3 body: {e}")))?;
        Ok(bytes.into_bytes().to_vec())
    }
}

/// In-memory store. Tests and local development only.
///
/// Deliberately not wired to any env value that could select it in production:
/// a facilitator that silently "anchored" to a HashMap would report durable
/// evidence for data that dies with the process.
#[derive(Debug, Default)]
pub struct MemoryEvidenceStore {
    inner: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl MemoryEvidenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl EvidenceStore for MemoryEvidenceStore {
    fn pointer_for(&self, payment_id: &str, _blob: &[u8]) -> DurablePointer {
        DurablePointer(format!("mem://{payment_id}"))
    }

    fn backend(&self) -> StorageBackend {
        StorageBackend::S3
    }

    async fn put(
        &self,
        payment_id: &str,
        blob: &[u8],
        _retention: Retention,
    ) -> Result<StoredObject, StoreError> {
        let pointer = format!("mem://{payment_id}");
        self.inner
            .lock()
            .expect("poisoned")
            .insert(pointer.clone(), blob.to_vec());
        Ok(StoredObject::new(DurablePointer(pointer), self.backend()))
    }

    async fn delete(&self, reference: &str) -> Result<(), StoreError> {
        self.inner.lock().expect("poisoned").remove(reference);
        Ok(())
    }

    async fn get(&self, pointer: &DurablePointer) -> Result<Vec<u8>, StoreError> {
        self.inner
            .lock()
            .expect("poisoned")
            .get(pointer.as_str())
            .cloned()
            .ok_or(StoreError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx402::envelope::{open, seal, PayerPublicKey, PayerSecretKey, SealedEnvelope};

    #[tokio::test]
    async fn memory_store_round_trips_a_sealed_envelope() {
        // The full path an anchor takes: seal, store the bytes, read them back,
        // parse and decrypt. If any layer mangles the blob this catches it.
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        let payer = PayerPublicKey::Secp256k1(Box::new(sk.public_key()));
        let body = b"evidence that outlives the session";
        let env = seal(body, &payer, b"pid-1").unwrap();

        let store = MemoryEvidenceStore::new();
        let pointer = store
            .put("pid-1", &env.to_bytes(), Retention::Days90)
            .await
            .unwrap();

        let fetched = store.get(&pointer.pointer).await.unwrap();
        let parsed = SealedEnvelope::from_bytes(&fetched).unwrap();
        let secret = PayerSecretKey::Secp256k1(Box::new(sk));
        assert_eq!(open(&parsed, &secret, b"pid-1").unwrap(), body);
    }

    #[tokio::test]
    async fn missing_evidence_is_not_found() {
        let store = MemoryEvidenceStore::new();
        assert!(matches!(
            store.get(&DurablePointer("mem://nope".into())).await,
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn only_unavailable_is_retryable() {
        // NotFound must never be retried into existence, and Expired is a final
        // answer. Getting this wrong turns a transient blip into a permanent
        // "no evidence" record.
        assert!(StoreError::Unavailable("x".into()).is_retryable());
        assert!(!StoreError::NotFound.is_retryable());
        assert!(!StoreError::Expired.is_retryable());
        assert!(!StoreError::NotConfigured(StorageBackend::Ipfs).is_retryable());
    }

    #[test]
    fn s3_pointer_round_trips_through_key_extraction() {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-2"))
            .build();
        let store = S3EvidenceStore::new(
            aws_sdk_s3::Client::from_conf(config),
            "uvd-dx402-evidence".into(),
            "https://evidence.ultravioletadao.xyz/".into(),
        );

        // The pointer addresses the payment, not the bucket layout, so buyers
        // holding a year-old pointer keep working if the S3 keys are reorganised.
        let pointer = store.pointer_for("0xabc", b"");
        assert_eq!(
            pointer.as_str(),
            "s3+https://evidence.ultravioletadao.xyz/0xabc"
        );
        assert_eq!(
            store.key_from_pointer(&pointer).unwrap(),
            S3EvidenceStore::object_key("0xabc")
        );
    }

    #[test]
    fn s3_store_rejects_a_foreign_pointer() {
        // A pointer at somebody else's host must not be dereferenced through our
        // credentials -- that is an SSRF waiting to happen.
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-2"))
            .build();
        let store = S3EvidenceStore::new(
            aws_sdk_s3::Client::from_conf(config),
            "uvd-dx402-evidence".into(),
            "https://evidence.ultravioletadao.xyz".into(),
        );

        for hostile in [
            "s3+https://evil.example/0xabc",
            "https://evidence.ultravioletadao.xyz/0xabc", // no s3+ tag
            "ipfs://bafyfoo",
            "s3+http://169.254.169.254/latest/meta-data/",
            // Path traversal: the payment id is interpolated into an S3 key, so
            // anything with a separator in it must be refused outright rather
            // than sanitised.
            "s3+https://evidence.ultravioletadao.xyz/../../etc/passwd",
            "s3+https://evidence.ultravioletadao.xyz/a/b",
            "s3+https://evidence.ultravioletadao.xyz/", // empty payment id
            "s3+https://evidence.ultravioletadao.xyz.evil.example/0xabc",
        ] {
            assert!(
                matches!(
                    store.key_from_pointer(&DurablePointer(hostile.into())),
                    Err(StoreError::ForeignPointer(_))
                ),
                "{hostile} was accepted"
            );
        }
    }
}
