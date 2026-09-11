//! A seller saying how long its price stands, and a buyer able to check it.
use std::time::Duration;

use x402_axum::layer::{X402Error, OFFER_VALIDITY_EXTENSION};
use x402_rs::types::PaymentRequiredResponse;

/// The buyer's reader, restated here so this test does not need the buyer crate:
/// seller and buyer must read the SAME envelope from the SAME key.
fn offer_valid_until(
    extensions: &std::collections::HashMap<String, serde_json::Value>,
) -> Option<u64> {
    extensions
        .get(OFFER_VALIDITY_EXTENSION)?
        .get("info")?
        .get("validUntil")?
        .as_u64()
}

fn challenge_with_validity(valid_for: Duration) -> PaymentRequiredResponse {
    let err = X402Error::payment_header_required(Vec::new()).with_offer_validity(valid_for);
    // Round-trip through the wire, because what matters is what a buyer READS,
    // not what the seller holds in memory.
    let bytes = serde_json::to_vec(err.challenge()).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn what_the_seller_declares_is_what_the_buyer_reads() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let challenge = challenge_with_validity(Duration::from_secs(120));

    assert!(
        challenge.extensions.contains_key(OFFER_VALIDITY_EXTENSION),
        "the key carries its version, so a later transport change is visible"
    );
    let valid_until = offer_valid_until(&challenge.extensions)
        .expect("the buyer reads the seller's own declaration");
    assert!(
        (now + 118..=now + 122).contains(&valid_until),
        "expected about now+120, got {valid_until}"
    );
}

#[test]
fn a_challenge_without_the_extension_states_no_expiry() {
    // Not an error and not an expiry of zero: a seller that says nothing has
    // made no commitment, and inventing one on its behalf would be inventing
    // terms.
    let err = X402Error::payment_header_required(Vec::new());
    let bytes = serde_json::to_vec(err.challenge()).unwrap();
    let challenge: PaymentRequiredResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(offer_valid_until(&challenge.extensions), None);
}

#[test]
fn the_key_is_the_literal_both_sides_publish() {
    // The previous version of this test compared the re-export with its own
    // source and could never fail. This pins the LITERAL: a rename in the shared
    // crate changes what goes on the wire for every seller and every buyer at
    // once, and silence is not the right response to that.
    assert_eq!(OFFER_VALIDITY_EXTENSION, "offer-receipt/1");
    // And the version is in the key, which is the property the annex insists on:
    // the extension's transport may still change, and a value read from an
    // unversioned key could not be compared against anything later.
    let (name, version) = OFFER_VALIDITY_EXTENSION
        .split_once('/')
        .expect("the key must carry its version");
    assert_eq!(name, "offer-receipt");
    assert!(
        version.parse::<u32>().is_ok(),
        "the version must be a number, got {version:?}"
    );
}
