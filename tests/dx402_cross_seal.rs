//! Rust opens what the PYTHON SDK sealed.
//!
//! The other direction (Rust seals, Python opens) is covered by
//! `tests/dx402_vector_gen.rs` plus the SDK test suites. This file closes the
//! loop: it proves the SELLER half of the SDKs produces envelopes this
//! implementation accepts, which is what a non-Rust resource server actually
//! needs in order to anchor anything at all.
//!
//! The fixtures under `tests/vectors/python-sealed-*.hex` were produced by the
//! Python SDK's `seal_evidence()` and are committed on purpose, so this runs in
//! CI without a Python step. An env-var-gated test that silently skips when the
//! blobs are missing would look green while proving nothing.
//!
//! Regenerate (only when the envelope format changes on purpose):
//!   cd uvd-x402-sdk-python && PYTHONPATH=src python3 -c "..."   # see the SDK tests

use x402_rs::dx402::envelope::{open, PayerSecretKey, SealedEnvelope};

const PID: &[u8] = b"0x1111111111111111111111111111111111111111111111111111111111111111";
const BODY: &[u8] = b"the paid response that must outlive the session";

fn load(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    let hex_str = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    hex::decode(hex_str.trim()).expect("fixture should be valid hex")
}

#[test]
fn rust_opens_a_python_sealed_secp256k1_envelope() {
    let blob = load("python-sealed-secp256k1.hex");
    let envelope = SealedEnvelope::from_bytes(&blob).expect("python blob should parse");
    let sk = k256::SecretKey::from_slice(&[0x42u8; 32]).unwrap();
    let plaintext = open(&envelope, &PayerSecretKey::Secp256k1(Box::new(sk)), PID)
        .expect("rust should open a python-sealed envelope");
    assert_eq!(plaintext, BODY);
}

#[test]
fn rust_opens_a_python_sealed_x25519_envelope() {
    // This is the one that would catch a wrong ed25519 -> X25519 public-key
    // conversion in the SDK. Python sealing and unsealing with its own map
    // proves nothing: both sides would share the same mistake. Only a second
    // implementation agreeing does.
    let blob = load("python-sealed-x25519.hex");
    let envelope = SealedEnvelope::from_bytes(&blob).expect("python blob should parse");
    let plaintext = open(&envelope, &PayerSecretKey::Ed25519Seed([0x37u8; 32]), PID)
        .expect("rust should open a python-sealed envelope");
    assert_eq!(plaintext, BODY);
}
