//! Pinata-backed evidence store (IPFS).
//!
//! Two networks, and they are different products rather than two spellings of
//! the same one:
//!
//! * `private` -- Pinata's own storage. Readable only through a signed URL we
//!   mint, not resolvable from a public gateway, and **deletable**. Retention
//!   keeps meaning what the receipt says it means, so this is the default.
//! * `public`  -- real IPFS. The CID is global and anyone resolves it without
//!   us, which is the point of asking for it. But unpinning removes OUR copy,
//!   not everyone's, so `retention` degrades to "we stop paying" and the
//!   `retentionUntil` we SIGNED stops being true. Irreversible.
//!
//! Measured against the live API on 2026-08-19, because three things differ
//! from the documentation and each one changes the code:
//!
//! 1. A `name` containing `/` is truncated at the slash (`evidence/0xab.dx402`
//!    came back as `evidence`), so the S3-style key layout does not survive.
//!    The reliable index is `keyvalues.paymentId`.
//! 2. Uploads are deduplicated by content, and a duplicate returns the FIRST
//!    record including its network -- an upload requested as `public` came back
//!    `"network":"private"`. Harmless here (every envelope carries a random
//!    nonce, so ciphertext is unique) but it would silently ignore the caller's
//!    choice for anything deterministic.
//! 3. Private reads need a signed URL against the ACCOUNT'S OWN gateway. The
//!    generic `gateway.pinata.cloud` answers 403.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;

use super::store::{EvidenceStore, StoreError, StoredObject};
use super::types::{DurablePointer, Retention, StorageBackend};

const UPLOAD_URL: &str = "https://uploads.pinata.cloud/v3/files";
const API_BASE: &str = "https://api.pinata.cloud";

/// How long a signed read URL stays valid. Long enough for a buyer on a slow
/// link, short enough that a leaked URL is not a standing grant.
const SIGNED_URL_TTL_SECS: u64 = 300;

/// Bound on every Pinata call.
///
/// DX402 is an addition to the payment path, never a gate in front of it. An
/// upload that hangs must cost the receipt, not the sale -- and with the
/// fallback store in front, a timeout here is what makes S3 take over.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Which IPFS network an object lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PinataNetwork {
    /// Pinata's own storage. Deletable, so retention stays honest.
    #[default]
    Private,
    /// Public IPFS. Globally resolvable and effectively permanent.
    Public,
}

impl PinataNetwork {
    fn as_str(self) -> &'static str {
        match self {
            PinataNetwork::Private => "private",
            PinataNetwork::Public => "public",
        }
    }

    /// Whether an object on this network can actually be removed.
    ///
    /// This is the property the receipt's `retentionUntil` depends on, so it is
    /// spelled out rather than implied by the network name.
    pub fn is_revocable(self) -> bool {
        matches!(self, PinataNetwork::Private)
    }
}

/// CIDv1, raw codec, sha2-256 -- computed locally, without contacting Pinata.
///
/// This is what lets the caller reserve the registry slot BEFORE uploading a
/// byte, which is load-bearing: uploading first meant a request that correctly
/// lost the anti-replay had already overwritten the evidence it lost to.
///
/// Layout: `0x01` (v1) `0x55` (raw) `0x12` (sha2-256) `0x20` (32 bytes) digest,
/// then multibase base32-lower with a `b` prefix -- the `bafkrei...` form.
/// Valid for content that fits one block, which every sealed envelope does.
pub fn cid_v1_raw(blob: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(blob);
    let mut bytes = Vec::with_capacity(4 + digest.len());
    bytes.extend_from_slice(&[0x01, 0x55, 0x12, 0x20]);
    bytes.extend_from_slice(&digest);
    format!(
        "b{}",
        data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
    )
}

#[derive(Debug, Deserialize)]
struct UploadEnvelope {
    data: UploadData,
}

#[derive(Debug, Deserialize)]
struct UploadData {
    id: String,
    cid: String,
    /// Echoed back by Pinata. Worth reading rather than assuming: a
    /// content-duplicate returns the FIRST record's network, so what we asked
    /// for and what we got are not always the same.
    #[serde(default)]
    network: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SignedUrlEnvelope {
    data: String,
}

/// Pinata-backed store.
#[derive(Debug, Clone)]
pub struct PinataEvidenceStore {
    client: reqwest::Client,
    jwt: String,
    /// The account's own gateway domain. NOT `gateway.pinata.cloud`: signing a
    /// URL against the generic host answers 403.
    gateway: String,
    /// Where a pointer dereferences for a private object -- our own host, since
    /// only we can mint the signed URL.
    public_base: String,
    network: PinataNetwork,
}

impl PinataEvidenceStore {
    pub fn new(
        jwt: impl Into<String>,
        gateway: impl Into<String>,
        public_base: impl Into<String>,
        network: PinataNetwork,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .connect_timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap_or_default(),
            jwt: jwt.into(),
            gateway: gateway.into().trim_end_matches('/').to_string(),
            public_base: public_base.into().trim_end_matches('/').to_string(),
            network,
        }
    }

    pub fn network(&self) -> PinataNetwork {
        self.network
    }

    /// The pointer for a private object: our own host, because dereferencing it
    /// requires minting a signed URL and only we hold the key.
    fn private_pointer(&self, payment_id: &str) -> DurablePointer {
        DurablePointer(format!(
            "ipfs+https://{}/dx402/blob/{payment_id}",
            self.host()
        ))
    }

    fn host(&self) -> &str {
        self.public_base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
    }

    async fn signed_url(&self, cid: &str) -> Result<String, StoreError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let body = serde_json::json!({
            "url": format!("https://{}/files/{cid}", self.gateway),
            "date": now,
            "expires": SIGNED_URL_TTL_SECS,
            "method": "GET",
        });
        let res = self
            .client
            .post(format!("{API_BASE}/v3/files/private/download_link"))
            .bearer_auth(&self.jwt)
            .json(&body)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata sign: {e}")))?;

        if !res.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "pinata sign returned {}",
                res.status()
            )));
        }
        res.json::<SignedUrlEnvelope>()
            .await
            .map(|e| e.data)
            .map_err(|e| StoreError::Unavailable(format!("pinata sign body: {e}")))
    }
}

#[async_trait]
impl EvidenceStore for PinataEvidenceStore {
    fn backend(&self) -> StorageBackend {
        StorageBackend::Ipfs
    }

    async fn put(
        &self,
        payment_id: &str,
        blob: &[u8],
        retention: Retention,
    ) -> Result<StoredObject, StoreError> {
        // `keyvalues`, not `name`: a name containing a slash is truncated at it,
        // so the paymentId has to travel somewhere that survives.
        // `retentionUntil` as an absolute deadline, not just the retention
        // name: the sweeper reads it back off the object, and a name would
        // force it to re-derive the deadline from an anchoring time it does not
        // have. Written at upload because the object id does not exist yet when
        // the registry record is written.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let keyvalues = serde_json::json!({
            "paymentId": payment_id,
            "retention": retention.to_string(),
            "retentionUntil": retention.until(now).to_string(),
        })
        .to_string();

        let part = reqwest::multipart::Part::bytes(blob.to_vec())
            .file_name(format!("{payment_id}.dx402"))
            .mime_str("application/octet-stream")
            .map_err(|e| StoreError::Unavailable(format!("pinata part: {e}")))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("network", self.network.as_str())
            .text("name", format!("{payment_id}.dx402"))
            .text("keyvalues", keyvalues);

        let res = self
            .client
            .post(UPLOAD_URL)
            .bearer_auth(&self.jwt)
            .multipart(form)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata upload: {e}")))?;

        let status = res.status();
        if !status.is_success() {
            return Err(StoreError::Unavailable(format!(
                "pinata upload returned {status}"
            )));
        }

        let data = res
            .json::<UploadEnvelope>()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata upload body: {e}")))?
            .data;

        // Trust what came back over what we asked for. A content-duplicate
        // returns the first record, network included, so a `public` request can
        // land on a `private` object. Recording the wrong one would make a
        // pointer that never resolves.
        let landed = match data.network.as_deref() {
            Some("public") => PinataNetwork::Public,
            _ => PinataNetwork::Private,
        };
        if landed != self.network {
            warn!(
                payment_id,
                asked = self.network.as_str(),
                got = landed.as_str(),
                pinata_id = %data.id,
                "DX402 pinata upload landed on a different network than requested \
                 (content duplicate) -- recording where it actually is"
            );
        }

        let pointer = match landed {
            // Public: the CID is the address, and anyone resolves it without us.
            PinataNetwork::Public => DurablePointer(format!("ipfs://{}", data.cid)),
            // Private: only we can mint a read URL, so it dereferences here.
            PinataNetwork::Private => self.private_pointer(payment_id),
        };
        Ok(StoredObject {
            pointer,
            // Pinata's own object id, persisted so retention can actually be
            // enforced later. A private pointer names the payment, not the
            // object, so without this there is nothing to hand `delete`.
            reference: Some(format!("{}/{}", landed.as_str(), data.id)),
        })
    }

    async fn get(&self, pointer: &DurablePointer) -> Result<Vec<u8>, StoreError> {
        let raw = pointer.as_str();

        let url = if let Some(cid) = raw.strip_prefix("ipfs://") {
            // Public: our own gateway still serves it, and using it keeps the
            // read off a third party we do not control.
            format!("https://{}/ipfs/{cid}", self.gateway)
        } else if raw.starts_with("ipfs+https://") {
            // Private: the pointer names the payment, so the CID has to be
            // looked up. The caller resolves it from the registry and hands us
            // an `ipfs://` pointer; a bare payment pointer cannot be fetched
            // without that lookup.
            return Err(StoreError::ForeignPointer(format!(
                "{raw} needs its CID resolved from the registry first"
            )));
        } else {
            return Err(StoreError::ForeignPointer(raw.to_string()));
        };

        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata fetch: {e}")))?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StoreError::NotFound);
        }
        if !res.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "pinata fetch returned {}",
                res.status()
            )));
        }
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| StoreError::Unavailable(format!("pinata fetch body: {e}")))
    }

    fn pointer_for(&self, payment_id: &str, blob: &[u8]) -> DurablePointer {
        match self.network {
            // Computed locally, so the registry slot can still be reserved
            // before a single byte is uploaded.
            PinataNetwork::Public => DurablePointer(format!("ipfs://{}", cid_v1_raw(blob))),
            PinataNetwork::Private => self.private_pointer(payment_id),
        }
    }
}

impl PinataEvidenceStore {
    /// Fetch a private object by CID, minting a signed URL first.
    ///
    /// Separate from `get` because a private pointer names the payment, not the
    /// content: the caller resolves the CID from the registry and calls this.
    pub async fn get_private_by_cid(&self, cid: &str) -> Result<Vec<u8>, StoreError> {
        let url = self.signed_url(cid).await?;
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata private fetch: {e}")))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StoreError::NotFound);
        }
        if !res.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "pinata private fetch returned {}",
                res.status()
            )));
        }
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| StoreError::Unavailable(format!("pinata private body: {e}")))
    }

    /// Delete an object. This is what makes retention real on the private
    /// network -- there is no bucket lifecycle rule doing it for us.
    ///
    /// `reference` is `"<network>/<id>"` as `put` recorded it. The network is
    /// carried rather than re-read from `self` because an object written while
    /// the store was configured differently must still be deletable.
    pub async fn delete_reference(&self, reference: &str) -> Result<(), StoreError> {
        let (network, pinata_id) = reference
            .split_once('/')
            .unwrap_or((self.network.as_str(), reference));
        let res = self
            .client
            .delete(format!("{API_BASE}/v3/files/{network}/{pinata_id}"))
            .bearer_auth(&self.jwt)
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(format!("pinata delete: {e}")))?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Unavailable(format!(
                "pinata delete returned {}",
                res.status()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cid_matches_what_pinata_returns() {
        // Pinned against a REAL upload, not against this function's own output.
        // A vector produced and checked by the same code proves only that the
        // code agrees with itself -- three fabricated SHA-256 variants of
        // ERC-8004 SEAL v1 passed CI for months on exactly that mistake.
        //
        // Provenance: uploaded the five bytes `DX402` to the live Pinata v3 API
        // on 2026-08-19 and recorded the CID it computed. The first vector
        // written here was invented and did NOT match -- which is the whole
        // reason this test compares against something we did not produce.
        assert_eq!(
            cid_v1_raw(b"DX402"),
            "bafkreidm5aqvxku7tlskrqc5mixxsuudvzx3erlq7r56rvkeway6t7q6cy"
        );
    }

    #[test]
    fn the_cid_is_stable_and_content_addressed() {
        assert_eq!(cid_v1_raw(b"same"), cid_v1_raw(b"same"));
        assert_ne!(cid_v1_raw(b"same"), cid_v1_raw(b"other"));
        assert!(cid_v1_raw(b"anything").starts_with("bafkrei"));
    }

    #[test]
    fn only_the_private_network_can_honour_a_retention_promise() {
        // The receipt SIGNS `retentionUntil`. On public IPFS unpinning removes
        // our copy and not the network's, so that signature would attest to a
        // deletion that never happens.
        assert!(PinataNetwork::Private.is_revocable());
        assert!(!PinataNetwork::Public.is_revocable());
    }

    #[test]
    fn a_public_pointer_is_the_cid_and_a_private_one_is_the_payment() {
        let public = PinataEvidenceStore::new(
            "jwt",
            "gw.mypinata.cloud",
            "https://f.test",
            PinataNetwork::Public,
        );
        let private = PinataEvidenceStore::new(
            "jwt",
            "gw.mypinata.cloud",
            "https://f.test",
            PinataNetwork::Private,
        );

        let blob = b"sealed bytes";
        assert_eq!(
            public.pointer_for("0xabc", blob).as_str(),
            format!("ipfs://{}", cid_v1_raw(blob)),
            "public addresses the CONTENT -- resolvable without us"
        );
        assert_eq!(
            private.pointer_for("0xabc", blob).as_str(),
            "ipfs+https://f.test/dx402/blob/0xabc",
            "private addresses the PAYMENT -- only we can mint a read URL"
        );
    }

    #[test]
    fn a_public_pointer_survives_a_reupload_of_the_same_bytes() {
        // Pinata deduplicates by content. Because the pointer is derived from
        // the bytes rather than from Pinata's record id, that dedup cannot make
        // an old pointer dangle.
        let store = PinataEvidenceStore::new("j", "g", "https://f.test", PinataNetwork::Public);
        assert_eq!(
            store.pointer_for("0xone", b"identical").as_str(),
            store.pointer_for("0xtwo", b"identical").as_str()
        );
    }
}

/// Primary store with an automatic fallback.
///
/// When the primary is unreachable the anchor still lands, in the fallback. That
/// is not a downgrade of the promise: S3 is the more conservative backend --
/// private, deletable, retention enforced by a bucket rule -- so falling back
/// never converts a revocable promise into an irrevocable one. The reverse
/// (falling back INTO public IPFS) would, and is not offered.
///
/// The record must say where the bytes ACTUALLY are. A record claiming `ipfs`
/// for something sitting in S3 sends every later read to the wrong place, and
/// the evidence looks lost while being perfectly intact.
#[derive(Debug, Clone)]
pub struct FallbackEvidenceStore {
    primary: std::sync::Arc<dyn EvidenceStore>,
    fallback: std::sync::Arc<dyn EvidenceStore>,
}

impl FallbackEvidenceStore {
    pub fn new(
        primary: std::sync::Arc<dyn EvidenceStore>,
        fallback: std::sync::Arc<dyn EvidenceStore>,
    ) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl EvidenceStore for FallbackEvidenceStore {
    fn backend(&self) -> StorageBackend {
        self.primary.backend()
    }

    async fn put(
        &self,
        payment_id: &str,
        blob: &[u8],
        retention: Retention,
    ) -> Result<StoredObject, StoreError> {
        match self.primary.put(payment_id, blob, retention).await {
            Ok(stored) => Ok(stored),
            Err(e) => {
                // A category of its own, not a generic error: this is the line
                // that tells an operator the Pinata credential expired months
                // before anyone notices the evidence quietly moved.
                warn!(
                    payment_id,
                    error = %e,
                    primary = %self.primary.backend(),
                    fallback = %self.fallback.backend(),
                    "dx402_primary_store_unavailable -- anchoring to the fallback instead"
                );
                self.fallback.put(payment_id, blob, retention).await
            }
        }
    }

    async fn get(&self, pointer: &DurablePointer) -> Result<Vec<u8>, StoreError> {
        // Ask, do not guess. Routing on the pointer's scheme would hardcode an
        // assumption about which store is primary -- and since `put` may have
        // landed in either one, a wrong guess reports perfectly intact evidence
        // as missing.
        //
        // Only `ForeignPointer` falls through: it means "this pointer is not
        // mine", which is the one answer that says nothing about whether the
        // object exists. Retrying on `NotFound` would turn a genuine deletion
        // into a second lookup that can only agree.
        match self.primary.get(pointer).await {
            // "Not mine" -- says nothing about whether the object exists.
            Err(StoreError::ForeignPointer(_)) => self.fallback.get(pointer).await,
            // "I am down" -- and the object may well have been written to the
            // fallback during the same outage, which is exactly what `put`
            // does. Costs a wasted round trip to a dead host on every read
            // while the outage lasts; reads are off the payment path, so that
            // is the cheaper of the two mistakes.
            Err(StoreError::Unavailable(_)) => self.fallback.get(pointer).await,
            // `NotFound` and `Expired` are verdicts. Asking a second store can
            // only agree, and retrying would blur "deleted" into "look again".
            other => other,
        }
    }

    fn pointer_for(&self, payment_id: &str, blob: &[u8]) -> DurablePointer {
        // Reserving names the PRIMARY. If the write then falls back, `put`
        // returns the fallback's pointer and the caller records that one.
        self.primary.pointer_for(payment_id, blob)
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use crate::dx402::store::MemoryEvidenceStore;
    use std::sync::Arc;

    /// A store that refuses everything, standing in for an outage.
    #[derive(Debug)]
    struct DeadStore;

    #[async_trait]
    impl EvidenceStore for DeadStore {
        fn backend(&self) -> StorageBackend {
            StorageBackend::Ipfs
        }
        async fn put(&self, _: &str, _: &[u8], _: Retention) -> Result<StoredObject, StoreError> {
            Err(StoreError::Unavailable("simulated outage".into()))
        }
        async fn get(&self, _: &DurablePointer) -> Result<Vec<u8>, StoreError> {
            Err(StoreError::Unavailable("simulated outage".into()))
        }
        fn pointer_for(&self, payment_id: &str, _: &[u8]) -> DurablePointer {
            DurablePointer(format!("ipfs+https://dead.test/dx402/blob/{payment_id}"))
        }
    }

    #[tokio::test]
    async fn an_outage_in_the_primary_still_anchors() {
        // The whole point: evidence is not lost because a third party is down.
        let store =
            FallbackEvidenceStore::new(Arc::new(DeadStore), Arc::new(MemoryEvidenceStore::new()));
        let stored = store
            .put("0xabc", b"sealed", Retention::Days90)
            .await
            .expect("the fallback takes over");

        assert!(
            !stored.pointer.as_str().starts_with("ipfs"),
            "the pointer must name where the bytes ACTUALLY landed, not where we tried: {}",
            stored.pointer
        );
        assert_eq!(store.get(&stored.pointer).await.unwrap(), b"sealed");
    }

    #[tokio::test]
    async fn a_healthy_primary_is_used_and_the_fallback_is_not() {
        let store =
            FallbackEvidenceStore::new(Arc::new(MemoryEvidenceStore::new()), Arc::new(DeadStore));
        let pointer = store
            .put("0xabc", b"sealed", Retention::Days90)
            .await
            .unwrap();
        assert_eq!(store.get(&pointer.pointer).await.unwrap(), b"sealed");
    }

    #[tokio::test]
    async fn both_down_is_an_error_the_caller_degrades_on() {
        // Not a panic and not a silent success: `anchor` turns this into a skip,
        // and the payment is untouched either way.
        let store = FallbackEvidenceStore::new(Arc::new(DeadStore), Arc::new(DeadStore));
        assert!(store
            .put("0xabc", b"sealed", Retention::Days90)
            .await
            .is_err());
    }
}

// ============================================================================
// Retention sweeper
// ============================================================================
//
// S3 expires objects with a bucket lifecycle rule. Pinata expires nothing on
// its own, so on that backend `retentionUntil` -- which the facilitator SIGNS
// into every receipt -- is a promise with no mechanism behind it unless
// something deletes the object. This is that something.
//
// Objects are found by `keyvalues.retentionUntil`, written at upload time,
// rather than by a reference stored in the registry. That ordering matters: the
// upload happens AFTER the registry write (so a caller that loses the
// anti-replay cannot overwrite evidence it does not own), so Pinata's object id
// does not exist yet when the record is written. Reading the deadline off the
// object itself needs no second write and works on objects already uploaded.

#[derive(Debug, Deserialize)]
struct FileListEnvelope {
    data: FileListData,
}

#[derive(Debug, Deserialize)]
struct FileListData {
    #[serde(default)]
    files: Vec<FileEntry>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    id: String,
    #[serde(default)]
    keyvalues: std::collections::HashMap<String, String>,
}

/// What a sweep did, so an operator can see it working rather than assume it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub examined: usize,
    pub expired: usize,
    pub deleted: usize,
    pub failed: usize,
    /// Objects with no `retentionUntil` we could read. Reported rather than
    /// deleted: an object we cannot date is one we must not destroy.
    pub undatable: usize,
}

impl PinataEvidenceStore {
    /// Delete every object whose retention has expired.
    ///
    /// Conservative on purpose. An object whose `retentionUntil` cannot be read
    /// is counted and left alone -- deleting evidence because we could not
    /// parse its deadline would be the worst possible failure of a component
    /// whose whole job is honouring a deadline.
    ///
    /// Never deletes on the public network: unpinning there does not remove the
    /// bytes from IPFS, so it would report a deletion that did not happen.
    pub async fn sweep_expired(
        &self,
        now: u64,
        max_pages: usize,
    ) -> Result<SweepReport, StoreError> {
        let mut report = SweepReport::default();
        if self.network == PinataNetwork::Public {
            warn!(
                "DX402 retention sweep skipped: public IPFS objects cannot be deleted, \
                 only unpinned -- reporting a deletion there would be a lie"
            );
            return Ok(report);
        }

        let mut token: Option<String> = None;
        for _ in 0..max_pages {
            let mut url = format!("{API_BASE}/v3/files/private?limit=100");
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={t}"));
            }
            let res = self
                .client
                .get(&url)
                .bearer_auth(&self.jwt)
                .send()
                .await
                .map_err(|e| StoreError::Unavailable(format!("pinata list: {e}")))?;
            if !res.status().is_success() {
                return Err(StoreError::Unavailable(format!(
                    "pinata list returned {}",
                    res.status()
                )));
            }
            let page = res
                .json::<FileListEnvelope>()
                .await
                .map_err(|e| StoreError::Unavailable(format!("pinata list body: {e}")))?;

            for f in &page.data.files {
                report.examined += 1;
                let Some(until) = f
                    .keyvalues
                    .get("retentionUntil")
                    .and_then(|v| v.parse::<u64>().ok())
                else {
                    report.undatable += 1;
                    continue;
                };
                if until > now {
                    continue;
                }
                report.expired += 1;
                match self.delete_reference(&format!("private/{}", f.id)).await {
                    Ok(()) => report.deleted += 1,
                    Err(e) => {
                        report.failed += 1;
                        warn!(id = %f.id, error = %e, "DX402 retention sweep could not delete");
                    }
                }
            }

            token = page.data.next_page_token.clone();
            if token.is_none() {
                break;
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    #[tokio::test]
    async fn the_public_network_is_never_swept() {
        // Unpinning does not delete from IPFS. A sweep that reported deletions
        // there would be attesting to something that did not happen -- and the
        // receipt already carries our signature over `retentionUntil`.
        let store = PinataEvidenceStore::new("j", "g", "https://f.test", PinataNetwork::Public);
        let report = store.sweep_expired(9_999_999_999, 1).await.unwrap();
        assert_eq!(
            report,
            SweepReport::default(),
            "nothing examined, nothing deleted"
        );
    }
}

/// Run the retention sweep on a timer.
///
/// Only meaningful for Pinata: S3 expires objects with a bucket lifecycle rule,
/// so nothing has to run. Without this, choosing the ipfs backend would mean
/// evidence that never expires while every receipt we signed says it does --
/// which is why the backend stayed off until this existed.
///
/// Hourly by default. Retention is measured in days, so an hour of lag past the
/// deadline is immaterial, and a slow cadence keeps the list calls negligible.
pub fn spawn_retention_sweeper(
    store: std::sync::Arc<PinataEvidenceStore>,
    tick_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn(async move {
        let interval = std::time::Duration::from_secs(tick_secs.max(300));
        tracing::info!(
            tick_secs = interval.as_secs(),
            "DX402 retention sweeper started (pinata has no lifecycle rule of its own)"
        );
        loop {
            tokio::time::sleep(interval).await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match store.sweep_expired(now, 20).await {
                Ok(r) if r.expired > 0 || r.undatable > 0 => tracing::info!(
                    examined = r.examined,
                    expired = r.expired,
                    deleted = r.deleted,
                    failed = r.failed,
                    undatable = r.undatable,
                    "DX402 retention sweep"
                ),
                Ok(_) => {}
                // Never fatal: a sweep that could not run leaves objects in
                // place, which is the safe direction. The next tick retries.
                Err(e) => tracing::warn!(error = %e, "DX402 retention sweep failed"),
            }
        }
    })
}
