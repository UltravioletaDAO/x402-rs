//! DX402 `durable-evidence`: the seller-side post-hook.
//!
//! This is the piece that makes a paid response survive the session, and it has
//! to live here rather than in the facilitator for a structural reason: **the
//! facilitator never sees the response body.** It participates in `/verify` and
//! `/settle` only. The body exists in exactly one place -- inside this
//! middleware, after the inner handler has run.
//!
//! ```text
//! Client ──GET+X-PAYMENT──► [ this middleware ]
//!                             │  inner handler → BODY   ◄── only place it exists
//!                             │  settle → payer identity
//!                             │  seal(BODY → payer's public key)
//!                             │  upload ciphertext → sink
//!                             │  POST /dx402/anchor  (metadata only)
//! Client ◄─200 + BODY + X-Payment-Response + X-Durable-Evidence
//! ```
//!
//! # It cannot break a payment
//!
//! Every failure here -- oversized body, unreachable sink, unrecoverable payer
//! key -- resolves to a [`SkipReason`] carried in the header. The buyer still
//! gets their bytes and the settlement still stands. That is not defensive
//! coding, it is the design constraint: evidence is an addition to the payment
//! path, never a gate in front of it.

use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use x402_rs::dx402::envelope::{seal, PayerPublicKey};
use x402_rs::dx402::types::{
    AnchorRequest, DurableEvidence, DurablePointer, EvidenceMode, Retention, SkipReason,
    StorageBackend, EVIDENCE_HEADER,
};
use x402_rs::network::Network;
use x402_rs::types::MixedAddress;

/// Per-route DX402 configuration.
#[derive(Debug, Clone)]
pub struct DurableConfig {
    pub mode: EvidenceMode,
    pub backend: StorageBackend,
    pub retention: Retention,
    /// Bodies above this are skipped. A large body is a reason to skip evidence,
    /// never a reason to fail a payment.
    pub max_body_bytes: usize,
}

impl Default for DurableConfig {
    fn default() -> Self {
        Self {
            mode: EvidenceMode::Direct,
            backend: StorageBackend::S3,
            retention: Retention::Days90,
            max_body_bytes: 1_048_576,
        }
    }
}

/// Where sealed ciphertext is written.
///
/// Abstracted so a seller can keep evidence in their own bucket, on IPFS, or
/// anywhere reachable by URL, without this crate taking a hard dependency on any
/// storage SDK.
#[async_trait::async_trait]
pub trait EvidenceSink: Send + Sync + std::fmt::Debug {
    async fn put(&self, payment_id: &str, blob: &[u8]) -> Result<DurablePointer, String>;
}

/// How long evidence work may hold a paid response hostage.
///
/// Unbounded, a facilitator or sink that accepts a connection and then stalls
/// blocks the buyer's already-settled response for as long as it stalls. DX402
/// is an addition to the payment path and must never be a gate in front of it:
/// a slow anchor has to cost the receipt, never the delivery.
const EVIDENCE_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn evidence_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(EVIDENCE_HTTP_TIMEOUT)
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        // A builder failure means no TLS backend; the default client is still
        // better than panicking inside a payment path.
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Writes evidence with an HTTP `PUT`, which covers presigned S3 URLs, most IPFS
/// pinning gateways, and any plain object store.
#[derive(Debug, Clone)]
pub struct HttpPutSink {
    client: reqwest::Client,
    /// Base URL. The object lands at `{base}/{payment_id}.dx402`.
    base: String,
    bearer: Option<String>,
}

impl HttpPutSink {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            client: evidence_http_client(),
            base: base.into().trim_end_matches('/').to_string(),
            bearer: None,
        }
    }

    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }
}

#[async_trait::async_trait]
impl EvidenceSink for HttpPutSink {
    async fn put(&self, payment_id: &str, blob: &[u8]) -> Result<DurablePointer, String> {
        let url = format!("{}/{}.dx402", self.base, payment_id);
        let mut req = self
            .client
            .put(&url)
            .header("content-type", "application/octet-stream")
            .body(blob.to_vec());
        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("sink returned {}", res.status()));
        }
        Ok(DurablePointer(url))
    }
}

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct MemorySink {
    inner: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, pointer: &DurablePointer) -> Option<Vec<u8>> {
        self.inner
            .lock()
            .expect("poisoned")
            .get(pointer.as_str())
            .cloned()
    }
}

#[async_trait::async_trait]
impl EvidenceSink for MemorySink {
    async fn put(&self, payment_id: &str, blob: &[u8]) -> Result<DurablePointer, String> {
        let pointer = format!("mem://{payment_id}");
        self.inner
            .lock()
            .expect("poisoned")
            .insert(pointer.clone(), blob.to_vec());
        Ok(DurablePointer(pointer))
    }
}

/// Everything the post-hook needs about the settled payment.
#[derive(Debug, Clone)]
pub struct SettledContext {
    pub payment_id: String,
    pub network: Network,
    pub tx_hash: String,
    pub payer: MixedAddress,
    pub payee: MixedAddress,
    /// The proof the facilitator returned from `/settle`.
    ///
    /// Passed straight back on the anchor so the facilitator can verify the
    /// payment it is about to sign a receipt for. Absent on chains that do not
    /// produce one; the anchor gate reports that and never enforces it.
    pub proof: Option<x402_rs::erc8004::ProofOfPayment>,
}

/// The DX402 post-hook.
#[derive(Debug, Clone)]
pub struct DurableEvidenceHook {
    config: DurableConfig,
    sink: Arc<dyn EvidenceSink>,
    facilitator_base: String,
    client: reqwest::Client,
    /// Key that signs the anchor authorization, proving this anchor comes from
    /// whoever got paid.
    ///
    /// Separate from any payment wallet on purpose: it authorises evidence, not
    /// transfers, so a leak forges anchors but moves no money.
    anchor_signer: Option<Arc<alloy::signers::local::PrivateKeySigner>>,
}

impl DurableEvidenceHook {
    pub fn new(
        config: DurableConfig,
        sink: Arc<dyn EvidenceSink>,
        facilitator_base: impl Into<String>,
    ) -> Self {
        Self {
            config,
            sink,
            facilitator_base: facilitator_base.into().trim_end_matches('/').to_string(),
            client: evidence_http_client(),
            anchor_signer: None,
        }
    }

    /// Attach the key that signs anchor authorizations.
    ///
    /// Without it the facilitator cannot tell this anchor came from the payee,
    /// and once the gate reaches phase 2 the anchor is refused. Evidence is
    /// still produced and stored; only the notarised receipt is lost.
    pub fn with_anchor_signer(mut self, signer: alloy::signers::local::PrivateKeySigner) -> Self {
        self.anchor_signer = Some(Arc::new(signer));
        self
    }

    pub fn config(&self) -> &DurableConfig {
        &self.config
    }

    /// Seal a body, write it to the sink, and register it with the facilitator.
    ///
    /// Always returns a [`DurableEvidence`] -- anchored or skipped -- so the
    /// caller has something to put in the header either way and never has to
    /// decide whether an error is fatal.
    pub async fn capture(
        &self,
        body: &[u8],
        payer_key: Result<PayerPublicKey, SkipReason>,
        ctx: &SettledContext,
    ) -> DurableEvidence {
        if body.len() > self.config.max_body_bytes {
            return DurableEvidence::skipped(SkipReason::TooLarge);
        }

        let payer_key = match payer_key {
            Ok(k) => k,
            Err(reason) => return DurableEvidence::skipped(reason),
        };

        // Hash the PLAINTEXT. This is what lets a buyer prove the anchored blob
        // decrypts to exactly the bytes they were served, which is the check
        // that catches a seller anchoring something other than what it sent.
        let content_hash = x402_rs::dx402::content_hash(body);

        let sealed = match seal(body, &payer_key, ctx.payment_id.as_bytes()) {
            Ok(s) => s,
            Err(_e) => {
                #[cfg(feature = "telemetry")]
                tracing::warn!(error = %_e, "DX402 seal failed; delivering without evidence");
                return DurableEvidence::skipped(SkipReason::AnchorFailed);
            }
        };
        // The payer is always the first recipient this hook writes.
        let key_alg = sealed
            .recipients
            .first()
            .map(|r| r.key_alg)
            .unwrap_or(x402_rs::dx402::types::KeyAlg::Secp256k1);

        let pointer = match self.sink.put(&ctx.payment_id, &sealed.to_bytes()).await {
            Ok(p) => p,
            Err(_e) => {
                #[cfg(feature = "telemetry")]
                tracing::warn!(error = %_e, "DX402 sink write failed; delivering without evidence");
                return DurableEvidence::skipped(SkipReason::AnchorFailed);
            }
        };

        // Sign the anchor so the facilitator can tell it came from the payee.
        // Best-effort: a missing or unusable key costs the receipt, never the
        // response.
        let seller_signature = self.anchor_signer.as_ref().and_then(|signer| {
            let payment_id = ctx.payment_id.parse().ok()?;
            let hash = content_hash.parse().ok()?;
            x402_rs::dx402::gate::sign_authorization(
                signer,
                payment_id,
                hash,
                pointer.as_str(),
                x402_rs::dx402::service::chain_id_of(ctx.network),
            )
            .ok()
        });

        let anchor = AnchorRequest {
            payment_id: ctx.payment_id.clone(),
            network: ctx.network,
            tx_hash: ctx.tx_hash.clone(),
            payer: ctx.payer.clone(),
            payee: ctx.payee.clone(),
            // This hook uploads through its own sink, so it always supplies a
            // pointer. A seller with no storage of its own can instead send the
            // sealed bytes as `sealed` and let the facilitator host them.
            pointer: Some(pointer),
            sealed: None,
            proof_of_payment: ctx.proof.clone(),
            seller_signature,
            backend: self.config.backend,
            content_hash,
            key_alg,
            mode: self.config.mode,
            retention: self.config.retention,
            wrapped_cek: None,
        };

        match self
            .client
            .post(format!("{}/dx402/anchor", self.facilitator_base))
            .json(&anchor)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => match res.json::<serde_json::Value>().await {
                Ok(v) => match serde_json::from_value(v) {
                    Ok(anchored) => DurableEvidence::Anchored(Box::new(anchored)),
                    Err(_) => DurableEvidence::skipped(SkipReason::AnchorFailed),
                },
                Err(_) => DurableEvidence::skipped(SkipReason::AnchorFailed),
            },
            _ => {
                // The ciphertext is already durable at this point; only the
                // notarised receipt is missing. Reported as a skip rather than
                // pretending we have a receipt we do not.
                #[cfg(feature = "telemetry")]
                tracing::warn!("DX402 anchor call failed; evidence stored but not notarised");
                DurableEvidence::skipped(SkipReason::AnchorFailed)
            }
        }
    }
}

/// Encode a [`DurableEvidence`] for the `X-Durable-Evidence` header.
///
/// base64url without padding, so it is a valid single-line header value
/// regardless of what the JSON contains.
pub fn encode_header(evidence: &DurableEvidence) -> Option<String> {
    let json = serde_json::to_vec(evidence).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// Decode an `X-Durable-Evidence` header value.
pub fn decode_header(value: &str) -> Option<DurableEvidence> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .ok()?;
    serde_json::from_slice(&raw).ok()
}

/// The header name, re-exported so callers do not hardcode it.
pub const HEADER_NAME: &str = EVIDENCE_HEADER;

/// Buffer a response body into memory.
///
/// Bounded by `limit`. Whatever happens, the caller gets back something it can
/// still deliver: skipping evidence must never change the bytes the buyer
/// receives. `settle_after_execution` settles BEFORE this runs and the
/// authorization nonce is spent, so a body dropped here is paid-for goods that
/// can never be re-fetched.
pub enum BufferedBody {
    /// Small enough to hash and seal.
    Ready(Bytes),
    /// No evidence for this one -- but here is the body to deliver anyway.
    Skip {
        body: axum_core::body::Body,
        reason: SkipReason,
    },
}

pub async fn buffer_body(body: axum_core::body::Body, limit: usize) -> BufferedBody {
    use http_body::Body as _;
    use http_body_util::BodyExt;

    // Ask before swallowing. A body that ANNOUNCES it is over the limit is
    // passed through untouched -- never collected. Without this, `collect()`
    // buffers the whole thing into memory and only then measures it, so a
    // multi-gigabyte download is an OOM of the 2 GB task rather than a skip.
    let hint = body.size_hint();
    let announced_too_big =
        hint.lower() as usize > limit || hint.upper().is_some_and(|upper| upper as usize > limit);
    if announced_too_big {
        return BufferedBody::Skip {
            body,
            reason: SkipReason::TooLarge,
        };
    }

    match body.collect().await {
        Ok(collected) => {
            let bytes = collected.to_bytes();
            if bytes.len() > limit {
                // A body that under-reported its size. The bytes are already in
                // hand, so hand them back rather than lose them.
                BufferedBody::Skip {
                    body: axum_core::body::Body::from(bytes),
                    reason: SkipReason::TooLarge,
                }
            } else {
                BufferedBody::Ready(bytes)
            }
        }
        // The stream itself failed: there is nothing to deliver either way.
        Err(_) => BufferedBody::Skip {
            body: axum_core::body::Body::empty(),
            reason: SkipReason::AnchorFailed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x402_rs::dx402::envelope::{open, PayerSecretKey, SealedEnvelope};

    fn addr(s: &str) -> MixedAddress {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    fn ctx() -> SettledContext {
        SettledContext {
            payment_id: format!("0x{}", "11".repeat(32)),
            network: Network::Base,
            tx_hash: format!("0x{}", "33".repeat(32)),
            payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
            payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
            proof: None,
        }
    }

    #[tokio::test]
    async fn the_buyer_can_decrypt_what_the_hook_stored() {
        // The whole product in one test: a response body goes in, ciphertext
        // lands in durable storage, and only the payer's key gets it back.
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        let payer_key = PayerPublicKey::Secp256k1(Box::new(sk.public_key()));

        let sink = Arc::new(MemorySink::new());
        let hook = DurableEvidenceHook::new(
            DurableConfig::default(),
            sink.clone(),
            "http://127.0.0.1:1", // unreachable on purpose, see below
        );

        let body = b"the paid response that must outlive the session";
        let ctx = ctx();
        let evidence = hook.capture(body, Ok(payer_key), &ctx).await;

        // The facilitator is unreachable, so there is no receipt -- but the
        // ciphertext is already durable. That distinction is the point of
        // reporting a skip instead of claiming an anchor.
        assert!(matches!(evidence, DurableEvidence::Skipped(_)));

        let stored = sink
            .get(&DurablePointer(format!("mem://{}", ctx.payment_id)))
            .expect("ciphertext should be durable even without a receipt");
        let parsed = SealedEnvelope::from_bytes(&stored).unwrap();
        let recovered = open(
            &parsed,
            &PayerSecretKey::Secp256k1(Box::new(sk)),
            ctx.payment_id.as_bytes(),
        )
        .unwrap();
        assert_eq!(recovered, body);
    }

    #[tokio::test]
    async fn an_oversized_body_is_skipped_not_failed() {
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                max_body_bytes: 16,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let evidence = hook
            .capture(
                &[0u8; 64],
                Ok(PayerPublicKey::Secp256k1(Box::new(sk.public_key()))),
                &ctx(),
            )
            .await;
        assert_eq!(evidence, DurableEvidence::skipped(SkipReason::TooLarge));
    }

    #[tokio::test]
    async fn a_payer_without_a_recoverable_key_is_skipped() {
        // Smart-contract wallets land here. They must still get their response.
        let hook = DurableEvidenceHook::new(
            DurableConfig::default(),
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let evidence = hook
            .capture(b"body", Err(SkipReason::NoPayerKey), &ctx())
            .await;
        assert_eq!(evidence, DurableEvidence::skipped(SkipReason::NoPayerKey));
    }

    #[tokio::test]
    async fn a_failing_sink_never_loses_the_response() {
        #[derive(Debug)]
        struct BrokenSink;
        #[async_trait::async_trait]
        impl EvidenceSink for BrokenSink {
            async fn put(&self, _: &str, _: &[u8]) -> Result<DurablePointer, String> {
                Err("disk on fire".into())
            }
        }

        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        let hook = DurableEvidenceHook::new(
            DurableConfig::default(),
            Arc::new(BrokenSink),
            "http://127.0.0.1:1",
        );
        let evidence = hook
            .capture(
                b"body",
                Ok(PayerPublicKey::Secp256k1(Box::new(sk.public_key()))),
                &ctx(),
            )
            .await;
        assert_eq!(evidence, DurableEvidence::skipped(SkipReason::AnchorFailed));
    }

    #[test]
    fn the_header_round_trips() {
        let evidence = DurableEvidence::skipped(SkipReason::TooLarge);
        let encoded = encode_header(&evidence).unwrap();
        assert!(
            !encoded.contains('=') && !encoded.contains('\n'),
            "header value must be a single unpadded line"
        );
        assert_eq!(decode_header(&encoded).unwrap(), evidence);
    }

    #[test]
    fn a_garbage_header_decodes_to_none_rather_than_panicking() {
        assert!(decode_header("").is_none());
        assert!(decode_header("!!!not base64!!!").is_none());
        assert!(decode_header("aGVsbG8").is_none()); // valid base64, not our JSON
    }

    #[tokio::test]
    async fn buffering_respects_the_limit() {
        let body = axum_core::body::Body::from(vec![0u8; 100]);
        match buffer_body(body, 1000).await {
            BufferedBody::Ready(bytes) => assert_eq!(bytes.len(), 100),
            BufferedBody::Skip { .. } => panic!("a body under the limit must be sealed"),
        }
    }

    #[tokio::test]
    async fn an_oversized_body_is_still_delivered_in_full() {
        // The buyer already paid: `settle_after_execution` settles BEFORE the
        // hook runs and the authorization nonce is spent, so a body dropped
        // here is paid-for goods that can never be re-fetched. "No evidence"
        // must never become "no goods".
        use http_body_util::BodyExt;
        let big = axum_core::body::Body::from(vec![7u8; 100]);
        match buffer_body(big, 10).await {
            BufferedBody::Ready(_) => panic!("must not seal an oversized body"),
            BufferedBody::Skip { body, reason } => {
                assert_eq!(reason, SkipReason::TooLarge);
                let delivered = body.collect().await.unwrap().to_bytes();
                assert_eq!(delivered.len(), 100, "the oversized body must survive");
                assert!(delivered.iter().all(|b| *b == 7), "delivered byte-for-byte");
            }
        }
    }

    #[tokio::test]
    async fn a_body_that_announces_it_is_too_big_is_never_buffered() {
        // The size hint is consulted BEFORE collect(). Without that, a
        // multi-gigabyte download is held whole in memory just to be measured
        // and thrown away -- an OOM of the task rather than a skip.
        use http_body::Body as _;
        let big = axum_core::body::Body::from(vec![3u8; 4096]);
        assert_eq!(big.size_hint().upper(), Some(4096), "precondition");
        match buffer_body(big, 16).await {
            BufferedBody::Ready(_) => panic!("must not seal"),
            BufferedBody::Skip { reason, .. } => assert_eq!(reason, SkipReason::TooLarge),
        }
    }
}
