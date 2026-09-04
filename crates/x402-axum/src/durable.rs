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
//! Every failure here -- oversized body, exhausted memory budget, unreachable
//! sink, unrecoverable payer key -- resolves to a [`SkipReason`] in the header,
//! and every one of them is counted ([`EvidenceStats`]). The buyer still
//! gets their bytes and the settlement still stands. That is not defensive
//! coding, it is the design constraint: evidence is an addition to the payment
//! path, never a gate in front of it.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use x402_rs::dx402::envelope::{seal, PayerPublicKey};
use x402_rs::dx402::types::{
    AnchorRequest, DurableEvidence, DurableEvidenceConfig, DurablePointer, EvidenceMode, Retention,
    SkipReason, StorageBackend, EVIDENCE_HEADER,
};
use x402_rs::network::Network;
use x402_rs::types::MixedAddress;

/// Largest body DX402 will seal by default: 32 MiB.
///
/// Not a storage ceiling -- S3 accepts 5 GiB in a single `PUT`, and in pointer
/// mode the object lands in the seller's own sink anyway. It is a *memory*
/// ceiling: sealing buffers the plaintext, then the ciphertext, then whatever
/// copy the sink makes to put it on the wire, so one capture costs several
/// times the body.
///
/// Why this number and not a rounder one. The only real data point is the
/// incident that prompted a configurable limit at all: an **18 MB** API
/// response that got `too_large` and no evidence. A default under that fails
/// the exact case it was raised for, so 2 or 4 MiB is not an option. Above it,
/// the deciding fact is that this constant ships in *other people's processes*
/// -- `DurableConfig::default()` is what a seller gets for not thinking about
/// it, on a host whose memory we do not size. A library default belongs at the
/// smallest number that clears the known case with room, not at the largest one
/// our own infrastructure could absorb. 32 MiB leaves ~78% headroom over the 18
/// MB case at half the footprint of 64.
///
/// Raising it is one environment variable. Lowering it after integrators have
/// built against a bigger promise is a regression, which is the asymmetry that
/// settles the direction of error.
pub const DEFAULT_MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Memory all in-flight captures may reserve at once, by default: 160 MiB.
///
/// This is the field that turns raising the body limit from a hazard into a
/// setting. With a body limit and nothing bounding concurrency, a burst of
/// large responses in parallel is an OOM -- and an OOM drops responses that
/// were already paid for. Bounded, the same burst is an ordered
/// [`SkipReason::Busy`]: no evidence for the overflow, but every buyer still
/// gets their bytes.
///
/// **This is not memory the process takes, it is memory it refuses to exceed.**
/// Reservations are sized from each body, so a seller returning 4 KB of JSON
/// never holds more than a few KB no matter how high this sits. It costs
/// nothing until large bodies actually flow, which is why it is the generous
/// half of the pair and `max_body_bytes` is the conservative one.
///
/// It is [`DEFAULT_MAX_BODY_BYTES`] times [`MEMORY_AMPLIFICATION`], so exactly
/// one worst-case capture fits and the second skips in order. That is one
/// decision, not two, and it has moved twice with the measurement: 128 MiB when
/// the factor was a guess of 4, up to 192 MiB when measuring said 5, and back
/// down to 160 MiB once the envelope stopped reallocating and the real number
/// turned out to be 4. Both times the body ceiling stayed at 32 MiB and the
/// budget followed it, rather than the other way round -- the clamp would
/// otherwise have cut the ceiling below the 18 MB incident that justified it.
pub const DEFAULT_MAX_INFLIGHT_BYTES: usize = 160 * 1024 * 1024;

/// Floor for `max_body_bytes`. A mis-parsed or hostile value must not be able
/// to leave the limit at zero, which would silently skip everything.
pub const MIN_MAX_BODY_BYTES: usize = 16 * 1024;

/// How many times the body size one capture really costs in memory.
///
/// **Measured, not estimated.** `tests/memory_amplification.rs` runs a whole
/// capture under a counting allocator and asserts this number still covers the
/// peak, in both debug and release, from 1 MiB up to the ceiling itself.
///
/// The history is worth keeping because it is what the measurement is for. It
/// was first written as 4 by counting the copies one can see -- plaintext,
/// ciphertext, the `to_bytes()` copy, the sink's copy -- and measured **5.0x**.
/// The fifth body was `SealedEnvelope::to_bytes` reserving 64 bytes for a
/// 115-byte header, so every seal overflowed its reservation by a hair and
/// `RawVec` doubled the whole ciphertext to absorb it. Reserving the real
/// header brought the measurement to a flat **4.0x**, which is what the four
/// visible copies said all along.
///
/// So this is 5: the measured peak plus one body of slack. Not 4, because a
/// guard that sits exactly on the measurement has no room for a copy someone
/// adds later without re-running this.
///
/// Public so the test can hold it to account. An OOM guard sized by an estimate
/// nobody ever checks is the guard that lets the burst through.
pub const MEMORY_AMPLIFICATION: usize = 5;

/// Per-route DX402 configuration.
#[derive(Debug, Clone)]
pub struct DurableConfig {
    pub mode: EvidenceMode,
    pub backend: StorageBackend,
    pub retention: Retention,
    /// Bodies above this are skipped. A large body is a reason to skip evidence,
    /// never a reason to fail a payment.
    pub max_body_bytes: usize,
    /// Ceiling on the memory the evidence path may hold across all concurrent
    /// captures. See [`DEFAULT_MAX_INFLIGHT_BYTES`].
    pub max_inflight_bytes: usize,
}

impl Default for DurableConfig {
    fn default() -> Self {
        Self {
            mode: EvidenceMode::Direct,
            backend: StorageBackend::S3,
            retention: Retention::Days90,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_inflight_bytes: DEFAULT_MAX_INFLIGHT_BYTES,
        }
    }
}

impl DurableConfig {
    /// Read the two size limits from the environment, keeping every other
    /// default.
    ///
    /// `DX402_MAX_BODY_BYTES` and `DX402_MAX_INFLIGHT_BYTES`, both in bytes. A
    /// value that does not parse is ignored in favour of the default and
    /// logged: a typo in a deployment variable must not get to decide how much
    /// memory the process may use.
    pub fn from_env() -> Self {
        let config = Self {
            max_body_bytes: env_bytes("DX402_MAX_BODY_BYTES").unwrap_or(DEFAULT_MAX_BODY_BYTES),
            max_inflight_bytes: env_bytes("DX402_MAX_INFLIGHT_BYTES")
                .unwrap_or(DEFAULT_MAX_INFLIGHT_BYTES),
            ..Self::default()
        }
        .sanitized();
        #[cfg(feature = "telemetry")]
        tracing::info!(
            max_body_bytes = config.max_body_bytes,
            max_inflight_bytes = config.max_inflight_bytes,
            "DX402 evidence size limits configured"
        );
        config
    }

    /// Apply the floor, then keep the body limit inside the memory budget.
    ///
    /// The floor is for values that came from outside the program -- an empty
    /// or fat-fingered environment variable that would otherwise leave the
    /// limit at zero and skip everything in silence. It is deliberately NOT
    /// applied to a config a caller wrote by hand: a test or a route that means
    /// 16 bytes gets 16 bytes.
    pub fn sanitized(mut self) -> Self {
        self.max_inflight_bytes = self
            .max_inflight_bytes
            .max(MIN_MAX_BODY_BYTES * MEMORY_AMPLIFICATION);
        self.max_body_bytes = self.max_body_bytes.max(MIN_MAX_BODY_BYTES);
        self.clamped_to_budget()
    }

    /// Keep the body limit inside what the memory budget can afford.
    ///
    /// A body limit the budget cannot cover is worse than a small one: every
    /// large response reserves more memory than exists and skips as
    /// [`SkipReason::Busy`] forever, which reads as a capacity problem rather
    /// than as the misconfiguration it is. Clamping turns it back into an
    /// honest `too_large`. This one applies to every config, hand-written
    /// included, because it is the invariant the budget depends on.
    fn clamped_to_budget(mut self) -> Self {
        let affordable = self.max_inflight_bytes / MEMORY_AMPLIFICATION;
        if self.max_body_bytes > affordable {
            #[cfg(feature = "telemetry")]
            tracing::warn!(
                requested = self.max_body_bytes,
                affordable,
                max_inflight_bytes = self.max_inflight_bytes,
                "DX402 body limit exceeds the memory budget; clamping"
            );
            self.max_body_bytes = affordable;
        }
        self
    }
}

fn env_bytes(name: &str) -> Option<usize> {
    parse_bytes(name, std::env::var(name).ok().as_deref())
}

// Split out of `env_bytes` so it can be tested without setting a process-global
// variable from a test that runs alongside others.
//
// Not `.ok()`: the `Err` arm exists to say out loud that the value was
// unusable. A variable that silently means "default" is how a deployment ends
// up running limits nobody chose.
#[allow(clippy::manual_ok_err)]
fn parse_bytes(_name: &str, raw: Option<&str>) -> Option<usize> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<usize>() {
        Ok(n) => Some(n),
        Err(_) => {
            #[cfg(feature = "telemetry")]
            tracing::warn!(
                var = _name,
                value = trimmed,
                "unparseable; using the default"
            );
            None
        }
    }
}

/// A byte-denominated permit pool bounding the memory the evidence path may
/// hold at any instant.
///
/// Deliberately non-blocking: [`EvidenceBudget::try_reserve`] refuses instead of
/// queueing. The buffering it guards happens *before* the buyer's response goes
/// out, so waiting for a permit would hold up a delivery that has already been
/// settled and paid for. Delivery wins; evidence is what gives way.
#[derive(Debug)]
pub struct EvidenceBudget {
    limit: usize,
    reserved: AtomicUsize,
}

impl EvidenceBudget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: limit.max(1),
            reserved: AtomicUsize::new(0),
        })
    }

    /// Take `bytes` out of the budget, or refuse.
    ///
    /// A zero-byte reservation always succeeds: it stands for a capture that is
    /// about to be skipped without buffering anything, and charging it would
    /// evict captures that would actually have used the memory.
    pub fn try_reserve(self: &Arc<Self>, bytes: usize) -> Option<EvidencePermit> {
        let want = bytes;
        let mut current = self.reserved.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(want)?;
            if next > self.limit {
                return None;
            }
            match self.reserved.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(EvidencePermit {
                        budget: Arc::clone(self),
                        bytes: want,
                    })
                }
                Err(observed) => current = observed,
            }
        }
    }

    pub fn limit_bytes(&self) -> usize {
        self.limit
    }

    pub fn reserved_bytes(&self) -> usize {
        self.reserved.load(Ordering::Relaxed)
    }
}

/// Holds a reservation for as long as the capture needs the memory.
///
/// Released on drop, which covers every early return in the capture path --
/// including the ones that skip. A permit leaked by an error path would shrink
/// the budget permanently and turn a healthy deployment into one that reports
/// [`SkipReason::Busy`] for everything.
#[derive(Debug)]
pub struct EvidencePermit {
    budget: Arc<EvidenceBudget>,
    bytes: usize,
}

impl EvidencePermit {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for EvidencePermit {
    fn drop(&mut self) {
        self.budget.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Counts what the evidence path did, so a skip is not a silence.
///
/// Every reason here is a normal outcome that leaves the payment intact, which
/// is exactly why it needs counting: nothing upstream fails, nothing pages, and
/// without a number nobody can tell "no responses were too large" from "every
/// response was too large".
#[derive(Debug, Default)]
pub struct EvidenceStats {
    anchored: AtomicU64,
    too_large: AtomicU64,
    busy: AtomicU64,
    anchor_failed: AtomicU64,
    no_payer_key: AtomicU64,
    disabled: AtomicU64,
    /// The buyer paid for the plain offer. Counted apart from every failure
    /// because it is not one: a route where this dominates is a route whose
    /// buyers are choosing, not a hook that is broken.
    not_selected: AtomicU64,
    /// A reason from a newer facilitator this build does not know.
    unknown: AtomicU64,
}

/// A point-in-time read of [`EvidenceStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvidenceCounts {
    pub anchored: u64,
    pub too_large: u64,
    pub busy: u64,
    pub anchor_failed: u64,
    pub no_payer_key: u64,
    pub disabled: u64,
    pub not_selected: u64,
    pub unknown: u64,
}

impl EvidenceStats {
    pub fn record_skip(&self, reason: SkipReason) {
        let counter = match reason {
            SkipReason::TooLarge => &self.too_large,
            SkipReason::Busy => &self.busy,
            SkipReason::AnchorFailed => &self.anchor_failed,
            SkipReason::NoPayerKey => &self.no_payer_key,
            SkipReason::Disabled => &self.disabled,
            SkipReason::NotSelected => &self.not_selected,
            SkipReason::Unknown => &self.unknown,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn record(&self, evidence: &DurableEvidence) {
        match evidence {
            DurableEvidence::Anchored(_) => {
                self.anchored.fetch_add(1, Ordering::Relaxed);
            }
            DurableEvidence::Skipped(skipped) => self.record_skip(skipped.skipped),
        }
    }

    pub fn snapshot(&self) -> EvidenceCounts {
        EvidenceCounts {
            anchored: self.anchored.load(Ordering::Relaxed),
            too_large: self.too_large.load(Ordering::Relaxed),
            busy: self.busy.load(Ordering::Relaxed),
            anchor_failed: self.anchor_failed.load(Ordering::Relaxed),
            no_payer_key: self.no_payer_key.load(Ordering::Relaxed),
            disabled: self.disabled.load(Ordering::Relaxed),
            not_selected: self.not_selected.load(Ordering::Relaxed),
            unknown: self.unknown.load(Ordering::Relaxed),
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
    /// The declaration on the offer the buyer actually paid for, when the
    /// route offered evidence as a choice.
    ///
    /// `retention` and `mode` come from HERE when present, not from the route's
    /// fixed config: the buyer paid for a specific promise, and the anchor has
    /// to keep that one.
    pub offer: Option<DurableEvidenceConfig>,
}

/// What the hook should do for one paid request, decided from the offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferDecision {
    /// The route offered evidence and the buyer paid for the offer without it.
    /// Deliver, and say so -- silence would look like a seller that failed.
    NotSelected,
    /// The buyer paid for the offer that declares it. Anchor on ITS terms.
    Declared(DurableEvidenceConfig),
    /// No offer on this route declares anything: the hook was attached to the
    /// whole route, the way it worked before offers existed. Anchor on the
    /// route's terms, so nobody who integrated earlier changes behaviour.
    Legacy,
}

impl OfferDecision {
    /// Decide from the full `accepts` array and the requirement that was paid.
    ///
    /// Pure, so the rule is testable without a request in flight. Both inputs
    /// are needed: the paid requirement alone cannot distinguish "declined" from
    /// "was never offered", and those must not produce the same outcome -- one
    /// is a buyer's choice, the other is a seller who never opted in and whose
    /// existing anchors must keep flowing.
    pub fn decide(
        accepts: &[x402_rs::types::PaymentRequirements],
        paid: &x402_rs::types::PaymentRequirements,
    ) -> Self {
        match DurableEvidenceConfig::from_requirements(paid) {
            Some(cfg) => OfferDecision::Declared(cfg),
            // Declared but unparseable on the PAID offer: the buyer paid for
            // terms nobody can read. Fail closed -- no evidence, and the header
            // says so -- rather than anchor under terms they did not buy.
            None if DurableEvidenceConfig::declared_on(paid) => OfferDecision::NotSelected,
            None if DurableEvidenceConfig::offered_in(accepts) => OfferDecision::NotSelected,
            None => OfferDecision::Legacy,
        }
    }
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
    /// Bounds the memory concurrent captures may hold. Shared by clone, so a
    /// hook handed to several routes still adds up to one budget.
    budget: Arc<EvidenceBudget>,
    stats: Arc<EvidenceStats>,
}

impl DurableEvidenceHook {
    pub fn new(
        config: DurableConfig,
        sink: Arc<dyn EvidenceSink>,
        facilitator_base: impl Into<String>,
    ) -> Self {
        let config = config.clamped_to_budget();
        let budget = EvidenceBudget::new(config.max_inflight_bytes);
        Self {
            config,
            sink,
            facilitator_base: facilitator_base.into().trim_end_matches('/').to_string(),
            client: evidence_http_client(),
            anchor_signer: None,
            budget,
            stats: Arc::new(EvidenceStats::default()),
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

    /// The `(mode, retention)` this capture anchors under.
    ///
    /// From the paid offer when the buyer chose one, else from the route. A
    /// buyer who paid extra for `permanent` and got `90d` has a receipt that
    /// contradicts what they bought.
    pub fn effective_terms(&self, ctx: &SettledContext) -> (EvidenceMode, Retention) {
        match &ctx.offer {
            Some(offer) => (offer.mode, offer.retention),
            None => (self.config.mode, self.config.retention),
        }
    }

    pub fn budget(&self) -> &Arc<EvidenceBudget> {
        &self.budget
    }

    /// Counters for anchored and skipped captures. See [`EvidenceStats`].
    pub fn stats(&self) -> EvidenceCounts {
        self.stats.snapshot()
    }

    /// Record a skip decided outside [`Self::capture`] -- the oversized body
    /// that `buffer_body` refuses, for instance.
    pub fn record_skip(&self, reason: SkipReason) {
        self.stats.record_skip(reason);
    }

    /// Reserve the memory one capture may need, before a single byte is
    /// buffered.
    ///
    /// Sized from the body's own `size_hint` when it has one and from the
    /// configured ceiling when it does not -- an unknown-length body has to be
    /// assumed to be the largest thing this deployment would accept. A body
    /// that already announces itself over the limit reserves nothing: it is
    /// about to be skipped as `too_large` without ever being buffered, and
    /// charging it against the budget would push out captures that could
    /// actually have succeeded.
    pub fn reserve_for(&self, body: &axum_core::body::Body) -> Result<EvidencePermit, SkipReason> {
        use http_body::Body as _;

        let limit = self.config.max_body_bytes as u64;
        let announced = body.size_hint().upper().unwrap_or(limit);
        let want = if announced > limit {
            0
        } else {
            announced as usize
        };
        match self
            .budget
            .try_reserve(want.saturating_mul(MEMORY_AMPLIFICATION))
        {
            Some(permit) => Ok(permit),
            None => {
                #[cfg(feature = "telemetry")]
                tracing::warn!(
                    wanted = want,
                    reserved = self.budget.reserved_bytes(),
                    limit = self.budget.limit_bytes(),
                    "DX402 evidence budget exhausted; delivering without evidence"
                );
                self.stats.record_skip(SkipReason::Busy);
                Err(SkipReason::Busy)
            }
        }
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
        let evidence = self.capture_inner(body, payer_key, ctx).await;
        self.stats.record(&evidence);
        evidence
    }

    async fn capture_inner(
        &self,
        body: &[u8],
        payer_key: Result<PayerPublicKey, SkipReason>,
        ctx: &SettledContext,
    ) -> DurableEvidence {
        // The buyer paid for a specific offer; its terms win over the route's.
        let (offer_mode, offer_retention) = self.effective_terms(ctx);
        if body.len() > self.config.max_body_bytes {
            #[cfg(feature = "telemetry")]
            tracing::warn!(
                body_bytes = body.len(),
                limit = self.config.max_body_bytes,
                "DX402 body over the limit; delivering without evidence"
            );
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
            mode: offer_mode,
            retention: offer_retention,
            wrapped_cek: None,
            // This hook sits in the seller's own response path, where the buyer
            // paid it directly and the ERC-20 `from` IS the buyer. The escrow
            // authorization only exists on the x402r rail, where a marketplace
            // releases funds on the seller's behalf and this hook is not the one
            // anchoring -- see `escrowRelease` in docs/DX402.md.
            escrow_release: None,
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
/// Bounded by `limit` per body; the number of bodies that may be in here at once
/// is bounded separately by [`EvidenceBudget`], claimed by the caller through
/// [`DurableEvidenceHook::reserve_for`] before this runs. One without the other
/// is not a bound: a generous `limit` and unbounded concurrency is an OOM, and
/// an OOM drops responses that were already settled.
///
/// Whatever happens, the caller gets back something it can
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

    // Bound the COLLECTION, not just the result. The check above only catches a
    // body that ANNOUNCES its size, and a chunked one announces nothing:
    // `upper()` is `None`, it sails past the guard, and an unbounded `collect()`
    // buffers however many bytes arrive before anyone measures them. For a
    // streaming handler -- exactly the large-body case -- `max_body_bytes` was
    // therefore not a memory bound at all, and `EvidenceBudget`, which exists to
    // stop that OOM, was charging a number the body had no obligation to honour.
    //
    // Read frame by frame and stop at the limit. What is already buffered is
    // handed back AHEAD of the untouched remainder, so stopping early costs the
    // evidence and never a byte of the response.
    //
    // `http_body_util::Limited` looks like it fits here and does not: it reports
    // the overflow as a stream ERROR, and the error arm below has nothing left
    // to deliver. That would answer a paid request with an empty body -- the one
    // outcome this whole path exists to prevent, since settlement happened
    // before the hook and the nonce is already spent.
    let mut body = body;
    let mut buffered: Vec<bytes::Bytes> = Vec::new();
    let mut buffered_len = 0usize;
    loop {
        match body.frame().await {
            Some(Ok(frame)) => {
                let Ok(data) = frame.into_data() else {
                    // Trailers carry no payload; nothing to measure or keep.
                    continue;
                };
                buffered_len += data.len();
                buffered.push(data);
                if buffered_len > limit {
                    return BufferedBody::Skip {
                        body: prefixed_body(buffered, body),
                        reason: SkipReason::TooLarge,
                    };
                }
            }
            Some(Err(_)) => {
                return BufferedBody::Skip {
                    body: prefixed_body(buffered, body),
                    reason: SkipReason::AnchorFailed,
                }
            }
            None => break,
        }
    }

    BufferedBody::Ready(concat(buffered, buffered_len))
}

/// The frames already read, ahead of whatever is still coming.
///
/// Exists so a capture can give up mid-stream without the buyer paying for it.
fn prefixed_body(
    buffered: Vec<bytes::Bytes>,
    rest: axum_core::body::Body,
) -> axum_core::body::Body {
    let len = buffered.iter().map(|b| b.len()).sum();
    axum_core::body::Body::new(Prefixed {
        prefix: Some(concat(buffered, len)),
        rest,
    })
}

fn concat(mut buffered: Vec<bytes::Bytes>, len: usize) -> bytes::Bytes {
    // One chunk is the common case by far -- do not copy it.
    if buffered.len() == 1 {
        return buffered.pop().unwrap_or_default();
    }
    let mut out = Vec::with_capacity(len);
    for chunk in buffered {
        out.extend_from_slice(&chunk);
    }
    out.into()
}

/// A body that emits some already-read bytes, then delegates.
///
/// Hand-written rather than pulled from a combinator crate: `x402-axum` has no
/// stream-adapter dependency, and adding one to concatenate two bodies is a
/// large debt for a small job.
struct Prefixed {
    prefix: Option<bytes::Bytes>,
    rest: axum_core::body::Body,
}

impl http_body::Body for Prefixed {
    type Data = bytes::Bytes;
    type Error = axum_core::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if let Some(prefix) = this.prefix.take() {
            if !prefix.is_empty() {
                return std::task::Poll::Ready(Some(Ok(http_body::Frame::data(prefix))));
            }
        }
        std::pin::Pin::new(&mut this.rest).poll_frame(cx)
    }

    fn size_hint(&self) -> http_body::SizeHint {
        // Deliberately not summed with the remainder's: the remainder is a
        // stream of unknown length, which is how we got here.
        http_body::SizeHint::default()
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
            offer: None,
        }
    }

    fn offer(extra: Option<serde_json::Value>) -> x402_rs::types::PaymentRequirements {
        let mut v = serde_json::json!({
            "scheme": "exact", "network": "base", "maxAmountRequired": "10000",
            "resource": "https://kk.example/data/42", "description": "d",
            "mimeType": "application/json",
            "payTo": "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8",
            "maxTimeoutSeconds": 300,
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        });
        if let Some(extra) = extra {
            v["extra"] = extra;
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn paying_for_the_plain_offer_declines_evidence() {
        // The buyer's side of the opt-in. The route offers both; the buyer pays
        // for the cheaper one; no evidence is produced AND the buyer is told
        // why. Same bytes delivered either way.
        let plain = offer(None);
        let mut durable = offer(None);
        DurableEvidenceConfig::default().declare_on(&mut durable);
        let accepts = vec![plain.clone(), durable.clone()];

        assert_eq!(
            OfferDecision::decide(&accepts, &plain),
            OfferDecision::NotSelected
        );
        assert_eq!(
            OfferDecision::decide(&accepts, &durable),
            OfferDecision::Declared(DurableEvidenceConfig::default())
        );
    }

    #[test]
    fn a_malformed_declaration_anchors_nobody() {
        // Before: unparseable -> "not offered" -> Legacy -> every buyer on the
        // route anchored under the route's terms, including the ones who paid
        // for the plain offer. The safe failure for a consent feature is the
        // other way round.
        let plain = offer(None);
        let broken = offer(Some(serde_json::json!({
            "extensions": {"durable-evidence": {"retention": "forever-and-ever"}}
        })));
        let accepts = vec![plain.clone(), broken.clone()];
        assert_eq!(
            OfferDecision::decide(&accepts, &plain),
            OfferDecision::NotSelected
        );
        assert_eq!(
            OfferDecision::decide(&accepts, &broken),
            OfferDecision::NotSelected,
            "the buyer paid for terms nobody can read: no evidence, said out loud"
        );
    }

    #[test]
    fn a_route_that_never_offered_a_choice_keeps_anchoring() {
        // Everyone who attached the hook before offers existed anchors every
        // paid response. That must not silently become "never", which is what
        // treating an undeclared requirement as "declined" would do.
        let plain = offer(None);
        assert_eq!(
            OfferDecision::decide(std::slice::from_ref(&plain), &plain),
            OfferDecision::Legacy
        );
    }

    #[test]
    fn the_paid_offer_sets_the_terms_not_the_route() {
        // A buyer who paid for a year and was anchored for 90 days holds a
        // receipt that contradicts what they bought.
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                retention: Retention::Days90,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let chosen = DurableEvidenceConfig {
            retention: Retention::Year1,
            mode: EvidenceMode::Direct,
            ..Default::default()
        };
        let with_offer = SettledContext {
            offer: Some(chosen),
            ..ctx()
        };
        assert_eq!(
            hook.effective_terms(&with_offer),
            (EvidenceMode::Direct, Retention::Year1)
        );
        assert_eq!(
            hook.effective_terms(&ctx()),
            (EvidenceMode::Direct, Retention::Days90),
            "no offer: the route's terms, as before"
        );
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
    #[test]
    fn the_default_limit_fits_the_default_budget_exactly() {
        // The two defaults are one decision, not two: 32 MiB of body at the
        // MEASURED amplification is exactly the budget, so a single worst-case
        // capture fits and a second one skips rather than allocating. The
        // budget is written in terms of the factor on purpose -- when the
        // measurement moved it from 4 to 6, holding the old 128 MiB would have
        // silently clamped the body limit down to ~21 MiB instead.
        let c = DurableConfig::default();
        assert_eq!(c.max_body_bytes, 32 * 1024 * 1024);
        assert_eq!(
            c.max_inflight_bytes,
            c.max_body_bytes * MEMORY_AMPLIFICATION
        );
        assert_eq!(
            c.max_body_bytes * MEMORY_AMPLIFICATION,
            c.max_inflight_bytes
        );
        assert_eq!(
            DurableConfig::default().sanitized().max_body_bytes,
            c.max_body_bytes
        );
    }

    #[test]
    fn a_body_limit_the_budget_cannot_afford_is_clamped_not_honoured() {
        // Left unclamped this configuration reports `busy` for every large
        // response forever, which reads as a capacity problem instead of the
        // misconfiguration it is.
        const BUDGET: usize = 64 * 1024 * 1024;
        let c = DurableConfig {
            max_body_bytes: 512 * 1024 * 1024,
            max_inflight_bytes: BUDGET,
            ..DurableConfig::default()
        }
        .sanitized();
        // Whatever the budget can actually pay for at the measured factor --
        // spelled out rather than hardcoded, so this stays true the next time
        // the measurement moves.
        assert_eq!(c.max_body_bytes, BUDGET / MEMORY_AMPLIFICATION);
    }

    #[test]
    fn the_default_clears_the_incident_that_prompted_it() {
        // An 18 MB response is the only measured case DX402 ever refused. A
        // default that does not cover it fails the reason the limit was made
        // configurable in the first place.
        const OBSERVED_INCIDENT_BYTES: usize = 18_000_000;
        assert!(
            DurableConfig::default().max_body_bytes > OBSERVED_INCIDENT_BYTES,
            "the default must seal the body that motivated the limit"
        );
    }

    #[test]
    fn a_zero_limit_is_raised_to_the_floor() {
        // `DX402_MAX_BODY_BYTES=0` would otherwise skip every response in
        // silence.
        let c = DurableConfig {
            max_body_bytes: 0,
            max_inflight_bytes: 0,
            ..DurableConfig::default()
        }
        .sanitized();
        assert_eq!(c.max_body_bytes, MIN_MAX_BODY_BYTES);
        assert!(c.max_inflight_bytes >= MIN_MAX_BODY_BYTES * MEMORY_AMPLIFICATION);
    }

    #[test]
    fn the_budget_refuses_instead_of_queueing() {
        let budget = EvidenceBudget::new(100);
        let first = budget.try_reserve(60).expect("fits");
        assert!(budget.try_reserve(60).is_none(), "must not oversubscribe");
        assert_eq!(budget.reserved_bytes(), 60);
        drop(first);
        assert_eq!(budget.reserved_bytes(), 0);
        assert!(budget.try_reserve(60).is_some(), "released on drop");
    }

    #[tokio::test]
    async fn a_second_large_response_skips_as_busy_rather_than_allocating() {
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                max_body_bytes: 1024,
                max_inflight_bytes: 1024 * MEMORY_AMPLIFICATION,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let first = hook
            .reserve_for(&axum_core::body::Body::from(vec![0u8; 1024]))
            .expect("the first capture fits the budget");
        let second = hook.reserve_for(&axum_core::body::Body::from(vec![0u8; 1024]));
        assert_eq!(second.err(), Some(SkipReason::Busy));
        assert_eq!(hook.stats().busy, 1, "a skip is not a silence");
        drop(first);
        assert!(
            hook.reserve_for(&axum_core::body::Body::from(vec![0u8; 1024]))
                .is_ok(),
            "the budget frees up once the first capture is done"
        );
    }

    #[tokio::test]
    async fn a_body_already_over_the_limit_does_not_spend_the_budget() {
        // It is about to be skipped as `too_large` without being buffered.
        // Charging it would push out captures that could have succeeded.
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                max_body_bytes: 1024,
                max_inflight_bytes: 1024 * MEMORY_AMPLIFICATION,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let oversized = hook
            .reserve_for(&axum_core::body::Body::from(vec![0u8; 4096]))
            .expect("an oversized body is not a budget problem");
        assert_eq!(
            oversized.bytes(),
            0,
            "nothing will be buffered, so nothing is charged"
        );
        assert!(
            hook.reserve_for(&axum_core::body::Body::from(vec![0u8; 1024]))
                .is_ok(),
            "a real capture still fits alongside it"
        );
    }

    #[tokio::test]
    async fn a_chunked_body_that_lies_about_its_size_is_still_bounded() {
        // A chunked response announces nothing -- `upper()` is `None` -- so the
        // announce-check waves it through. Before this, `collect()` then bought
        // however many bytes the handler chose to send, which made
        // `max_body_bytes` a suggestion for exactly the responses big enough to
        // matter.
        //
        // Counting frames rather than bytes is deliberate: the OLD code also
        // ended in `TooLarge` with the full body in hand, so only "how much did
        // we read before deciding" tells the two apart.
        const LIMIT: usize = 4 * 1024;
        const CHUNKS: usize = 1024; // 1 MiB, 256x the limit
        let polled = Arc::new(AtomicUsize::new(0));
        let body = axum_core::body::Body::new(ChunkedUnknown {
            remaining: CHUNKS,
            polled: Arc::clone(&polled),
        });

        let (returned, reason) = match buffer_body(body, LIMIT).await {
            BufferedBody::Skip { body, reason } => (body, reason),
            BufferedBody::Ready(_) => panic!("1 MiB over a 4 KiB limit must not be captured"),
        };
        assert_eq!(reason, SkipReason::TooLarge);

        let read = polled.load(Ordering::Relaxed);
        assert!(
            read <= LIMIT / 1024 + 1,
            "stopped after {read} chunks; the limit is {} chunks' worth, so the \
             buffer is not bounded by it",
            LIMIT / 1024
        );

        // The half that must never regress: giving up on the evidence returns
        // the WHOLE body, including the part already read. Settlement happened
        // before this hook and the nonce is spent, so a byte lost here is paid
        // goods that can never be re-fetched.
        use http_body_util::BodyExt as _;
        let delivered = returned.collect().await.unwrap().to_bytes();
        assert_eq!(
            delivered.len(),
            CHUNKS * 1024,
            "every byte must still be delivered"
        );
        assert!(
            delivered.iter().all(|b| *b == b'x'),
            "the bytes read before giving up must come back unchanged"
        );
    }

    /// Chunks of a length the body never announces -- what an ordinary
    /// streaming handler looks like from here. Counts how many were handed out,
    /// which is the only way to see whether the buffer stopped early.
    struct ChunkedUnknown {
        remaining: usize,
        polled: Arc<AtomicUsize>,
    }

    impl http_body::Body for ChunkedUnknown {
        type Data = bytes::Bytes;
        type Error = axum_core::Error;

        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            let this = self.get_mut();
            if this.remaining == 0 {
                return std::task::Poll::Ready(None);
            }
            this.remaining -= 1;
            this.polled.fetch_add(1, Ordering::Relaxed);
            std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(
                vec![b'x'; 1024],
            )))))
        }
        // The default `size_hint` reports `upper() == None`, which is the point.
    }

    #[tokio::test]
    async fn a_body_of_unknown_length_reserves_the_worst_case() {
        // No `size_hint` upper bound means it could be anything up to the
        // limit, and the reservation has to assume it is.
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                max_body_bytes: 1024,
                max_inflight_bytes: 1024 * MEMORY_AMPLIFICATION,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        // A body that declines to say how long it is -- what a streaming
        // handler produces.
        struct UnknownLength(Option<Bytes>);
        impl http_body::Body for UnknownLength {
            type Data = Bytes;
            type Error = std::convert::Infallible;
            fn poll_frame(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<http_body::Frame<Bytes>, Self::Error>>> {
                std::task::Poll::Ready(self.0.take().map(|b| Ok(http_body::Frame::data(b))))
            }
        }

        let body = axum_core::body::Body::new(UnknownLength(Some(Bytes::from_static(b"chunk"))));
        {
            use http_body::Body as _;
            assert_eq!(body.size_hint().upper(), None, "precondition");
        }
        let permit = hook.reserve_for(&body).expect("fits on its own");
        assert_eq!(permit.bytes(), 1024 * MEMORY_AMPLIFICATION);
    }

    #[tokio::test]
    async fn skips_are_counted_by_reason() {
        let hook = DurableEvidenceHook::new(
            DurableConfig {
                max_body_bytes: 16,
                ..DurableConfig::default()
            },
            Arc::new(MemorySink::new()),
            "http://127.0.0.1:1",
        );
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        hook.capture(
            &[0u8; 64],
            Ok(PayerPublicKey::Secp256k1(Box::new(sk.public_key()))),
            &ctx(),
        )
        .await;
        hook.capture(b"small", Err(SkipReason::NoPayerKey), &ctx())
            .await;
        let counts = hook.stats();
        assert_eq!(counts.too_large, 1);
        assert_eq!(counts.no_payer_key, 1);
        assert_eq!(counts.anchored, 0);
    }

    #[test]
    fn an_unusable_limit_falls_back_instead_of_being_obeyed() {
        // A typo in a deployment variable must not become a limit. Every one of
        // these means "I could not read a number", and the only safe reading of
        // that is the default -- silently taking 0, or panicking at boot over a
        // knob that is optional, are both worse than carrying on.
        for junk in ["", "   ", "32MB", "32 MiB", "-1", "1e6", "0x20", "abc"] {
            assert_eq!(
                parse_bytes("DX402_MAX_BODY_BYTES", Some(junk)),
                None,
                "{junk:?} is not a byte count and must not be treated as one"
            );
        }
        // An unset variable is the same answer by a different route.
        assert_eq!(parse_bytes("DX402_MAX_BODY_BYTES", None), None);
        // And a real number still gets through, surrounding whitespace included.
        assert_eq!(
            parse_bytes("DX402_MAX_BODY_BYTES", Some(" 16777216 ")),
            Some(16_777_216)
        );
        assert_eq!(parse_bytes("DX402_MAX_BODY_BYTES", Some("0")), Some(0));
    }

    #[test]
    fn a_burst_never_reserves_more_than_the_budget_allows() {
        // The handoff's success criterion, restated for the design that shipped:
        // 50 concurrent large captures must not be able to claim more memory
        // than the budget holds. Note it does NOT say they queue -- waiting for
        // a permit would delay a delivery that is already paid for, so the ones
        // that do not fit are refused outright and skip as `busy`.
        //
        // Reservations only, no allocation: the point is the arithmetic that
        // stands between a burst and an OOM, and 50 real 10 MiB bodies would
        // measure the machine rather than the guard.
        use std::thread;

        const BODY: usize = 10 * 1024 * 1024;
        const BURST: usize = 50;
        let limit = DEFAULT_MAX_INFLIGHT_BYTES;
        let budget = EvidenceBudget::new(limit);
        let charge = BODY * MEMORY_AMPLIFICATION;

        let held: Vec<_> = thread::scope(|scope| {
            let handles: Vec<_> = (0..BURST)
                .map(|_| {
                    let budget = Arc::clone(&budget);
                    scope.spawn(move || budget.try_reserve(charge))
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().unwrap())
                .collect()
        });

        // Some got through -- a budget that refuses everyone is not a budget.
        assert!(
            !held.is_empty(),
            "the burst should not have been shut out entirely"
        );
        // And never more than the budget can pay for.
        assert!(
            held.len() <= limit / charge,
            "{} permits handed out but only {} fit in {limit} bytes",
            held.len(),
            limit / charge
        );
        assert!(
            budget.reserved_bytes() <= limit,
            "reserved {} over a {limit}-byte budget",
            budget.reserved_bytes()
        );

        // The refused ones are the `busy` skips, and they cost nothing: once the
        // winners finish, the budget is whole again rather than leaked away.
        drop(held);
        assert_eq!(
            budget.reserved_bytes(),
            0,
            "permits must return what they took"
        );
    }
}
