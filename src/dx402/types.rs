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
    pub max_body_bytes: usize,
    /// Who bears the persistence cost. Informational at this layer; billing is
    /// the resource server's business.
    pub paid_by: PaidBy,
}

impl Default for DurableEvidenceConfig {
    fn default() -> Self {
        Self {
            mode: EvidenceMode::default(),
            backend: StorageBackend::default(),
            retention: Retention::default(),
            max_body_bytes: 1_048_576,
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
    /// The payer's public key could not be recovered for this network family,
    /// so there is nobody to encrypt to.
    NoPayerKey,
    /// The extension is switched off on this deployment.
    Disabled,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorRequest {
    pub payment_id: String,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn config_defaults_match_the_spec() {
        let c = DurableEvidenceConfig::default();
        assert_eq!(c.mode, EvidenceMode::Direct);
        assert_eq!(c.backend, StorageBackend::S3);
        assert_eq!(c.retention, Retention::Days90);
        assert_eq!(c.max_body_bytes, 1_048_576);
        assert_eq!(c.paid_by, PaidBy::Seller);
    }

    #[test]
    fn config_parses_spec_example() {
        let c: DurableEvidenceConfig = serde_json::from_str(
            r#"{"mode":"direct","backend":"s3","retention":"90d","maxBodyBytes":1048576,"paidBy":"seller"}"#,
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
