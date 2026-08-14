//! Recovering the payer's public key from the payment itself.
//!
//! This is the idea DX402 rests on:
//!
//! > A payment authorization is a digital signature. A digital signature yields
//! > the signer's **public key**, not merely their address. So the resource
//! > server can encrypt to the payer using key material the payment already
//! > produced -- no registration, no key exchange, no extra round trip.
//!
//! Paying *is* publishing your encryption key.
//!
//! Coverage across the seven families in [`crate::network::NetworkFamily`]:
//!
//! | Family | Curve | Where the key comes from |
//! |---|---|---|
//! | EVM | secp256k1 | ECDSA recovery over the EIP-712 digest |
//! | Solana / Fogo | ed25519 | the address **is** the key |
//! | NEAR | ed25519 | access key (`ed25519:...`) |
//! | Stellar | ed25519 | the `G...` address is the encoded key |
//! | Algorand | ed25519 | address is key + 4-byte checksum |
//! | Sui | either | address is a *hash*; the signature carries the key |
//! | XRPL | either | `SigningPubKey` of the signed transaction |
//!
//! Four of seven need nothing but the address. The rest read the key off a
//! signature we already hold.
//!
//! # What is not recoverable
//!
//! Smart-contract wallets (EIP-1271 / EIP-6492) have no EOA key to recover, and
//! Sui addresses are hashes. When no key can be derived the caller must skip
//! evidence with [`super::types::SkipReason::NoPayerKey`] -- never guess, and
//! never fail the payment.

use thiserror::Error;

use super::envelope::PayerPublicKey;

#[derive(Debug, Error)]
pub enum PubKeyError {
    #[error("signature must be 65 bytes, got {0}")]
    BadSignatureLength(usize),
    #[error("invalid recovery id {0}")]
    BadRecoveryId(u8),
    #[error("ECDSA recovery failed: {0}")]
    RecoveryFailed(String),
    #[error("recovered key belongs to {recovered}, expected {expected}")]
    AddressMismatch { recovered: String, expected: String },
    #[error("invalid ed25519 public key: {0}")]
    InvalidEd25519(String),
    #[error("malformed {kind} address: {reason}")]
    MalformedAddress { kind: &'static str, reason: String },
    #[error("no recoverable key: {0}")]
    NoKey(&'static str),
}

/// Recover a secp256k1 public key from an ECDSA signature over `digest`.
///
/// `signature` is the usual 65-byte `r ‖ s ‖ v` layout; `v` is accepted as
/// `0/1` or `27/28`.
///
/// When `expected_address` is supplied the recovered key is checked against it.
/// That check is not ceremony: without it, a malformed or substituted signature
/// would yield *some* valid public key, and the body would be encrypted to a
/// stranger while looking entirely successful.
pub fn from_evm_signature(
    signature: &[u8],
    digest: &[u8; 32],
    expected_address: Option<&alloy::primitives::Address>,
) -> Result<PayerPublicKey, PubKeyError> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    if signature.len() != 65 {
        return Err(PubKeyError::BadSignatureLength(signature.len()));
    }

    let sig = Signature::from_slice(&signature[..64])
        .map_err(|e| PubKeyError::RecoveryFailed(e.to_string()))?;

    let v = signature[64];
    let rec_byte = match v {
        0 | 1 => v,
        27 | 28 => v - 27,
        // Some EIP-155 style encodings carry a chain-shifted v. Fold it back.
        other if other >= 35 => (other - 35) % 2,
        other => return Err(PubKeyError::BadRecoveryId(other)),
    };
    let rec_id = RecoveryId::from_byte(rec_byte).ok_or(PubKeyError::BadRecoveryId(rec_byte))?;

    let vk = VerifyingKey::recover_from_prehash(digest, &sig, rec_id)
        .map_err(|e| PubKeyError::RecoveryFailed(e.to_string()))?;

    if let Some(expected) = expected_address {
        let recovered = evm_address_of(&vk);
        if recovered != *expected {
            return Err(PubKeyError::AddressMismatch {
                recovered: recovered.to_string(),
                expected: expected.to_string(),
            });
        }
    }

    Ok(PayerPublicKey::Secp256k1(Box::new(k256::PublicKey::from(
        &vk,
    ))))
}

/// The EVM address of a secp256k1 verifying key: last 20 bytes of the keccak256
/// of the uncompressed point, minus its `0x04` tag.
fn evm_address_of(vk: &k256::ecdsa::VerifyingKey) -> alloy::primitives::Address {
    use alloy::primitives::keccak256;
    let point = vk.to_encoded_point(false);
    let hash = keccak256(&point.as_bytes()[1..]);
    alloy::primitives::Address::from_slice(&hash[12..])
}

/// Build a payer key from a raw 32-byte ed25519 public key.
pub fn from_ed25519_bytes(bytes: &[u8]) -> Result<PayerPublicKey, PubKeyError> {
    let arr: [u8; 32] = bytes.try_into().map_err(|_| {
        PubKeyError::InvalidEd25519(format!("expected 32 bytes, got {}", bytes.len()))
    })?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map_err(|e| PubKeyError::InvalidEd25519(e.to_string()))?;
    Ok(super::envelope::ed25519_to_x25519_public(&vk))
}

/// Solana (and Fogo) addresses are base58-encoded ed25519 public keys.
///
/// No signature needed at all: the address the payment was made from is already
/// the encryption target.
pub fn from_solana_address(address: &str) -> Result<PayerPublicKey, PubKeyError> {
    let bytes = bs58::decode(address)
        .into_vec()
        .map_err(|e| PubKeyError::MalformedAddress {
            kind: "solana",
            reason: e.to_string(),
        })?;
    from_ed25519_bytes(&bytes)
}

/// NEAR access keys are written `ed25519:<base58>`.
///
/// Note this takes the *public key*, not the account id: `alice.near` is a name,
/// not key material.
pub fn from_near_public_key(key: &str) -> Result<PayerPublicKey, PubKeyError> {
    let raw = key
        .strip_prefix("ed25519:")
        .ok_or(PubKeyError::MalformedAddress {
            kind: "near",
            reason: "expected an ed25519: prefix".into(),
        })?;
    let bytes = bs58::decode(raw)
        .into_vec()
        .map_err(|e| PubKeyError::MalformedAddress {
            kind: "near",
            reason: e.to_string(),
        })?;
    from_ed25519_bytes(&bytes)
}

/// Stellar `G...` account addresses are strkey-encoded ed25519 public keys.
///
/// Contract addresses (`C...`) are not keys and are rejected.
pub fn from_stellar_address(address: &str) -> Result<PayerPublicKey, PubKeyError> {
    let pk = stellar_strkey::ed25519::PublicKey::from_string(address).map_err(|e| {
        PubKeyError::MalformedAddress {
            kind: "stellar",
            reason: e.to_string(),
        }
    })?;
    from_ed25519_bytes(&pk.0)
}

/// Algorand addresses are RFC 4648 base32 (unpadded) of `pubkey ‖ checksum[4]`.
pub fn from_algorand_address(address: &str) -> Result<PayerPublicKey, PubKeyError> {
    let decoded = base32_decode(address).ok_or(PubKeyError::MalformedAddress {
        kind: "algorand",
        reason: "not valid unpadded base32".into(),
    })?;
    if decoded.len() != 36 {
        return Err(PubKeyError::MalformedAddress {
            kind: "algorand",
            reason: format!("expected 36 decoded bytes, got {}", decoded.len()),
        });
    }
    from_ed25519_bytes(&decoded[..32])
}

/// Minimal RFC 4648 base32 decoder (uppercase alphabet, no padding).
///
/// Hand-rolled rather than pulling a dependency, and kept separate from the
/// `algorand` cargo feature so evidence works on a build without it.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buffer: u16 = 0;
    let mut bits: u8 = 0;
    for ch in s.bytes() {
        let val = ALPHABET.iter().position(|&c| c == ch)? as u16;
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Sui and XRPL carry the signer's public key inside the signature envelope
/// rather than in the address, so the caller extracts it and hands it here.
pub fn from_raw_public_key(bytes: &[u8], curve: RawCurve) -> Result<PayerPublicKey, PubKeyError> {
    match curve {
        RawCurve::Ed25519 => from_ed25519_bytes(bytes),
        RawCurve::Secp256k1 => {
            let pk = k256::PublicKey::from_sec1_bytes(bytes)
                .map_err(|e| PubKeyError::RecoveryFailed(e.to_string()))?;
            Ok(PayerPublicKey::Secp256k1(Box::new(pk)))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawCurve {
    Ed25519,
    Secp256k1,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx402::envelope::{open, seal, PayerSecretKey};
    use k256::elliptic_curve::sec1::ToEncodedPoint;

    #[test]
    fn evm_recovery_yields_a_key_that_decrypts() {
        // The end-to-end property that matters: a key recovered from a signature
        // is genuinely the payer's, so a body sealed to it opens with the payer's
        // private key and nothing else.
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let digest = [7u8; 32];
        let (sig, rec_id) = sk.sign_prehash(&digest).unwrap();

        let mut raw = [0u8; 65];
        raw[..64].copy_from_slice(&sig.to_bytes());
        raw[64] = rec_id.to_byte();

        let expected = evm_address_of(sk.verifying_key());
        let payer = from_evm_signature(&raw, &digest, Some(&expected)).unwrap();

        let body = b"recovered from the payment signature alone";
        let env = seal(body, &payer, b"pid").unwrap();
        let secret = PayerSecretKey::Secp256k1(Box::new(k256::SecretKey::from(&sk)));
        assert_eq!(open(&env, &secret, b"pid").unwrap(), body);
    }

    #[test]
    fn evm_recovery_accepts_both_v_encodings() {
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let digest = [3u8; 32];
        let (sig, rec_id) = sk.sign_prehash(&digest).unwrap();

        let mut low = [0u8; 65];
        low[..64].copy_from_slice(&sig.to_bytes());
        low[64] = rec_id.to_byte();

        let mut legacy = low;
        legacy[64] = rec_id.to_byte() + 27;

        assert_eq!(
            from_evm_signature(&low, &digest, None).unwrap(),
            from_evm_signature(&legacy, &digest, None).unwrap()
        );
    }

    #[test]
    fn evm_recovery_rejects_a_mismatched_address() {
        // Without this check a substituted signature would silently encrypt the
        // body to whoever the forged signature happened to recover to.
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let other = SigningKey::random(&mut rand::rngs::OsRng);
        let digest = [1u8; 32];
        let (sig, rec_id) = sk.sign_prehash(&digest).unwrap();

        let mut raw = [0u8; 65];
        raw[..64].copy_from_slice(&sig.to_bytes());
        raw[64] = rec_id.to_byte();

        let wrong = evm_address_of(other.verifying_key());
        assert!(matches!(
            from_evm_signature(&raw, &digest, Some(&wrong)),
            Err(PubKeyError::AddressMismatch { .. })
        ));
    }

    #[test]
    fn evm_recovery_rejects_bad_lengths_and_recovery_ids() {
        use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};

        assert!(matches!(
            from_evm_signature(&[0u8; 64], &[0u8; 32], None),
            Err(PubKeyError::BadSignatureLength(64))
        ));

        // Use a genuinely valid r/s so the recovery id is what is actually under
        // test. An all-zero buffer fails at signature parsing first and would
        // pass this test for the wrong reason.
        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let digest = [5u8; 32];
        let (sig, _) = sk.sign_prehash(&digest).unwrap();
        let mut raw = [0u8; 65];
        raw[..64].copy_from_slice(&sig.to_bytes());
        raw[64] = 9;
        assert!(matches!(
            from_evm_signature(&raw, &digest, None),
            Err(PubKeyError::BadRecoveryId(9))
        ));

        // A structurally invalid signature is reported as a recovery failure.
        assert!(matches!(
            from_evm_signature(&[0u8; 65], &[0u8; 32], None),
            Err(PubKeyError::RecoveryFailed(_))
        ));
    }

    #[test]
    fn solana_address_is_the_encryption_key() {
        // No signature involved. The address alone is enough, which is what makes
        // ed25519 chains the cheapest case for DX402.
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let address = bs58::encode(sk.verifying_key().to_bytes()).into_string();

        let payer = from_solana_address(&address).unwrap();
        let body = b"derived from a base58 address";
        let env = seal(body, &payer, b"pid").unwrap();
        assert_eq!(
            open(&env, &PayerSecretKey::Ed25519Seed(seed), b"pid").unwrap(),
            body
        );
    }

    #[test]
    fn near_public_key_round_trips() {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let key = format!(
            "ed25519:{}",
            bs58::encode(sk.verifying_key().to_bytes()).into_string()
        );

        let payer = from_near_public_key(&key).unwrap();
        let env = seal(b"near body", &payer, b"pid").unwrap();
        assert_eq!(
            open(&env, &PayerSecretKey::Ed25519Seed(seed), b"pid").unwrap(),
            b"near body"
        );
    }

    #[test]
    fn near_rejects_an_account_id() {
        // `alice.near` is a name, not key material. Accepting it would produce a
        // confident-looking failure much later, at decrypt time.
        assert!(matches!(
            from_near_public_key("uvd-facilitator.near"),
            Err(PubKeyError::MalformedAddress { kind: "near", .. })
        ));
    }

    #[test]
    fn stellar_address_round_trips() {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let address = stellar_strkey::ed25519::PublicKey(sk.verifying_key().to_bytes()).to_string();

        let payer = from_stellar_address(&address).unwrap();
        let env = seal(b"stellar body", &payer, b"pid").unwrap();
        assert_eq!(
            open(&env, &PayerSecretKey::Ed25519Seed(seed), b"pid").unwrap(),
            b"stellar body"
        );
    }

    #[test]
    fn stellar_rejects_a_contract_address() {
        assert!(
            from_stellar_address("CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75")
                .is_err()
        );
    }

    #[test]
    fn algorand_address_round_trips() {
        // Real mainnet facilitator address from lambda/balances/handler.py --
        // decoding it must yield a usable 32-byte key.
        let payer =
            from_algorand_address("KIMS5H6QLCUDL65L5UBTOXDPWLMTS7N3AAC3I6B2NCONEI5QIVK7LH2C2I")
                .unwrap();
        assert!(matches!(payer, PayerPublicKey::X25519(_)));
    }

    #[test]
    fn algorand_rejects_malformed_input() {
        assert!(from_algorand_address("not-base32-at-all!").is_err());
        assert!(from_algorand_address("AAAA").is_err());
    }

    #[test]
    fn base32_matches_known_vectors() {
        // RFC 4648 section 10 vectors with the padding stripped, so the
        // hand-rolled decoder is checked against the standard rather than
        // against itself. Padded forms for reference:
        //   f -> MY======   fo -> MZXQ====   foo -> MZXW6===
        //   foob -> MZXW6YQ=   fooba -> MZXW6YTB   foobar -> MZXW6YTBOI======
        assert_eq!(base32_decode("MY").unwrap(), b"f");
        assert_eq!(base32_decode("MZXQ").unwrap(), b"fo");
        assert_eq!(base32_decode("MZXW6").unwrap(), b"foo");
        assert_eq!(base32_decode("MZXW6YQ").unwrap(), b"foob");
        assert_eq!(base32_decode("MZXW6YTB").unwrap(), b"fooba");
        assert_eq!(base32_decode("MZXW6YTBOI").unwrap(), b"foobar");
        assert!(
            base32_decode("mzxw6").is_none(),
            "lowercase must be rejected"
        );
        assert!(
            base32_decode("MZXW1").is_none(),
            "'1' is not in the alphabet"
        );
    }

    #[test]
    fn raw_secp256k1_key_is_accepted() {
        let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
        let sec1 = sk.public_key().to_encoded_point(true).as_bytes().to_vec();
        let payer = from_raw_public_key(&sec1, RawCurve::Secp256k1).unwrap();
        let env = seal(b"xrpl body", &payer, b"pid").unwrap();
        assert_eq!(
            open(&env, &PayerSecretKey::Secp256k1(Box::new(sk)), b"pid").unwrap(),
            b"xrpl body"
        );
    }

    #[test]
    fn wrong_length_ed25519_keys_are_rejected() {
        assert!(from_ed25519_bytes(&[0u8; 31]).is_err());
        assert!(from_ed25519_bytes(&[0u8; 33]).is_err());
    }

    #[test]
    fn small_order_keys_cannot_be_sealed_to() {
        // `ed25519-dalek` accepts non-canonical and small-order encodings in
        // `VerifyingKey::from_bytes`, so this does NOT fail here -- it has to be
        // caught at the ECDH, where a small-order point drives the shared secret
        // to a constant that anyone could reproduce.
        //
        // The canonical small-order u-coordinates -- the same seven values
        // libsodium blacklists. Taken from that list rather than invented here,
        // because a made-up "looks small-order" value is just a valid point and
        // would let this test pass while proving nothing.
        const SMALL_ORDER: [&str; 7] = [
            // order 1 and 2
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0100000000000000000000000000000000000000000000000000000000000000",
            // order 8
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            // p-1, p, p+1 (non-canonical encodings of small-order points)
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ];

        for encoded in SMALL_ORDER {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&hex::decode(encoded).unwrap());
            let point =
                PayerPublicKey::X25519(curve25519_dalek::montgomery::MontgomeryPoint(bytes));
            assert!(
                matches!(
                    seal(b"body", &point, b"pid"),
                    Err(crate::dx402::envelope::EnvelopeError::DegenerateSharedSecret)
                ),
                "small-order point {encoded} was accepted for sealing"
            );
        }
    }
}
