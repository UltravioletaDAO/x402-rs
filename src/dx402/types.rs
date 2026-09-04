//! DX402 wire types for the `durable-evidence` extension.
//!
//! Spec: `docs/plans/dx402/02-SPEC-v0.1.md`.
//!
//! Everything here is the *metadata* side of DX402. Plaintext never appears in
//! any of these structures, and in `direct` mode neither does key material --
//! the facilitator is a notary and an index, not a custodian.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::network::Network;
use crate::types::MixedAddress;

/// Current wire version of the extension payload.
pub const DX402_VERSION: u8 = 1;

/// The extension key, as it appears under `extensions` in a payment payload or
/// settle response. Kebab-case, matching the registered convention used by
/// `offer-receipt` and `payment-identifier`.
pub const EXTENSION_KEY: &str = "durable-evidence";

/// Response header carrying the evidence pointer back to the buyer.
pub const EVIDENCE_HEADER: &str = "X-Durable-Evidence";

/// How the content encryption key is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceMode {
    /// End-to-end. The CEK is wrapped to the payer's own public key, recovered
    /// from the payment signature. No third party -- including this facilitator
    /// -- can decrypt. This is the default and the whole point of DX402.
    #[default]
    Direct,
    /// The CEK is wrapped to the facilitator's key and released through
    /// `POST /dx402/recover` against a payer signature.
    ///
    /// This mode exists because a buyer who loses their key otherwise loses the
    /// evidence forever, and refusing to offer it just pushes people to worse
    /// alternatives. It is strictly weaker than [`EvidenceMode::Direct`] and is
    /// recorded in the receipt so that no verifier can mistake one for the other.
    Escrowed,
}

impl EvidenceMode {
    /// EIP-712 encodes the mode as a `uint8`.
    pub fn as_u8(self) -> u8 {
        match self {
            EvidenceMode::Direct => 0,
            EvidenceMode::Escrowed => 1,
        }
    }
}

impl fmt::Display for EvidenceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvidenceMode::Direct => write!(f, "direct"),
            EvidenceMode::Escrowed => write!(f, "escrowed"),
        }
    }
}

/// Which durable store holds the ciphertext.
///
/// The ciphertext is byte-identical across all three; only the pointer syntax
/// differs. That is deliberate: privacy comes from the envelope, not from the
/// backend, so a deployment can migrate between them without a protocol change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// S3 bucket under our own account. Default: no external dependency, no
    /// per-file cost, bounded retention.
    #[default]
    S3,
    /// IPFS with pinning. Content-addressed, verifiable by CID.
    Ipfs,
    /// Arweave. Genuinely permanent, and therefore irrevocable -- see the
    /// security notes in the spec before defaulting anything to this.
    Arweave,
}

/// What a deployment can offer, and what each option actually promises.
///
/// Advertised so a seller can choose from what EXISTS here rather than from a
/// list written into a document, and so the landing page can render the real
/// set instead of a hardcoded one. `revocable` and `public` are not decoration:
/// they are the difference between the products, and they are what a page
/// selling durability tends to leave out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendOffer {
    /// Stable id a caller passes as `storage` on an anchor.
    pub id: String,
    /// Default retention for this backend.
    pub retention: String,
    /// Whether the bytes can actually be removed when retention expires.
    /// `false` means `retentionUntil` -- which the facilitator SIGNS -- cannot
    /// be honoured, only stopped being paid for.
    pub revocable: bool,
    /// Whether anyone can resolve it without going through the facilitator.
    pub public: bool,
    /// Offered right now. A `false` entry is still listed, with a reason, so a
    /// caller can tell "not here" from "not a thing".
    pub enabled: bool,
    /// Why it is not enabled, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageBackend::S3 => write!(f, "s3"),
            StorageBackend::Ipfs => write!(f, "ipfs"),
            StorageBackend::Arweave => write!(f, "arweave"),
        }
    }
}

/// How long the evidence is guaranteed to remain retrievable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    /// 90 days. The default, because anchoring is publishing and an unbounded
    /// default would make an accidental PII anchor unfixable.
    #[default]
    #[serde(rename = "90d")]
    Days90,
    #[serde(rename = "1y")]
    Year1,
    /// Irrevocable. Opt-in only.
    Permanent,
}

impl Retention {
    /// Absolute expiry as a unix timestamp, or `0` for permanent.
    pub fn until(self, anchored_at: u64) -> u64 {
        match self {
            Retention::Days90 => anchored_at + 90 * 86_400,
            Retention::Year1 => anchored_at + 365 * 86_400,
            Retention::Permanent => 0,
        }
    }
}

impl fmt::Display for Retention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Retention::Days90 => write!(f, "90d"),
            Retention::Year1 => write!(f, "1y"),
            Retention::Permanent => write!(f, "permanent"),
        }
    }
}

/// Which ECDH curve was used to wrap the CEK.
///
/// Recorded on the wire so a verifier knows how to interpret the ephemeral
/// public key without having to guess from its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAlg {
    /// secp256k1 ECDH. EVM and secp256k1-keyed XRPL payers.
    #[serde(rename = "ECIES-secp256k1")]
    Secp256k1,
    /// X25519 ECDH, reached by mapping the payer's ed25519 key to Montgomery
    /// form. Solana, NEAR, Stellar, Algorand, Sui, ed25519-keyed XRPL.
    #[serde(rename = "ECIES-X25519")]
    X25519,
}

impl fmt::Display for KeyAlg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyAlg::Secp256k1 => write!(f, "ECIES-secp256k1"),
            KeyAlg::X25519 => write!(f, "ECIES-X25519"),
        }
    }
}

/// Per-route configuration, declared under `extensions["durable-evidence"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DurableEvidenceConfig {
    pub mode: EvidenceMode,
    pub backend: StorageBackend,
    pub retention: Retention,
    /// Bodies larger than this are skipped rather than anchored. A large body is
    /// a reason to skip evidence, never a reason to fail a payment.
    ///
    /// A *memory* ceiling, not a storage one: sealing holds the plaintext and
    /// the ciphertext at once. The seller-side hook pairs it with a byte budget
    /// that bounds how many captures may hold that memory concurrently
    /// (`DurableConfig::max_inflight_bytes` in `x402-axum`), which is what makes
    /// a large value here safe.
    pub max_body_bytes: usize,
    /// Who bears the persistence cost. Informational at this layer; billing is
    /// the resource server's business.
    pub paid_by: PaidBy,
}

/// Where a `PaymentRequirements` entry declares the extension.
///
/// v1 requirements carry free-form `extra`; v2 requirements carry a top-level
/// `extensions` map. Declaring under `extra.extensions["durable-evidence"]` on
/// v1 mirrors the v2 shape exactly, so the v1 -> v2 conversion is a rename and
/// not a redesign. It also leaves the EIP-712 `name`/`version` keys that
/// already live in `extra` untouched.
pub const REQUIREMENTS_EXTENSIONS_KEY: &str = "extensions";

impl DurableEvidenceConfig {
    /// The declaration on one payment requirement, if it carries one.
    ///
    /// This is how the buyer opts in: the seller lists the same resource twice
    /// in `accepts` -- plain, and with this declared at a higher price -- and
    /// whichever the buyer pays for is the one the resource server honours. No
    /// change to the x402 core: the multi-offer `accepts` array already exists,
    /// and a client that does not know the extension picks the plain offer and
    /// everything degrades cleanly.
    ///
    /// Malformed declarations read as "not declared" rather than an error. A
    /// typo in one seller's config must not make its route unpayable.
    pub fn from_requirements(req: &crate::types::PaymentRequirements) -> Option<Self> {
        let raw = req
            .extra
            .as_ref()?
            .get(REQUIREMENTS_EXTENSIONS_KEY)?
            .get(EXTENSION_KEY)?;
        serde_json::from_value(raw.clone()).ok()
    }

    /// Declare this configuration on a payment requirement, in place.
    ///
    /// Merges rather than replaces: `extra` already carries the token's EIP-712
    /// domain on most routes, and dropping it would make the offer unpayable.
    pub fn declare_on(&self, req: &mut crate::types::PaymentRequirements) {
        let mut extra = match req.extra.take() {
            Some(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        let extensions = extra
            .entry(REQUIREMENTS_EXTENSIONS_KEY)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !extensions.is_object() {
            *extensions = serde_json::Value::Object(serde_json::Map::new());
        }
        extensions
            .as_object_mut()
            .expect("just made it an object")
            .insert(
                EXTENSION_KEY.to_string(),
                serde_json::to_value(self).expect("config serializes"),
            );
        req.extra = Some(serde_json::Value::Object(extra));
    }

    /// Whether a requirement carries the declaration key at all -- valid or not.
    ///
    /// Separate from [`Self::from_requirements`] on purpose. A malformed
    /// declaration must read as "not usable" for the terms but as "present"
    /// for the choice: treating it as absent made the hook fall back to the
    /// route-wide behaviour and anchor every buyer, including the ones who
    /// paid for the plain offer. For an extension whose premise is consent,
    /// the safe failure is anchoring nobody (red team, 2026-09-04).
    pub fn declared_on(req: &crate::types::PaymentRequirements) -> bool {
        req.extra
            .as_ref()
            .and_then(|e| e.get(REQUIREMENTS_EXTENSIONS_KEY))
            .and_then(|x| x.get(EXTENSION_KEY))
            .is_some()
    }

    /// Whether any offer in an `accepts` array declares the extension.
    ///
    /// The switch the seller-side hook keys on: once a route offers evidence as
    /// a choice, paying for the offer WITHOUT it means the buyer declined.
    /// Presence, not validity -- see [`Self::declared_on`].
    pub fn offered_in(accepts: &[crate::types::PaymentRequirements]) -> bool {
        accepts.iter().any(Self::declared_on)
    }
}

/// The `info` half of the top-level declaration.
///
/// `extensions["durable-evidence"] = { info, schema }` on the 402, next to
/// `accepts`. The configuration is flattened into `info`, and `acceptIndexes`
/// says which offers carry it -- the mechanism `offer-receipt` uses, and the
/// only way to say "this offer, not that one" without nesting the extension
/// under a single requirement, which the registry convention does not do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableEvidenceInfo {
    #[serde(flatten)]
    pub config: DurableEvidenceConfig,
    /// Positions in `accepts` of the offers that include evidence. Empty means
    /// "declared but applying to no offer", which the hook treats as declined.
    #[serde(default)]
    pub accept_indexes: Vec<usize>,
}

impl DurableEvidenceInfo {
    /// JSON Schema for `info`, published alongside it as the core spec asks.
    pub fn schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "mode": {"type": "string", "enum": ["direct", "escrowed"]},
                "backend": {"type": "string", "enum": ["s3", "ipfs", "arweave"]},
                "retention": {"type": "string", "enum": ["90d", "1y", "permanent"]},
                "maxBodyBytes": {"type": "integer", "minimum": 0},
                "paidBy": {"type": "string", "enum": ["seller", "buyer"]},
                "acceptIndexes": {"type": "array", "items": {"type": "integer", "minimum": 0}}
            },
            "additionalProperties": false
        })
    }

    /// Write the `{ info, schema }` object into a challenge's `extensions`.
    pub fn declare(&self, extensions: &mut std::collections::HashMap<String, serde_json::Value>) {
        extensions.insert(
            EXTENSION_KEY.to_string(),
            serde_json::json!({
                "info": serde_json::to_value(self).expect("info serializes"),
                "schema": Self::schema(),
            }),
        );
    }

    /// The declaration on a challenge, if it carries a readable one.
    pub fn from_extensions(
        extensions: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Option<Self> {
        let info = extensions.get(EXTENSION_KEY)?.get("info")?;
        serde_json::from_value(info.clone()).ok()
    }

    /// Whether the key is present at all, readable or not.
    pub fn declared_in(extensions: &std::collections::HashMap<String, serde_json::Value>) -> bool {
        extensions.contains_key(EXTENSION_KEY)
    }
}

impl Default for DurableEvidenceConfig {
    fn default() -> Self {
        Self {
            mode: EvidenceMode::default(),
            backend: StorageBackend::default(),
            retention: Retention::default(),
            max_body_bytes: 33_554_432,
            paid_by: PaidBy::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaidBy {
    #[default]
    Seller,
    Buyer,
}

/// A locator for anchored ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurablePointer(pub String);

impl DurablePointer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DurablePointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why no evidence was produced for a payment that otherwise succeeded.
///
/// Every variant here is a normal outcome, not an error. The payment settled and
/// the buyer got their response; only the durability guarantee is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Body exceeded `max_body_bytes`.
    TooLarge,
    /// The store was unreachable or rejected the write.
    AnchorFailed,
    /// The evidence path was already holding as much memory as it is allowed to.
    ///
    /// Distinct from [`SkipReason::AnchorFailed`] on purpose: nothing was
    /// broken and nothing was rejected, the deployment simply refused to buffer
    /// one more large body. Reporting it as a store failure would send the next
    /// investigation at the store.
    Busy,
    /// The payer's public key could not be recovered for this network family,
    /// so there is nobody to encrypt to.
    NoPayerKey,
    /// The extension is switched off on this deployment.
    Disabled,
    /// The route offered evidence and the buyer paid for the offer without it.
    ///
    /// Not a failure of anything: the buyer chose. Reported so a client that
    /// expected evidence can tell "I picked the plain offer" from "the seller
    /// could not anchor", which otherwise look identical -- no header at all.
    NotSelected,
    /// A reason this reader does not know.
    ///
    /// The set grows. Without a catch-all, `DurableEvidence` (untagged) failed
    /// BOTH arms on a new word and `decode_header` returned `None` -- the whole
    /// notice dropped, pointer included, over one string. Found by auditing the
    /// spec's "clients MUST treat `skipped` as an open set" against our own
    /// reader, which did not.
    #[serde(other)]
    Unknown,
}

/// The `durable-evidence` payload: what rides in the `X-Durable-Evidence` header
/// and under `SettleResponse.extensions["durable-evidence"]`.
///
/// Serialized as either an anchored record or a skip notice, never both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DurableEvidence {
    Anchored(Box<AnchoredEvidence>),
    Skipped(SkippedEvidence),
}

impl DurableEvidence {
    pub fn skipped(reason: SkipReason) -> Self {
        DurableEvidence::Skipped(SkippedEvidence {
            v: DX402_VERSION,
            skipped: reason,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEvidence {
    pub v: u8,
    pub skipped: SkipReason,
}

/// A successfully anchored piece of evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredEvidence {
    pub v: u8,
    /// Stable handle for the payment. Reuses `payment-identifier` when the
    /// extension is present; otherwise `keccak256(network || txHash || nonce)`.
    pub payment_id: String,
    pub pointer: DurablePointer,
    pub backend: StorageBackend,
    /// keccak256 of the **plaintext** body.
    ///
    /// Over the plaintext rather than the ciphertext on purpose: it lets the
    /// buyer prove that what was anchored decrypts to exactly what they were
    /// served, which is the one check that catches a seller anchoring something
    /// other than what it delivered.
    pub content_hash: String,
    pub cipher: String,
    pub key_alg: KeyAlg,
    pub mode: EvidenceMode,
    pub retention: Retention,
    /// EIP-712 signature by the facilitator over [`EvidenceReceipt`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    /// Whether the payee proved by signature that this anchor is theirs.
    ///
    /// Emitted so a seller learns on the FIRST anchor that its signature was
    /// rejected. Without it the only symptom was a later `409` on a collision
    /// that may never come -- a 201 that looks entirely successful while the
    /// anchor stays provisional forever.
    ///
    /// `default` on purpose: this header is already in the wild without the
    /// field, and a buyer must keep being able to parse evidence a seller
    /// emitted before it existed. Absent reads as "not proven", which is the
    /// safe direction -- it never upgrades an old claim to verified.
    #[serde(default)]
    pub verified: bool,
    /// Why finality was not granted, when it was not.
    ///
    /// Carried so a seller learns on the FIRST anchor instead of discovering it
    /// through a collision that may never come. `proof_missing` is the common
    /// one: without a `proofOfPayment` the gate reaches no conclusion, so the
    /// anchor is provisional -- it holds the slot and decrypts, it just does not
    /// claim an authorship nobody checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_verified_reason: Option<String>,
    /// Whether the claimant proved it controls the address it declared as payee.
    ///
    /// The rung below [`Self::verified`], reported separately so a seller can
    /// tell "my signature was accepted, the chain half just is not available on
    /// this network" from "my signature was rejected". Collapsing them would
    /// make a correct signature look like a failed one everywhere the gate
    /// cannot read a receipt -- which is every non-EVM family today.
    #[serde(default)]
    pub signed: bool,
}

/// The notarised claim the facilitator signs.
///
/// Verifiable offline by anyone holding the facilitator's public key -- no call
/// back to us required, which is the property the IETF receipt drafts identify
/// as missing from a bare `PAYMENT-RESPONSE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReceipt {
    pub payment_id: String,
    pub content_hash: String,
    pub pointer: DurablePointer,
    pub payer: MixedAddress,
    pub payee: MixedAddress,
    pub tx_hash: String,
    pub network: Network,
    pub mode: EvidenceMode,
    pub anchored_at: u64,
    /// Unix timestamp after which retrieval is no longer guaranteed. `0` means
    /// permanent.
    pub retention_until: u64,
}

/// Request body for `POST /dx402/anchor`.
///
/// Note what is absent: the plaintext, and in `direct` mode the CEK. The
/// resource server encrypts and uploads on its own; it tells the facilitator
/// only where the ciphertext went and what it hashes to.
///
/// No `Eq`: it now carries a `ProofOfPayment`, which holds token amounts that
/// do not implement it.
/// Deserialize a `Network` from either the v1 serde name or a CAIP-2 id.
fn deserialize_network_or_caip2<'de, D>(d: D) -> Result<Network, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let raw = String::deserialize(d)?;
    // CAIP-2 first: it is unambiguous (it always contains a colon) and the v1
    // names never do.
    if let Some(network) = Network::from_caip2(&raw) {
        return Ok(network);
    }
    serde_json::from_value(serde_json::Value::String(raw.clone()))
        .map_err(|_| D::Error::custom(format!("unknown network `{raw}`")))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorRequest {
    pub payment_id: String,
    /// Accepts both the v1 name (`base`) and the v2 CAIP-2 id (`eip155:8453`).
    ///
    /// Every other route on this facilitator takes either -- that is the whole
    /// point of the v2 format -- so a client that speaks CAIP-2 everywhere else
    /// got a bare `422` here with no field named. Worse than an unknown network:
    /// the caller has no reason to suspect the one field it spells the same way
    /// it does on `/verify` and `/settle`.
    #[serde(deserialize_with = "deserialize_network_or_caip2")]
    pub network: Network,
    pub tx_hash: String,
    pub payer: MixedAddress,
    pub payee: MixedAddress,
    /// Where the resource server put the ciphertext.
    ///
    /// Optional, and omitting it is the easy path: send `sealed` instead and the
    /// facilitator stores the blob and issues the pointer itself. A seller that
    /// already has durable storage can keep using it by supplying a pointer;
    /// one that does not should not have to stand up a bucket to get evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<DurablePointer>,
    /// The sealed envelope itself, base64. Present when the seller wants the
    /// facilitator to host it.
    ///
    /// This is ciphertext. Accepting it does not make the facilitator a
    /// custodian: in `direct` mode it cannot read what it stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<String>,
    pub backend: StorageBackend,
    pub content_hash: String,
    pub key_alg: KeyAlg,
    pub mode: EvidenceMode,
    pub retention: Retention,
    /// `escrowed` mode only: the CEK wrapped to the facilitator's key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapped_cek: Option<String>,
    /// The `ProofOfPayment` the facilitator handed back from `/settle`.
    ///
    /// Optional on the wire so existing callers keep working while the gate is
    /// in phase 1, but it is what makes an anchor believable: without it the
    /// facilitator is signing a receipt for a payment it never checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_of_payment: Option<crate::erc8004::ProofOfPayment>,
    /// EIP-712 signature by the payee over `(paymentId, contentHash, pointer)`.
    ///
    /// Proves the anchor comes from whoever got paid. Without it, an observer of
    /// the settlement could anchor garbage first and let anti-replay lock the
    /// real seller out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seller_signature: Option<String>,
    /// The x402r escrow authorization, when the payment was released from escrow.
    ///
    /// Required only on that rail, and only to answer one question: who funded
    /// the escrow. The ERC-20 `from` of a release is the operator's TokenStore,
    /// so without this the gate cannot tell an honest escrow anchor from
    /// evidence hung off a stranger's payment, and refuses both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escrow_release: Option<crate::dx402::gate::EscrowRelease>,
}

/// A single-use challenge for `escrowed` recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryChallenge {
    pub payment_id: String,
    pub payer: MixedAddress,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

/// Maximum lifetime of a recovery challenge, in seconds.
pub const CHALLENGE_TTL_SECS: u64 = 300;

/// Request body for `POST /dx402/recover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRequest {
    pub challenge: RecoveryChallenge,
    /// Payer signature over the EIP-712 encoding of `challenge`.
    pub signature: String,
}

/// Stable error codes, so callers can branch without parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dx402ErrorCode {
    Dx402Disabled,
    Dx402UnknownPayment,
    Dx402NotPayer,
    Dx402ChallengeExpired,
    Dx402ChallengeReplayed,
    Dx402DirectMode,
    Dx402EvidenceExpired,
    Dx402StoreUnavailable,
    /// The anchor gate refused it: no proof, a proof that did not check out, or
    /// evidence sealed to somebody who did not pay.
    ///
    /// Only reachable with `DX402_REQUIRE_PROOF=true`.
    Dx402ProofRejected,
    /// This payment already has evidence anchored. Anchoring is once-only.
    Dx402AlreadyAnchored,
    /// A `sellerSignature` was supplied and did not verify against the payee, so
    /// the anchor could not supersede the record already holding this payment.
    ///
    /// Split out of `Dx402AlreadyAnchored` because that code, while true, sends
    /// the integrator to audit the wrong thing. "You already anchored this" is a
    /// plausible story -- they go looking for a retry, a race, a repeated
    /// heartbeat -- and they find candidates, because those always exist. Nobody
    /// suspects the shape of a digest they do not know has shapes.
    ///
    /// Reported by KarmaKadabra, 2026-08-19, after isolating it with three
    /// anchors to one paymentId.
    Dx402SignatureNotVerified,
    /// The request named a storage backend this deployment cannot write to.
    ///
    /// `backend` used to be free text the caller supplied and nothing checked:
    /// a request could ask for `arweave`, which has no implementation at all,
    /// and the record plus the API response would both claim it while the bytes
    /// went wherever the configured store put them. `backend` is not part of
    /// the EIP-712 receipt, so it is not even a signed lie -- just a persisted
    /// one, which is worse for anybody reading the index to decide where their
    /// evidence lives.
    Dx402BackendUnavailable,
}

impl Dx402ErrorCode {
    /// Whether a caller should retry rather than record a negative result.
    ///
    /// This distinction is load-bearing. INC-2026-07-21 turned a transient RPC
    /// failure into a permanent wrong answer because a caller persisted a 503 as
    /// "not registered"; the same mistake here would record "no evidence exists"
    /// for a payment whose evidence is merely momentarily unreachable.
    pub fn is_retryable(self) -> bool {
        matches!(self, Dx402ErrorCode::Dx402StoreUnavailable)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Dx402ErrorCode::Dx402Disabled => "dx402_disabled",
            Dx402ErrorCode::Dx402UnknownPayment => "dx402_unknown_payment",
            Dx402ErrorCode::Dx402NotPayer => "dx402_not_payer",
            Dx402ErrorCode::Dx402ChallengeExpired => "dx402_challenge_expired",
            Dx402ErrorCode::Dx402ChallengeReplayed => "dx402_challenge_replayed",
            Dx402ErrorCode::Dx402DirectMode => "dx402_direct_mode",
            Dx402ErrorCode::Dx402EvidenceExpired => "dx402_evidence_expired",
            Dx402ErrorCode::Dx402StoreUnavailable => "dx402_store_unavailable",
            Dx402ErrorCode::Dx402ProofRejected => "dx402_proof_rejected",
            Dx402ErrorCode::Dx402AlreadyAnchored => "dx402_already_anchored",
            Dx402ErrorCode::Dx402SignatureNotVerified => "dx402_signature_not_verified",
            Dx402ErrorCode::Dx402BackendUnavailable => "dx402_backend_unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirements(extra: Option<serde_json::Value>) -> crate::types::PaymentRequirements {
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
    fn the_buyer_opts_in_by_paying_for_the_offer_that_declares_it() {
        // The whole opt-in in one round trip. Nothing new on the wire: the
        // seller lists the resource twice and the declaration rides in the
        // slot v1 already has for it.
        let cfg = DurableEvidenceConfig {
            retention: Retention::Year1,
            ..Default::default()
        };
        let mut durable = requirements(None);
        cfg.declare_on(&mut durable);
        let plain = requirements(None);

        assert_eq!(
            DurableEvidenceConfig::from_requirements(&durable),
            Some(cfg)
        );
        assert_eq!(DurableEvidenceConfig::from_requirements(&plain), None);
        assert!(DurableEvidenceConfig::offered_in(&[
            plain.clone(),
            durable.clone()
        ]));
        assert!(!DurableEvidenceConfig::offered_in(&[plain]));

        // The path mirrors v2's top-level `extensions` map, so the v1 -> v2
        // conversion is a rename and not a redesign.
        assert!(durable.extra.unwrap()["extensions"]["durable-evidence"].is_object());
    }

    #[test]
    fn declaring_keeps_the_eip712_domain_the_route_already_carries() {
        // `extra` is where the token's `name`/`version` live on most routes.
        // Replacing it instead of merging would make the durable offer
        // unpayable -- a signature over the wrong domain -- with no error.
        let mut req = requirements(Some(
            serde_json::json!({"name": "USD Coin", "version": "2"}),
        ));
        DurableEvidenceConfig::default().declare_on(&mut req);
        let extra = req.extra.unwrap();
        assert_eq!(extra["name"], "USD Coin");
        assert_eq!(extra["version"], "2");
        assert!(extra["extensions"]["durable-evidence"].is_object());
    }

    #[test]
    fn a_malformed_declaration_reads_as_not_declared() {
        // One seller's typo must not make its route unpayable. It reads as the
        // plain offer, which is what a buyer would get anyway.
        let req = requirements(Some(serde_json::json!({
            "extensions": {"durable-evidence": {"retention": "forever-and-ever"}}
        })));
        assert_eq!(DurableEvidenceConfig::from_requirements(&req), None);
        // ...but it still COUNTS as offered, so a plain payer is "declined",
        // never silently anchored under the route's terms.
        assert!(DurableEvidenceConfig::declared_on(&req));
        assert!(DurableEvidenceConfig::offered_in(std::slice::from_ref(
            &req
        )));
        let req = requirements(Some(serde_json::json!({"extensions": "not-a-map"})));
        assert_eq!(DurableEvidenceConfig::from_requirements(&req), None);
        assert!(!DurableEvidenceConfig::declared_on(&req));
    }

    #[test]
    fn a_skip_reason_from_the_future_does_not_drop_the_notice() {
        let ev: DurableEvidence =
            serde_json::from_str(r#"{"v":1,"skipped":"from_the_future"}"#).unwrap();
        match ev {
            DurableEvidence::Skipped(s) => assert_eq!(s.skipped, SkipReason::Unknown),
            other => panic!("expected a skip notice, got {other:?}"),
        }
    }

    #[test]
    fn the_top_level_declaration_round_trips_and_names_its_offers() {
        let info = DurableEvidenceInfo {
            config: DurableEvidenceConfig {
                retention: Retention::Year1,
                ..Default::default()
            },
            accept_indexes: vec![1],
        };
        let mut ext = std::collections::HashMap::new();
        info.declare(&mut ext);
        let raw = &ext["durable-evidence"];
        assert!(raw["schema"].is_object(), "schema published next to info");
        assert_eq!(raw["info"]["retention"], "1y");
        assert_eq!(raw["info"]["acceptIndexes"], serde_json::json!([1]));
        assert_eq!(DurableEvidenceInfo::from_extensions(&ext), Some(info));
        assert!(DurableEvidenceInfo::declared_in(&ext));

        // Present but unreadable: declared, not usable -- same rule as per-offer.
        let mut broken = std::collections::HashMap::new();
        broken.insert(
            "durable-evidence".to_string(),
            serde_json::json!({"info": {"retention": "forever-and-ever"}}),
        );
        assert_eq!(DurableEvidenceInfo::from_extensions(&broken), None);
        assert!(DurableEvidenceInfo::declared_in(&broken));
    }

    #[test]
    fn extension_key_matches_registry_convention() {
        // Kebab-case, like `offer-receipt` and `payment-identifier`. A mismatch
        // here silently drops the extension on any conforming implementation.
        assert_eq!(EXTENSION_KEY, "durable-evidence");
    }

    #[test]
    fn retention_default_is_bounded() {
        // Anchoring is publishing. If the default were Permanent, one careless
        // seller anchoring PII would have no remedy at all.
        assert_eq!(Retention::default(), Retention::Days90);
        assert_eq!(Retention::Days90.until(1_000), 1_000 + 90 * 86_400);
        assert_eq!(Retention::Permanent.until(1_000), 0);
    }

    #[test]
    fn default_mode_is_end_to_end() {
        assert_eq!(EvidenceMode::default(), EvidenceMode::Direct);
        assert_eq!(EvidenceMode::Direct.as_u8(), 0);
        assert_eq!(EvidenceMode::Escrowed.as_u8(), 1);
    }

    #[test]
    fn only_store_unavailable_is_retryable() {
        assert!(Dx402ErrorCode::Dx402StoreUnavailable.is_retryable());
        for code in [
            Dx402ErrorCode::Dx402Disabled,
            Dx402ErrorCode::Dx402UnknownPayment,
            Dx402ErrorCode::Dx402NotPayer,
            Dx402ErrorCode::Dx402ChallengeExpired,
            Dx402ErrorCode::Dx402ChallengeReplayed,
            Dx402ErrorCode::Dx402DirectMode,
            Dx402ErrorCode::Dx402EvidenceExpired,
        ] {
            assert!(!code.is_retryable(), "{code:?} must not be retryable");
        }
    }

    #[test]
    fn skip_notice_serializes_without_anchor_fields() {
        let json = serde_json::to_value(DurableEvidence::skipped(SkipReason::TooLarge)).unwrap();
        assert_eq!(json["skipped"], "too_large");
        assert_eq!(json["v"], 1);
        assert!(json.get("pointer").is_none());
    }

    #[test]
    fn every_skip_reason_has_a_stable_wire_name() {
        // A reader that does not know a variant fails the whole payload, so
        // these names are a compatibility surface, not a formatting choice.
        for (reason, wire) in [
            (SkipReason::TooLarge, "too_large"),
            (SkipReason::Busy, "busy"),
            (SkipReason::AnchorFailed, "anchor_failed"),
            (SkipReason::NoPayerKey, "no_payer_key"),
            (SkipReason::Disabled, "disabled"),
        ] {
            let json = serde_json::to_value(DurableEvidence::skipped(reason)).unwrap();
            assert_eq!(json["skipped"], wire);
        }
    }

    #[test]
    fn config_defaults_match_the_spec() {
        let c = DurableEvidenceConfig::default();
        assert_eq!(c.mode, EvidenceMode::Direct);
        assert_eq!(c.backend, StorageBackend::S3);
        assert_eq!(c.retention, Retention::Days90);
        assert_eq!(c.max_body_bytes, 33_554_432);
        assert_eq!(c.paid_by, PaidBy::Seller);
    }

    #[test]
    fn config_parses_spec_example() {
        let c: DurableEvidenceConfig = serde_json::from_str(
            r#"{"mode":"direct","backend":"s3","retention":"90d","maxBodyBytes":33554432,"paidBy":"seller"}"#,
        )
        .unwrap();
        assert_eq!(c, DurableEvidenceConfig::default());
    }

    #[test]
    fn key_alg_wire_names_are_stable() {
        assert_eq!(
            serde_json::to_value(KeyAlg::Secp256k1).unwrap(),
            "ECIES-secp256k1"
        );
        assert_eq!(
            serde_json::to_value(KeyAlg::X25519).unwrap(),
            "ECIES-X25519"
        );
    }
}

#[cfg(test)]
mod anchor_network_tests {
    use super::*;

    #[test]
    fn the_anchor_route_takes_a_caip2_network_like_every_other_route() {
        // A client that speaks v2 everywhere else got a bare 422 here, with no
        // field named -- worse than an unknown network, because it has no reason
        // to suspect the one field it spells exactly as it does on /verify.
        let body = |net: &str| {
            format!(
                r#"{{"paymentId":"0x{id}","network":"{net}","txHash":"0x{tx}",
                     "payer":"0x{a}","payee":"0x{b}","sealed":"AA==","backend":"s3",
                     "contentHash":"0x{id}","keyAlg":"ECIES-X25519","mode":"direct",
                     "retention":"90d"}}"#,
                id = "ab".repeat(32),
                tx = "cd".repeat(32),
                a = "11".repeat(20),
                b = "22".repeat(20),
            )
        };

        let v1: AnchorRequest = serde_json::from_str(&body("base")).expect("v1 name");
        let v2: AnchorRequest = serde_json::from_str(&body("eip155:8453")).expect("CAIP-2 id");
        assert_eq!(v1.network, v2.network);
        assert_eq!(v1.network, Network::Base);

        // And an unknown one still fails, by name.
        let err = serde_json::from_str::<AnchorRequest>(&body("eip155:99999999"))
            .expect_err("unknown network must not deserialize");
        assert!(err.to_string().contains("eip155:99999999"), "{err}");
    }
}
