//! A seller may advertise more schemes than this buyer can pay.
use x402_rs::types::PaymentRequiredResponse;

/// One `exact` offer this buyer can pay, and one `batch-settlement` it cannot.
const MIXED: &str = r#"{
  "x402Version": 1,
  "error": "Payment required",
  "accepts": [
    {
      "scheme": "batch-settlement",
      "network": "base",
      "maxAmountRequired": "30000",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
      "resource": "https://api.example.com/thing",
      "description": "batched",
      "mimeType": "application/json",
      "maxTimeoutSeconds": 300
    },
    {
      "scheme": "exact",
      "network": "base",
      "maxAmountRequired": "10000",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
      "resource": "https://api.example.com/thing",
      "description": "plain",
      "mimeType": "application/json",
      "maxTimeoutSeconds": 300
    }
  ]
}"#;

#[test]
fn a_scheme_we_cannot_pay_does_not_cost_us_the_one_we_can() {
    // The whole challenge used to fail to deserialize because ONE entry named a
    // scheme this build does not implement -- so a buyer lost the `exact` offer
    // sitting right beside it and could not pay a seller that was perfectly
    // payable. The same failure the catalog had in P0, one layer along.
    let parsed: PaymentRequiredResponse =
        serde_json::from_str(MIXED).expect("a challenge must survive an offer we cannot read");

    assert_eq!(parsed.accepts.len(), 1, "the readable offer survives");
    assert_eq!(parsed.accepts[0].max_amount_required.to_string(), "10000");
    assert_eq!(
        parsed.unreadable_offers.len(),
        1,
        "and the one we could not read is counted, not silently dropped"
    );
    assert_eq!(
        parsed.unreadable_offers[0].scheme.as_deref(),
        Some("batch-settlement")
    );
}

/// A challenge whose offers are ALL unreadable.
const NONE_READABLE: &str = r#"{
  "x402Version": 1,
  "error": "Payment required",
  "accepts": [
    {"scheme": "batch-settlement", "network": "base", "maxAmountRequired": "1",
     "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
     "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
     "resource": "https://api.example.com/t", "description": "", "mimeType": "",
     "maxTimeoutSeconds": 300},
    {"scheme": "agent-pay", "network": "base", "maxAmountRequired": "1",
     "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
     "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
     "resource": "https://api.example.com/t", "description": "", "mimeType": "",
     "maxTimeoutSeconds": 300}
  ]
}"#;

#[test]
fn a_challenge_with_nothing_payable_still_parses_and_names_what_was_offered() {
    // The parse succeeding is the point: the buyer has to be able to SAY what
    // the seller wanted. A refusal reading "Accepted: []" sends a caller looking
    // for a bug in its own code.
    let parsed: PaymentRequiredResponse = serde_json::from_str(NONE_READABLE).unwrap();
    assert!(parsed.accepts.is_empty());
    assert_eq!(parsed.unreadable_offers.len(), 2);

    let refusal = x402_reqwest::policy::no_readable_offer(&parsed.unreadable_offers);
    assert_eq!(refusal.code(), "no-readable-offer");
    let message = refusal.to_string();
    assert!(message.contains("batch-settlement"), "{message}");
    assert!(message.contains("agent-pay"), "{message}");
}

#[test]
fn the_sellers_validity_reaches_the_buyers_policy_from_a_real_challenge() {
    // The end-to-end path that was NOT wired: a seller declares how long its
    // offer stands in the challenge's `extensions`, and the middleware has to
    // carry that map into the policy. Passing `accepts` alone threw the
    // declaration away before anything could read it.
    let with_validity = MIXED.replace(
        r#""accepts": ["#,
        r#""extensions": {"offer-receipt/1": {"info": {"validUntil": 1700000000}}}, "accepts": ["#,
    );
    let parsed: PaymentRequiredResponse = serde_json::from_str(&with_validity).unwrap();
    assert_eq!(
        x402_reqwest::policy::offer_valid_until(&parsed.extensions),
        Some(1_700_000_000),
        "what the seller declared must survive the parse the buyer actually runs"
    );
}

// ============================================================================
// The path the middleware actually takes
// ============================================================================
//
// These drive `pay_for_challenge`, which is what `Middleware::handle` calls.
// The wiring that carries a challenge's `extensions` into the policy was missing
// for a whole commit while every unit test passed, because nothing exercised the
// decision end to end. This is that test.

use std::time::Duration;
use x402_reqwest::policy::PurchasePolicy;
use x402_reqwest::X402Payments;
use x402_rs::network::Network;
use x402_rs::types::{TokenAmount, TokenAsset};

/// A wallet is required to build the client; no signing happens in these tests
/// because every one of them is refused before the wallet is reached.
fn client() -> X402Payments {
    let signer = alloy::signers::local::PrivateKeySigner::random();
    X402Payments::with_wallet(signer)
}

fn usdc_base() -> TokenAsset {
    TokenAsset {
        address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
            .parse::<x402_rs::types::EvmAddress>()
            .unwrap()
            .into(),
        network: Network::Base,
    }
}

fn challenge_expiring_at(valid_until: u64) -> PaymentRequiredResponse {
    let json = MIXED.replace(
        r#""accepts": ["#,
        &format!(
            r#""extensions": {{"offer-receipt/1": {{"info": {{"validUntil": {valid_until}}}}}}}, "accepts": ["#
        ),
    );
    serde_json::from_str(&json).unwrap()
}

#[tokio::test]
async fn an_expired_offer_is_refused_on_the_path_the_middleware_takes() {
    // The seller said these terms stood until an instant that has passed. The
    // policy knows how to check that and the parse preserves it; what was
    // missing was the wire between them, and only a test at this level sees it.
    let expired = challenge_expiring_at(1_000); // 1970, thoroughly past
    let payments = client().with_policy(
        PurchasePolicy::new().per_payment(usdc_base(), TokenAmount::from(1_000_000u64)),
    );

    let err = payments
        .pay_for_challenge(&expired)
        .await
        .expect_err("an offer that lapsed must not be signed");
    let message = err.to_string();
    assert!(
        message.contains("expired"),
        "expected an expiry refusal, got: {message}"
    );
}

#[tokio::test]
async fn an_unexpired_offer_is_not_refused_for_expiry() {
    // The other half: the check must not refuse everything. A far-future
    // `validUntil` gets past the expiry gate.
    let far_future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + Duration::from_secs(3600).as_secs();
    let live = challenge_expiring_at(far_future);
    let payments = client().with_policy(
        PurchasePolicy::new().per_payment(usdc_base(), TokenAmount::from(1_000_000u64)),
    );

    // It may still fail for wallet reasons; what it must NOT say is "expired".
    if let Err(e) = payments.pay_for_challenge(&live).await {
        assert!(
            !e.to_string().contains("expired"),
            "a live offer was refused as expired: {e}"
        );
    }
}

#[tokio::test]
async fn a_challenge_with_nothing_payable_refuses_by_naming_the_schemes() {
    let none: PaymentRequiredResponse = serde_json::from_str(NONE_READABLE).unwrap();
    let err = client()
        .pay_for_challenge(&none)
        .await
        .expect_err("nothing here is payable");
    let message = err.to_string();
    assert!(message.contains("batch-settlement"), "{message}");
}

#[tokio::test]
async fn an_asset_the_policy_never_budgeted_is_refused_on_the_same_path() {
    // Default-deny reaches the real path too, not just the unit test.
    let live = challenge_expiring_at(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600,
    );
    let payments = client().with_policy(PurchasePolicy::new()); // no budget at all
    let err = payments
        .pay_for_challenge(&live)
        .await
        .expect_err("an unbudgeted asset must not be signed");
    assert!(
        err.to_string().contains("no budget"),
        "expected an asset refusal, got: {err}"
    );
}
