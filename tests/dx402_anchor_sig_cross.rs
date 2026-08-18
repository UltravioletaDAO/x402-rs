//! Rust verifies anchor authorizations produced by the PYTHON SDK.
//!
//! This is the check that matters for the helper: an EIP-712 digest built
//! slightly differently does not error anywhere -- it yields a signature that
//! simply never verifies, and the seller's anchor silently stays provisional
//! with no clue why. Only a second implementation accepting the signature
//! establishes that the two agree.
//!
//! Regenerate the signatures from the Python SDK; see
//! `uvd-x402-sdk-python/tests/test_dx402.py`.

use x402_rs::dx402::gate::verify_authorization_for;
use x402_rs::types::MixedAddress;

fn addr(s: &str) -> MixedAddress {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
}

fn b256(hex_str: &str) -> alloy::primitives::B256 {
    hex_str.parse().unwrap()
}

const PID: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
const CH: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn rust_accepts_a_python_signed_ed25519_anchor() {
    // The Solana path. Without it that chain could not prove authorship at all,
    // so the anchor stayed hijackable even with the on-chain gate enforced.
    assert!(verify_authorization_for(
        &addr("3znAGhp6Tk4kmebhXnk9K3jaTMffu82PJfEG91AeRkq2"),
        "0x66bea5bbcc3ccdf9652739e6b3c85916ee4431ebd886d80c1e72b6c400d151254f58b4fc194b33e14ada62cf910e7d29db8c1c727af17f5d43472c6e6423ea02",
        b256(PID),
        b256(CH),
        "",
        0
    ));
}

#[test]
fn rust_accepts_a_python_signed_evm_anchor() {
    // The payee IS the signing key's address -- that is the whole claim being
    // made. An earlier version of this test signed with one key and declared a
    // different payee, and verification correctly refused it.
    assert!(verify_authorization_for(
        &addr("0x17c5185167401eD00cF5F5b2fc97D9BBfDb7D025"),
        "0xccae22505c387aac0074cdd81d71f38341f1e865a6e504f5dcdd56b98654c11073e899ae3200af924f961cae8f50bf845bd090c71cc99f446f8bd28237c9b9d600",
        b256(PID),
        b256(CH),
        "s3+https://e/x",
        8453
    ));
}

#[test]
fn a_python_signature_is_bound_to_its_content() {
    // Same signature, different content hash: must not verify. Otherwise a
    // seller could authorise one anchor and have something else stored under it.
    assert!(!verify_authorization_for(
        &addr("3znAGhp6Tk4kmebhXnk9K3jaTMffu82PJfEG91AeRkq2"),
        "0x66bea5bbcc3ccdf9652739e6b3c85916ee4431ebd886d80c1e72b6c400d151254f58b4fc194b33e14ada62cf910e7d29db8c1c727af17f5d43472c6e6423ea02",
        b256(PID),
        b256("0x9999999999999999999999999999999999999999999999999999999999999999"),
        "",
        0
    ));
}
