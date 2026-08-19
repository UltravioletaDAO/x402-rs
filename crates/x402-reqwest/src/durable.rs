//! DX402 `durable-evidence`: the buyer side.
//!
//! After a paid request returns, the response carries an `X-Durable-Evidence`
//! header pointing at a sealed copy of the body. This module turns that pointer
//! back into plaintext -- now, or months later from nothing but the header and
//! the wallet that paid.
//!
//! ```text
//! 200 OK + X-Durable-Evidence  ──►  pointer
//!                                     │ fetch ciphertext
//!                                     │ decrypt with the payer's own key
//!                                     │ check contentHash
//!                                     ▼
//!                                   the original body
//! ```
//!
//! # Why the buyer needs no permission
//!
//! In `direct` mode the ciphertext was sealed to the public key belonging to the
//! wallet that paid. Retrieval is not an authorization decision anyone makes --
//! it is arithmetic. No account, no token, no ACL to misconfigure, and no
//! request to us that could be refused.
//!
//! # Verify the hash
//!
//! [`recover`] checks `contentHash` against the decrypted bytes. That check is
//! the one that catches a seller who anchored something other than what it
//! served, so it is not optional and there is no flag to skip it.

use thiserror::Error;
use x402_rs::dx402::envelope::{open, PayerSecretKey, SealedEnvelope};
use x402_rs::dx402::types::{AnchoredEvidence, DurableEvidence};

/// Header emitted alongside `X-Payment-Response`.
pub const EVIDENCE_HEADER: &str = x402_rs::dx402::types::EVIDENCE_HEADER;

#[derive(Debug, Error)]
pub enum RecoverError {
    #[error("no {EVIDENCE_HEADER} header on this response")]
    NoHeader,
    #[error("the {EVIDENCE_HEADER} header is malformed")]
    MalformedHeader,
    #[error("no evidence was anchored for this payment: {0}")]
    Skipped(String),
    #[error("could not fetch the sealed evidence: {0}")]
    Fetch(String),
    #[error("the sealed blob is malformed: {0}")]
    MalformedBlob(String),
    #[error("decryption failed -- wrong key, or the blob belongs to another payment")]
    Decrypt,
    /// The anchored bytes are not the bytes that were delivered.
    ///
    /// This is the interesting failure: it means the seller anchored something
    /// other than what it served, which is precisely the fraud `contentHash`
    /// exists to expose. Treat it as evidence, not as a transport glitch.
    #[error("content hash mismatch: anchored {anchored}, decrypted {actual}")]
    ContentHashMismatch { anchored: String, actual: String },
}

/// Parse an `X-Durable-Evidence` header value.
pub fn parse_header(value: &str) -> Result<DurableEvidence, RecoverError> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .map_err(|_| RecoverError::MalformedHeader)?;
    serde_json::from_slice(&raw).map_err(|_| RecoverError::MalformedHeader)
}

/// Pull the anchored record out of a response's headers.
pub fn evidence_of(headers: &http::HeaderMap) -> Result<AnchoredEvidence, RecoverError> {
    let value = headers
        .get(EVIDENCE_HEADER)
        .ok_or(RecoverError::NoHeader)?
        .to_str()
        .map_err(|_| RecoverError::MalformedHeader)?;

    match parse_header(value)? {
        DurableEvidence::Anchored(a) => Ok(*a),
        DurableEvidence::Skipped(s) => Err(RecoverError::Skipped(
            serde_json::to_value(s.skipped)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".into()),
        )),
    }
}

/// Fetch, decrypt and verify the evidence described by `evidence`.
///
/// `payment_id` must be the same value the seller used; it is the AEAD
/// associated data, so a mismatch shows up as a decryption failure rather than
/// as wrong plaintext.
pub async fn recover(
    client: &reqwest::Client,
    evidence: &AnchoredEvidence,
    secret: &PayerSecretKey,
) -> Result<Vec<u8>, RecoverError> {
    let url = dereference(evidence.pointer.as_str());

    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|e| RecoverError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| RecoverError::Fetch(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| RecoverError::Fetch(e.to_string()))?;

    let sealed = SealedEnvelope::from_bytes(&bytes)
        .map_err(|e| RecoverError::MalformedBlob(e.to_string()))?;

    let plaintext =
        open(&sealed, secret, evidence.payment_id.as_bytes()).map_err(|_| RecoverError::Decrypt)?;

    let actual = x402_rs::dx402::content_hash(&plaintext);
    if !actual.eq_ignore_ascii_case(&evidence.content_hash) {
        return Err(RecoverError::ContentHashMismatch {
            anchored: evidence.content_hash.clone(),
            actual,
        });
    }

    Ok(plaintext)
}

/// Turn a DX402 pointer into a URL that can be fetched.
///
/// `s3+https://…` is our own scheme tag over an ordinary HTTPS URL; `ipfs://`
/// goes through a public gateway. Anything else is passed through untouched so a
/// caller with their own resolver is not blocked by this function.
fn dereference(pointer: &str) -> String {
    if let Some(rest) = pointer.strip_prefix("s3+") {
        return rest.to_string();
    }
    if let Some(cid) = pointer.strip_prefix("ipfs://") {
        return format!("https://ipfs.io/ipfs/{cid}");
    }
    if let Some(id) = pointer.strip_prefix("ar://") {
        return format!("https://arweave.net/{id}");
    }
    pointer.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use x402_rs::dx402::envelope::{seal, PayerPublicKey};
    use x402_rs::dx402::types::{
        DurablePointer, EvidenceMode, KeyAlg, Retention, SkipReason, StorageBackend, DX402_VERSION,
    };

    #[test]
    fn evidence_emitted_before_verified_existed_still_parses() {
        // `X-Durable-Evidence` headers are already in the wild without this
        // field. A buyer on a newer SDK must keep being able to open evidence a
        // seller anchored before it existed, and an absent field must read as
        // "not proven" -- never as verified.
        let old = serde_json::json!({
            "v": 1,
            "paymentId": "0xabc",
            "pointer": "s3+https://example.test/blob/0xabc",
            "backend": "s3",
            "contentHash": "0xdef",
            "cipher": "AES-256-GCM",
            "keyAlg": "ECIES-secp256k1",
            "mode": "direct",
            "retention": "90d"
        });
        let parsed: AnchoredEvidence =
            serde_json::from_value(old).expect("old evidence must still parse");
        assert!(!parsed.verified);
    }

    fn anchored(content_hash: String) -> AnchoredEvidence {
        AnchoredEvidence {
            v: DX402_VERSION,
            payment_id: format!("0x{}", "11".repeat(32)),
            pointer: DurablePointer("mem://x".into()),
            backend: StorageBackend::S3,
            content_hash,
            cipher: "AES-256-GCM".into(),
            key_alg: KeyAlg::Secp256k1,
            mode: EvidenceMode::Direct,
            retention: Retention::Days90,
            receipt: None,
            verified: false,
            signed: false,
            not_verified_reason: None,
        }
    }

    #[test]
    fn pointers_dereference_to_fetchable_urls() {
        assert_eq!(
            dereference("s3+https://evidence.ultravioletadao.xyz/e/a.dx402"),
            "https://evidence.ultravioletadao.xyz/e/a.dx402"
        );
        assert_eq!(
            dereference("ipfs://bafy123"),
            "https://ipfs.io/ipfs/bafy123"
        );
        assert_eq!(dereference("ar://tx123"), "https://arweave.net/tx123");
        // Unknown schemes pass through rather than being mangled.
        assert_eq!(dereference("https://x.example/y"), "https://x.example/y");
    }

    #[test]
    fn a_skip_notice_is_reported_as_a_skip_not_an_error() {
        // "The seller chose not to anchor" and "the evidence is broken" are
        // different situations and a buyer must be able to tell them apart.
        let evidence = DurableEvidence::skipped(SkipReason::TooLarge);
        let json = serde_json::to_vec(&evidence).unwrap();
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);

        let mut headers = http::HeaderMap::new();
        headers.insert(EVIDENCE_HEADER, encoded.parse().unwrap());

        match evidence_of(&headers) {
            Err(RecoverError::Skipped(reason)) => assert_eq!(reason, "too_large"),
            other => panic!("expected a skip, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_header_is_distinguished_from_a_malformed_one() {
        assert!(matches!(
            evidence_of(&http::HeaderMap::new()),
            Err(RecoverError::NoHeader)
        ));

        let mut headers = http::HeaderMap::new();
        headers.insert(EVIDENCE_HEADER, "!!!not-base64!!!".parse().unwrap());
        assert!(matches!(
            evidence_of(&headers),
            Err(RecoverError::MalformedHeader)
        ));
    }

    #[test]
    fn decryption_and_the_hash_check_agree_on_an_honest_anchor() {
        // Fixed key rather than a random one: a crypto test that passes only
        // sometimes is worse than no test.
        let sk = k256::SecretKey::from_slice(&[0x42u8; 32]).unwrap();
        let payer = PayerPublicKey::Secp256k1(Box::new(sk.public_key()));
        let body = b"what the seller actually delivered";
        let pid = format!("0x{}", "11".repeat(32));

        let sealed = seal(body, &payer, pid.as_bytes()).unwrap();
        let evidence = anchored(x402_rs::dx402::content_hash(body));

        let parsed = SealedEnvelope::from_bytes(&sealed.to_bytes()).unwrap();
        let plaintext = open(
            &parsed,
            &PayerSecretKey::Secp256k1(Box::new(sk)),
            pid.as_bytes(),
        )
        .unwrap();

        assert_eq!(plaintext, body);
        assert_eq!(
            x402_rs::dx402::content_hash(&plaintext),
            evidence.content_hash
        );
    }

    #[test]
    fn a_dishonest_anchor_fails_the_hash_check() {
        // The seller anchors one thing and claims the hash of another. This is
        // the fraud contentHash exists to catch.
        let delivered = b"what the buyer was served";
        let anchored_instead = b"something else entirely";

        let claimed = anchored(x402_rs::dx402::content_hash(delivered));
        let actual = x402_rs::dx402::content_hash(anchored_instead);
        assert_ne!(actual, claimed.content_hash);
    }
}
