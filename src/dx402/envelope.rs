//! The DX402 envelope: AES-256-GCM over the body, ECIES over the content key.
//!
//! Spec: `docs/plans/dx402/02-SPEC-v0.1.md` section 5.
//!
//! ```text
//! CEK        := random 32 bytes
//! ciphertext := AES-256-GCM(CEK, body, aad = paymentId)
//! shared     := ECDH(ephemeral_secret, payer_public_key)
//! wrapKey    := HKDF-SHA256(ikm = shared, salt = paymentId, info = "DX402-v1-wrap")
//! wrappedCEK := AES-256-GCM(wrapKey, CEK)
//! ```
//!
//! Two curves, because our payers live on two. secp256k1 for EVM and
//! secp256k1-keyed XRPL; X25519 (reached from ed25519 by the birational map) for
//! Solana, NEAR, Stellar, Algorand, Sui and ed25519-keyed XRPL.
//!
//! The `aad = paymentId` binding matters: it means a ciphertext lifted from one
//! payment cannot be passed off as the evidence for another, because GCM
//! authentication fails when the associated data does not match.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use hkdf::Hkdf;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use rand::RngCore;
use sha2::Sha256;
use thiserror::Error;

use super::types::KeyAlg;

/// HKDF `info` string. Domain-separates this derivation from any other use of
/// the same ECDH output.
const HKDF_INFO: &[u8] = b"DX402-v1-wrap";

/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Content encryption key length in bytes.
const CEK_LEN: usize = 32;

#[derive(Debug, Error)]
pub enum EnvelopeError {
    #[error("invalid payer public key: {0}")]
    InvalidPayerKey(String),
    #[error("invalid ephemeral public key: {0}")]
    InvalidEphemeralKey(String),
    #[error("AEAD failure (wrong key, tampered ciphertext, or mismatched paymentId)")]
    Aead,
    #[error("malformed envelope: {0}")]
    Malformed(String),
    /// The X25519 exchange produced an all-zero shared secret, which means one
    /// side contributed a small-order point.
    ///
    /// Left unchecked this is a real break, not a curiosity: `ed25519-dalek`
    /// accepts non-canonical and small-order encodings in
    /// `VerifyingKey::from_bytes`, so anyone able to influence the recorded
    /// payer key could force the shared secret to a constant and derive the CEK
    /// wrapping key themselves. RFC 7748 section 6.1 requires rejecting it.
    #[error("degenerate ECDH result (small-order public key)")]
    DegenerateSharedSecret,
    #[error("curve mismatch: envelope is {envelope}, key is {key}")]
    CurveMismatch { envelope: KeyAlg, key: KeyAlg },
}

/// A payer's public key, in whichever curve their chain uses.
///
/// Obtained without asking the payer for anything -- see [`super::pubkey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayerPublicKey {
    /// SEC1-compressed secp256k1 point (33 bytes).
    Secp256k1(Box<k256::PublicKey>),
    /// Montgomery-form X25519 point (32 bytes), mapped from the payer's ed25519
    /// verifying key.
    X25519(MontgomeryPoint),
}

impl PayerPublicKey {
    pub fn key_alg(&self) -> KeyAlg {
        match self {
            PayerPublicKey::Secp256k1(_) => KeyAlg::Secp256k1,
            PayerPublicKey::X25519(_) => KeyAlg::X25519,
        }
    }
}

/// The complete anchored artifact.
///
/// This is what gets written to the evidence store. It is self-describing: a
/// holder of the payer private key needs nothing else to decrypt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedEnvelope {
    /// Who can open this. At least one; the first is conventionally the payer.
    pub recipients: Vec<Recipient>,
    pub body_nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

/// One party who can decrypt the body.
///
/// The **body ciphertext is not duplicated** -- every recipient unwraps the same
/// CEK. Adding a party costs about sixty bytes, not a second copy of the
/// payload, which is what makes bidirectional evidence practically free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub role: RecipientRole,
    pub key_alg: KeyAlg,
    /// Ephemeral public key: 33 bytes SEC1-compressed, or 32 bytes Montgomery.
    pub ephemeral_public: Vec<u8>,
    pub cek_nonce: [u8; NONCE_LEN],
    pub wrapped_cek: Vec<u8>,
}

/// Why a party holds a key to this evidence.
///
/// Recorded because it is not decoration: a buyer has to be able to see that the
/// seller -- or a designated auditor -- can also read what they bought.
/// Discovering that afterwards would destroy the privacy property DX402 sells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientRole {
    /// The address that paid. Always present.
    Payer,
    /// The resource server, so it can answer a false "that is not what you sent".
    Seller,
    /// A designated third party. Opt-in, never implicit.
    Auditor,
}

impl RecipientRole {
    fn as_u8(self) -> u8 {
        match self {
            RecipientRole::Payer => 0,
            RecipientRole::Seller => 1,
            RecipientRole::Auditor => 2,
        }
    }

    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(RecipientRole::Payer),
            1 => Some(RecipientRole::Seller),
            2 => Some(RecipientRole::Auditor),
            _ => None,
        }
    }
}

/// Magic prefix so a stray blob is identifiable, and a version byte so the
/// format can change without ambiguity.
const MAGIC: &[u8; 5] = b"DX402";
/// Single recipient. Still EMITTED for the one-recipient case so every reader
/// already in the wild keeps working.
const FORMAT_VERSION_V1: u8 = 1;
/// Multiple recipients. Only emitted when there actually are several, so a v2
/// blob is a positive signal that somebody besides the payer can read it.
const FORMAT_VERSION_V2: u8 = 2;

impl SealedEnvelope {
    /// Serialize to the byte layout that is actually stored.
    ///
    /// v1 (one recipient, the payer):
    /// `MAGIC | 1 | alg | eph_len | eph | cek_nonce | wrapped_len | wrapped | body_nonce | ciphertext`
    ///
    /// v2 (several):
    /// `MAGIC | 2 | count | count×(role | alg | eph_len | eph | cek_nonce | wrapped_len | wrapped) | body_nonce | ciphertext`
    ///
    /// A single-payer envelope is still emitted as **v1**, byte-for-byte as
    /// before. Every reader already deployed keeps working, and a v2 blob
    /// becomes a positive signal that somebody besides the payer can open it,
    /// rather than a version bump nobody can interpret.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.ciphertext.len());
        out.extend_from_slice(MAGIC);

        let single_payer =
            self.recipients.len() == 1 && self.recipients[0].role == RecipientRole::Payer;

        if single_payer {
            out.push(FORMAT_VERSION_V1);
            write_recipient_body(&mut out, &self.recipients[0]);
        } else {
            out.push(FORMAT_VERSION_V2);
            out.push(self.recipients.len() as u8);
            for r in &self.recipients {
                out.push(r.role.as_u8());
                write_recipient_body(&mut out, r);
            }
        }

        out.extend_from_slice(&self.body_nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parse the stored layout back. Accepts v1 and v2.
    pub fn from_bytes(raw: &[u8]) -> Result<Self, EnvelopeError> {
        let mut cur = 0usize;

        let mut take = |n: usize, what: &str| -> Result<&[u8], EnvelopeError> {
            if raw.len() < cur + n {
                return Err(EnvelopeError::Malformed(format!("truncated at {what}")));
            }
            let out = &raw[cur..cur + n];
            cur += n;
            Ok(out)
        };

        if take(5, "magic")? != MAGIC {
            return Err(EnvelopeError::Malformed("bad magic".into()));
        }

        let version = take(1, "version")?[0];
        let count = match version {
            FORMAT_VERSION_V1 => 1usize,
            FORMAT_VERSION_V2 => take(1, "recipient count")?[0] as usize,
            other => {
                return Err(EnvelopeError::Malformed(format!(
                    "unsupported format version {other}"
                )))
            }
        };
        if count == 0 {
            // An envelope nobody can open is not evidence.
            return Err(EnvelopeError::Malformed("no recipients".into()));
        }

        let mut recipients = Vec::with_capacity(count);
        for _ in 0..count {
            let role = if version == FORMAT_VERSION_V1 {
                RecipientRole::Payer
            } else {
                let b = take(1, "role")?[0];
                RecipientRole::from_u8(b)
                    .ok_or_else(|| EnvelopeError::Malformed(format!("unknown role {b}")))?
            };

            let alg_byte = take(1, "alg")?[0];
            let key_alg = match alg_byte {
                1 => KeyAlg::Secp256k1,
                2 => KeyAlg::X25519,
                other => {
                    return Err(EnvelopeError::Malformed(format!(
                        "unknown key algorithm {other}"
                    )))
                }
            };

            let eph_len = take(1, "eph_len")?[0] as usize;
            let ephemeral_public = take(eph_len, "ephemeral key")?.to_vec();

            let mut cek_nonce = [0u8; NONCE_LEN];
            cek_nonce.copy_from_slice(take(NONCE_LEN, "cek nonce")?);

            let len_bytes = take(2, "wrapped len")?;
            let wrapped_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
            let wrapped_cek = take(wrapped_len, "wrapped cek")?.to_vec();

            recipients.push(Recipient {
                role,
                key_alg,
                ephemeral_public,
                cek_nonce,
                wrapped_cek,
            });
        }

        let mut body_nonce = [0u8; NONCE_LEN];
        body_nonce.copy_from_slice(take(NONCE_LEN, "body nonce")?);
        let ciphertext = raw[cur..].to_vec();

        Ok(SealedEnvelope {
            recipients,
            body_nonce,
            ciphertext,
        })
    }

    /// The roles that can open this envelope.
    ///
    /// Callers surface this so a buyer can see who else holds a key. It belongs
    /// in the signed receipt, not only here.
    pub fn roles(&self) -> Vec<RecipientRole> {
        self.recipients.iter().map(|r| r.role).collect()
    }
}

fn write_recipient_body(out: &mut Vec<u8>, r: &Recipient) {
    out.push(match r.key_alg {
        KeyAlg::Secp256k1 => 1,
        KeyAlg::X25519 => 2,
    });
    out.push(r.ephemeral_public.len() as u8);
    out.extend_from_slice(&r.ephemeral_public);
    out.extend_from_slice(&r.cek_nonce);
    out.extend_from_slice(&(r.wrapped_cek.len() as u16).to_be_bytes());
    out.extend_from_slice(&r.wrapped_cek);
}

/// Reject an all-zero X25519 output, per RFC 7748 section 6.1.
///
/// A small-order public key drives every exchange to the same constant, which
/// would let anyone who supplied that key reconstruct the wrapping key. Checked
/// in constant time so the check itself does not leak.
fn reject_degenerate(shared: &MontgomeryPoint) -> Result<(), EnvelopeError> {
    use subtle::ConstantTimeEq;
    if bool::from(shared.to_bytes().ct_eq(&[0u8; 32])) {
        return Err(EnvelopeError::DegenerateSharedSecret);
    }
    Ok(())
}

/// Derive the CEK wrapping key from an ECDH shared secret.
fn wrap_key(shared: &[u8], payment_id: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(payment_id), shared);
    let mut okm = [0u8; 32];
    // Only fails for absurd output lengths; 32 bytes is always valid.
    hk.expand(HKDF_INFO, &mut okm)
        .expect("HKDF expand of 32 bytes cannot fail");
    okm
}

fn aead_seal(key: &[u8; 32], nonce: &[u8; NONCE_LEN], msg: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg, aad })
        .expect("AES-256-GCM encryption cannot fail for in-memory input")
}

fn aead_open(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    ct: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| EnvelopeError::Aead)
}

/// Wrap `cek` to one public key, producing a [`Recipient`].
fn wrap_for(
    role: RecipientRole,
    key: &PayerPublicKey,
    cek: &[u8; CEK_LEN],
    payment_id: &[u8],
    rng: &mut impl RngCore,
) -> Result<Recipient, EnvelopeError> {
    let mut cek_nonce = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut cek_nonce);

    let (ephemeral_public, shared) = match key {
        PayerPublicKey::Secp256k1(pk) => {
            let eph = k256::SecretKey::random(&mut rand::rngs::OsRng);
            let shared = k256::ecdh::diffie_hellman(eph.to_nonzero_scalar(), pk.as_affine());
            let eph_pub = eph.public_key().to_encoded_point(true).as_bytes().to_vec();
            (eph_pub, shared.raw_secret_bytes().to_vec())
        }
        PayerPublicKey::X25519(point) => {
            let mut eph_secret = [0u8; 32];
            rng.fill_bytes(&mut eph_secret);
            let eph_pub = MontgomeryPoint::mul_base_clamped(eph_secret);
            let shared = point.mul_clamped(eph_secret);
            reject_degenerate(&shared)?;
            (eph_pub.to_bytes().to_vec(), shared.to_bytes().to_vec())
        }
    };

    let wk = wrap_key(&shared, payment_id);
    Ok(Recipient {
        role,
        key_alg: key.key_alg(),
        ephemeral_public,
        cek_nonce,
        wrapped_cek: aead_seal(&wk, &cek_nonce, cek, payment_id),
    })
}

/// Encrypt `body` so that only the holder of the payer's private key can read it.
///
/// `payment_id` is bound in as AEAD associated data, which is what stops a
/// ciphertext from being replayed as the evidence for a different payment.
pub fn seal(
    body: &[u8],
    payer: &PayerPublicKey,
    payment_id: &[u8],
) -> Result<SealedEnvelope, EnvelopeError> {
    seal_to(body, &[(RecipientRole::Payer, payer.clone())], payment_id)
}

/// Encrypt `body` so that every listed recipient can read it, and nobody else.
///
/// The body is encrypted **once**; only the content key is wrapped per
/// recipient. Adding the seller so it can defend itself against a false
/// "that is not what you sent" costs about sixty bytes, not a second copy.
///
/// Order is preserved and carried into the stored bytes, so a reader can report
/// exactly who holds a key -- which belongs in the signed receipt, because a
/// buyer discovering after the fact that somebody else could read their purchase
/// would destroy the property this whole design sells.
pub fn seal_to(
    body: &[u8],
    recipients: &[(RecipientRole, PayerPublicKey)],
    payment_id: &[u8],
) -> Result<SealedEnvelope, EnvelopeError> {
    if recipients.is_empty() {
        return Err(EnvelopeError::Malformed(
            "an envelope with no recipients could never be opened".into(),
        ));
    }

    let mut rng = rand::rngs::OsRng;
    let mut cek = [0u8; CEK_LEN];
    rng.fill_bytes(&mut cek);
    let mut body_nonce = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut body_nonce);

    let ciphertext = aead_seal(&cek, &body_nonce, body, payment_id);

    let wrapped = recipients
        .iter()
        .map(|(role, key)| wrap_for(*role, key, &cek, payment_id, &mut rng))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SealedEnvelope {
        recipients: wrapped,
        body_nonce,
        ciphertext,
    })
}

/// The payer's private key, used only on the buyer's side.
///
/// The facilitator never holds one of these in `direct` mode. It exists here so
/// the round trip can be tested, and so `x402-reqwest` has a type to work with.
pub enum PayerSecretKey {
    Secp256k1(Box<k256::SecretKey>),
    /// Raw ed25519 seed (32 bytes). Converted to an X25519 scalar internally.
    Ed25519Seed([u8; 32]),
}

/// Map an ed25519 seed to its X25519 secret scalar bytes.
///
/// This is the standard conversion: SHA-512 the seed and take the low half. The
/// clamping that X25519 requires is applied by `mul_clamped`.
fn ed25519_seed_to_x25519(seed: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha512};
    let h = Sha512::digest(seed);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h[..32]);
    out
}

/// Recover the plaintext body from a sealed envelope.
///
/// Tries every recipient slot, because a holder does not necessarily know which
/// one is theirs -- and in a multi-recipient envelope the payer is not always
/// first. Only a slot on the same curve is attempted; the rest are skipped
/// rather than reported, since "that slot was not for me" is not an error.
pub fn open(
    envelope: &SealedEnvelope,
    secret: &PayerSecretKey,
    payment_id: &[u8],
) -> Result<Vec<u8>, EnvelopeError> {
    let want = match secret {
        PayerSecretKey::Secp256k1(_) => KeyAlg::Secp256k1,
        PayerSecretKey::Ed25519Seed(_) => KeyAlg::X25519,
    };

    let mut saw_any_matching_curve = false;

    for recipient in &envelope.recipients {
        if recipient.key_alg != want {
            continue;
        }
        saw_any_matching_curve = true;

        let shared = match (secret, recipient.key_alg) {
            (PayerSecretKey::Secp256k1(sk), KeyAlg::Secp256k1) => {
                let Ok(eph) = k256::PublicKey::from_sec1_bytes(&recipient.ephemeral_public) else {
                    continue;
                };
                k256::ecdh::diffie_hellman(sk.to_nonzero_scalar(), eph.as_affine())
                    .raw_secret_bytes()
                    .to_vec()
            }
            (PayerSecretKey::Ed25519Seed(seed), KeyAlg::X25519) => {
                if recipient.ephemeral_public.len() != 32 {
                    continue;
                }
                let mut eph = [0u8; 32];
                eph.copy_from_slice(&recipient.ephemeral_public);
                let scalar = ed25519_seed_to_x25519(seed);
                let shared = MontgomeryPoint(eph).mul_clamped(scalar);
                reject_degenerate(&shared)?;
                shared.to_bytes().to_vec()
            }
            _ => continue,
        };

        let wk = wrap_key(&shared, payment_id);
        let Ok(cek_vec) = aead_open(
            &wk,
            &recipient.cek_nonce,
            &recipient.wrapped_cek,
            payment_id,
        ) else {
            // Not our slot. Keep looking.
            continue;
        };
        if cek_vec.len() != CEK_LEN {
            return Err(EnvelopeError::Malformed(format!(
                "unwrapped CEK is {} bytes, expected {CEK_LEN}",
                cek_vec.len()
            )));
        }
        let mut cek = [0u8; CEK_LEN];
        cek.copy_from_slice(&cek_vec);

        return aead_open(&cek, &envelope.body_nonce, &envelope.ciphertext, payment_id);
    }

    if saw_any_matching_curve {
        // The right curve was present but no slot opened: wrong key, or the
        // blob belongs to a different payment.
        Err(EnvelopeError::Aead)
    } else {
        Err(EnvelopeError::CurveMismatch {
            envelope: envelope
                .recipients
                .first()
                .map(|r| r.key_alg)
                .unwrap_or(want),
            key: want,
        })
    }
}

/// Map an ed25519 verifying key to the Montgomery point used for X25519 ECDH.
pub fn ed25519_to_x25519_public(verifying_key: &ed25519_dalek::VerifyingKey) -> PayerPublicKey {
    PayerPublicKey::X25519(verifying_key.to_montgomery())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: &[u8] = b"0xdeadbeef";

    fn secp_pair() -> (PayerPublicKey, PayerSecretKey) {
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        (
            PayerPublicKey::Secp256k1(Box::new(sk.public_key())),
            PayerSecretKey::Secp256k1(Box::new(sk)),
        )
    }

    fn ed_pair() -> (PayerPublicKey, PayerSecretKey) {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        (
            ed25519_to_x25519_public(&sk.verifying_key()),
            PayerSecretKey::Ed25519Seed(seed),
        )
    }

    #[test]
    fn secp256k1_round_trip() {
        let (pk, sk) = secp_pair();
        let body = b"the response body that must survive the session";
        let env = seal(body, &pk, PID).unwrap();
        assert_eq!(env.recipients[0].key_alg, KeyAlg::Secp256k1);
        assert_eq!(env.recipients[0].ephemeral_public.len(), 33);
        assert_eq!(open(&env, &sk, PID).unwrap(), body);
    }

    #[test]
    fn x25519_round_trip() {
        let (pk, sk) = ed_pair();
        let body = b"solana payer, ed25519 key, no signature needed";
        let env = seal(body, &pk, PID).unwrap();
        assert_eq!(env.recipients[0].key_alg, KeyAlg::X25519);
        assert_eq!(env.recipients[0].ephemeral_public.len(), 32);
        assert_eq!(open(&env, &sk, PID).unwrap(), body);
    }

    #[test]
    fn ciphertext_does_not_contain_plaintext() {
        // The blob is what lands in durable storage. If the plaintext leaked into
        // it, every property DX402 claims would be false.
        let (pk, _) = secp_pair();
        let body = b"SENSITIVE-MARKER-STRING";
        let env = seal(body, &pk, PID).unwrap();
        let blob = env.to_bytes();
        assert!(
            blob.windows(body.len()).all(|w| w != body),
            "plaintext found in sealed blob"
        );
    }

    #[test]
    fn a_different_payer_cannot_open_it() {
        let (pk, _) = secp_pair();
        let (_, other_sk) = secp_pair();
        let env = seal(b"private", &pk, PID).unwrap();
        assert!(matches!(
            open(&env, &other_sk, PID),
            Err(EnvelopeError::Aead)
        ));
    }

    #[test]
    fn evidence_is_bound_to_its_payment_id() {
        // Lifting a ciphertext from one payment and presenting it as the evidence
        // for another must fail, or the anchor proves nothing about which
        // transaction it belongs to.
        let (pk, sk) = secp_pair();
        let env = seal(b"body", &pk, PID).unwrap();
        assert!(matches!(
            open(&env, &sk, b"0xa-different-payment"),
            Err(EnvelopeError::Aead)
        ));
    }

    #[test]
    fn tampering_with_the_ciphertext_is_detected() {
        let (pk, sk) = secp_pair();
        let mut env = seal(b"body that matters", &pk, PID).unwrap();
        env.ciphertext[0] ^= 0xff;
        assert!(matches!(open(&env, &sk, PID), Err(EnvelopeError::Aead)));
    }

    #[test]
    fn serialization_round_trips() {
        let (pk, sk) = secp_pair();
        let body = b"anchored bytes";
        let env = seal(body, &pk, PID).unwrap();
        let parsed = SealedEnvelope::from_bytes(&env.to_bytes()).unwrap();
        assert_eq!(parsed, env);
        assert_eq!(open(&parsed, &sk, PID).unwrap(), body);
    }

    #[test]
    fn curve_mismatch_is_reported_not_silently_wrong() {
        let (secp_pk, _) = secp_pair();
        let (_, ed_sk) = ed_pair();
        let env = seal(b"body", &secp_pk, PID).unwrap();
        assert!(matches!(
            open(&env, &ed_sk, PID),
            Err(EnvelopeError::CurveMismatch { .. })
        ));
    }

    #[test]
    fn malformed_blobs_are_rejected() {
        assert!(SealedEnvelope::from_bytes(b"").is_err());
        assert!(SealedEnvelope::from_bytes(b"NOTDX402...").is_err());
        let (pk, _) = secp_pair();
        let good = seal(b"x", &pk, PID).unwrap().to_bytes();
        // Truncation at every prefix must error, never panic.
        for n in 0..good.len() {
            let _ = SealedEnvelope::from_bytes(&good[..n]);
        }
    }

    #[test]
    fn two_seals_of_the_same_body_differ() {
        // Fresh CEK and fresh nonces every time. Identical blobs would let an
        // observer of the store learn that two buyers received the same answer.
        let (pk, _) = secp_pair();
        let a = seal(b"same body", &pk, PID).unwrap();
        let b = seal(b"same body", &pk, PID).unwrap();
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(
            a.recipients[0].ephemeral_public,
            b.recipients[0].ephemeral_public
        );
    }

    #[test]
    fn both_parties_can_open_a_bidirectional_envelope() {
        // The whole point of v2: the seller can answer a false "that is not
        // what you sent", which it could not do when the envelope was sealed to
        // the buyer alone.
        let (buyer_pub, buyer_sk) = secp_pair();
        let (seller_pub, seller_sk) = secp_pair();
        let body = b"what was actually delivered";

        let env = seal_to(
            body,
            &[
                (RecipientRole::Payer, buyer_pub),
                (RecipientRole::Seller, seller_pub),
            ],
            PID,
        )
        .unwrap();

        assert_eq!(open(&env, &buyer_sk, PID).unwrap(), body);
        assert_eq!(open(&env, &seller_sk, PID).unwrap(), body);
    }

    #[test]
    fn the_body_is_encrypted_once_regardless_of_recipients() {
        // Adding a party costs a wrapped key, not a second copy of the payload.
        // If this ever regressed, bidirectional evidence would double storage.
        let (a, _) = secp_pair();
        let (b, _) = secp_pair();
        let (c, _) = secp_pair();
        let body = vec![7u8; 4096];

        let one = seal_to(&body, &[(RecipientRole::Payer, a.clone())], PID).unwrap();
        let three = seal_to(
            &body,
            &[
                (RecipientRole::Payer, a),
                (RecipientRole::Seller, b),
                (RecipientRole::Auditor, c),
            ],
            PID,
        )
        .unwrap();

        assert_eq!(one.ciphertext.len(), three.ciphertext.len());
        let overhead = three.to_bytes().len() - one.to_bytes().len();
        assert!(overhead < 200, "two extra recipients cost {overhead} bytes");
    }

    #[test]
    fn a_stranger_still_cannot_open_a_multi_recipient_envelope() {
        let (buyer, _) = secp_pair();
        let (seller, _) = secp_pair();
        let (_, stranger_sk) = secp_pair();

        let env = seal_to(
            b"private",
            &[
                (RecipientRole::Payer, buyer),
                (RecipientRole::Seller, seller),
            ],
            PID,
        )
        .unwrap();
        assert!(matches!(
            open(&env, &stranger_sk, PID),
            Err(EnvelopeError::Aead)
        ));
    }

    #[test]
    fn a_single_payer_envelope_is_still_emitted_as_v1() {
        // Byte-for-byte compatibility with every reader already deployed. If
        // this flipped to v2, the SDKs in the wild would stop reading new
        // evidence.
        let (pk, _) = secp_pair();
        let blob = seal(b"body", &pk, PID).unwrap().to_bytes();
        assert_eq!(blob[5], 1, "single-payer envelopes must stay v1");

        let (a, _) = secp_pair();
        let (b, _) = secp_pair();
        let multi = seal_to(
            b"body",
            &[(RecipientRole::Payer, a), (RecipientRole::Seller, b)],
            PID,
        )
        .unwrap()
        .to_bytes();
        assert_eq!(multi[5], 2, "multi-recipient envelopes must be v2");
    }

    #[test]
    fn roles_survive_a_serialization_round_trip() {
        // A buyer has to be able to see who else holds a key.
        let (a, _) = secp_pair();
        let (b, _) = secp_pair();
        let (c, _) = ed_pair();
        let env = seal_to(
            b"body",
            &[
                (RecipientRole::Payer, a),
                (RecipientRole::Seller, b),
                (RecipientRole::Auditor, c),
            ],
            PID,
        )
        .unwrap();

        let parsed = SealedEnvelope::from_bytes(&env.to_bytes()).unwrap();
        assert_eq!(
            parsed.roles(),
            vec![
                RecipientRole::Payer,
                RecipientRole::Seller,
                RecipientRole::Auditor
            ]
        );
    }

    #[test]
    fn mixed_curves_across_recipients_both_open() {
        // A Solana buyer and an EVM seller on the same evidence.
        let (buyer, buyer_sk) = ed_pair();
        let (seller, seller_sk) = secp_pair();
        let env = seal_to(
            b"cross-curve",
            &[
                (RecipientRole::Payer, buyer),
                (RecipientRole::Seller, seller),
            ],
            PID,
        )
        .unwrap();

        assert_eq!(open(&env, &buyer_sk, PID).unwrap(), b"cross-curve");
        assert_eq!(open(&env, &seller_sk, PID).unwrap(), b"cross-curve");
    }

    #[test]
    fn an_envelope_with_no_recipients_is_refused() {
        let (pk, _) = secp_pair();
        let _ = pk;
        assert!(seal_to(b"body", &[], PID).is_err());
    }

    #[test]
    fn empty_body_round_trips() {
        let (pk, sk) = secp_pair();
        let env = seal(b"", &pk, PID).unwrap();
        assert_eq!(open(&env, &sk, PID).unwrap(), b"");
    }
}
