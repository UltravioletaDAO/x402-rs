//! What the catalog says about the AGE of a price, on the wire.
//!
//! # What these pin
//!
//! A listing is a claim somebody else made about their own money, and a
//! consumer's first question about one is not "what does it cost" but "when did
//! anyone last check". Before this phase the catalog could not answer that, and
//! two of its fields actively lied about it:
//!
//! 1. **A payment moved the content date.** `track_settlement` bumped
//!    `lastUpdated`, so commercial activity made a record look freshly written.
//! 2. **Nothing recorded what the origin actually answers.** The health prober
//!    reads a live 402 on every probe and kept only the recipients, so a
//!    resource could be marked alive and go on advertising a price from months
//!    ago.
//!
//! These run against the real registry and assert on the **serialized listing**,
//! because the field names are the contract: an SDK, the bazaar page and the
//! next phase's scheduler all read them.

use serde_json::Value;
use url::Url;

use x402_rs::discovery::DiscoveryRegistry;
use x402_rs::discovery_price::{normalize_declared_option, DeclaredPaymentOption};
use x402_rs::discovery_terms::{
    ObservationContext, ObservationPhase, ObservedTerms, TermsProvenance, TermsTransport,
};
use x402_rs::types_v2::{DiscoveryResource, RECORD_FORMAT_VERSION};

const PAYEE: &str = "0xe4dc963c56979E0260fc146b87eE24F18220e545";
const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

fn option(amount: &str) -> x402_rs::discovery_price::CatalogPaymentOption {
    normalize_declared_option(DeclaredPaymentOption {
        scheme: Some("exact".to_string()),
        network: Some("base".to_string()),
        asset: Some(USDC_BASE.to_string()),
        amount: Some(serde_json::json!(amount)),
        pay_to: Some(PAYEE.to_string()),
        max_timeout_seconds: Some(300),
        ..Default::default()
    })
    .expect("fixture option must normalize")
}

fn resource(url: &str, amount: &str) -> DiscoveryResource {
    DiscoveryResource::new(
        Url::parse(url).unwrap(),
        "http".to_string(),
        "A paid endpoint".to_string(),
        vec![option(amount)],
    )
}

fn observation(amount: &str, observed_at: u64, against: Option<String>) -> ObservedTerms {
    ObservedTerms {
        accepts: vec![option(amount)],
        observed_at,
        context: ObservationContext::anonymous_get("http"),
        phase: ObservationPhase::Verification,
        provenance: TermsProvenance::OriginResponse,
        transport: TermsTransport::Header,
        x402_version: Some(2),
        http_status: Some(402),
        content_hash: against,
        conflict: None,
        rejected: Default::default(),
        truncated: false,
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The one item a listing returns, as JSON.
async fn listed(registry: &DiscoveryRegistry) -> Value {
    let response = registry.list(10, 0, None).await;
    let json = serde_json::to_value(&response).expect("the listing must serialize");
    json["items"][0].clone()
}

#[tokio::test]
async fn an_unchecked_price_says_so_rather_than_looking_current() {
    let registry = DiscoveryRegistry::new();
    registry
        .register(resource("https://api.unchecked.example/x", "10000"))
        .await
        .unwrap();

    let item = listed(&registry).await;
    assert_eq!(item["priceFreshness"], "unknown");
    assert!(
        item.get("termsObservedAt").is_none(),
        "a date nobody produced is absent, not zero and not now"
    );
    assert!(item.get("observedTerms").is_none());
    assert_eq!(item["recordVersion"], RECORD_FORMAT_VERSION);
    // The declared listing is still served in full. "Unverified" is a statement
    // about our knowledge, not a reason to hide the offer.
    assert_eq!(item["accepts"][0]["amount"], "10000");
    assert_eq!(item["accepts"][0]["scheme"], "exact");
}

#[tokio::test]
async fn a_reading_that_agrees_is_reported_with_its_context_and_its_date() {
    let registry = DiscoveryRegistry::new();
    let url = "https://api.agreeing.example/x";
    let r = resource(url, "10000");
    let fingerprint = r.content_fingerprint();
    registry.register(r).await.unwrap();
    registry
        .terms()
        .record(url, observation("10000", now() - 600, Some(fingerprint)))
        .await;

    let item = listed(&registry).await;
    assert_eq!(item["priceFreshness"], "fresh");
    assert!(item["termsObservedAt"].as_u64().unwrap() > 0);

    // The context is part of the claim. "The price of this resource" is not a
    // well-formed question: this is the price for an unauthenticated GET, and a
    // parameterized POST is a different purchase.
    let observed = &item["observedTerms"];
    assert_eq!(observed["context"]["method"], "GET");
    assert_eq!(observed["context"]["authenticated"], false);
    assert_eq!(observed["phase"], "verification");
    assert_eq!(observed["provenance"], "origin_response");
    assert_eq!(observed["transport"], "header");
    assert_eq!(observed["accepts"][0]["amount"], "10000");
}

#[tokio::test]
async fn a_disagreement_is_published_as_a_disagreement_with_both_sides_intact() {
    let registry = DiscoveryRegistry::new();
    let url = "https://api.disagreeing.example/x";
    let r = resource(url, "10000");
    let fingerprint = r.content_fingerprint();
    registry.register(r).await.unwrap();
    // The origin charges three times what the listing says.
    registry
        .terms()
        .record(url, observation("30000", now() - 600, Some(fingerprint)))
        .await;

    let item = listed(&registry).await;
    assert_eq!(item["priceFreshness"], "conflict");
    assert_eq!(
        item["accepts"][0]["amount"], "10000",
        "the declared listing is not silently rewritten to match"
    );
    assert_eq!(
        item["observedTerms"]["accepts"][0]["amount"], "30000",
        "and the reading is not discarded for disagreeing"
    );
}

#[tokio::test]
async fn a_settlement_moves_its_own_date_and_only_its_own_date() {
    let registry = DiscoveryRegistry::new();
    let url = "https://api.paid.example/x";
    // An explicitly OLD write date, so "the settlement did not move it" cannot
    // pass by accident just because both happened in the same second.
    let mut listing = resource(url, "10000");
    listing.last_updated = 1_700_000_000;
    registry.update(listing).await.unwrap();
    let before = listed(&registry).await;
    assert_eq!(before["lastUpdated"], 1_700_000_000);

    let mut settled = resource(url, "10000");
    settled.accepts[0].amount = x402_rs::types::TokenAmount::from(999u64);
    registry.track_settlement(settled).await.unwrap();

    let after = listed(&registry).await;
    assert_eq!(
        after["lastUpdated"], before["lastUpdated"],
        "a payment is not a content update"
    );
    assert_eq!(after["lastUpdated"], 1_700_000_000);
    assert_eq!(
        after["accepts"][0]["amount"], "10000",
        "one settled amount is not the seller's price list"
    );
    assert_eq!(after["settlementCount"], 1);
    assert!(
        after["lastSettledAt"].as_u64().unwrap() > 0,
        "the activity is recorded, on the date it belongs to"
    );
    assert_eq!(
        after["priceFreshness"], "unknown",
        "and it verifies nothing about the price"
    );
}

#[tokio::test]
async fn a_revised_listing_marks_the_previous_reading_for_revalidation() {
    let registry = DiscoveryRegistry::new();
    let url = "https://api.revised.example/x";
    let original = resource(url, "10000");
    let fingerprint = original.content_fingerprint();
    registry.register(original).await.unwrap();
    registry
        .terms()
        .record(url, observation("10000", now() - 600, Some(fingerprint)))
        .await;
    assert_eq!(listed(&registry).await["priceFreshness"], "fresh");

    // The owner publishes new terms. The reading was of the previous revision,
    // so it is not evidence about this one -- and it is not deleted either.
    registry.update(resource(url, "20000")).await.unwrap();
    let item = listed(&registry).await;
    assert_eq!(
        item["priceFreshness"], "stale",
        "a new revision makes the previous reading obsolete, not wrong"
    );
    assert_eq!(item["observedTerms"]["accepts"][0]["amount"], "10000");
}

#[tokio::test]
async fn the_content_hash_changes_with_the_offer_and_with_nothing_else() {
    let a = resource("https://api.hash.example/x", "10000");
    let b = resource("https://api.hash.example/x", "10000");
    assert_eq!(
        a.content_fingerprint(),
        b.content_fingerprint(),
        "two records of the same offer are the same offer"
    );

    // Every field that describes OUR relationship with the record, rather than
    // the offer, must leave the fingerprint alone -- otherwise an unchanged feed
    // hashes differently on every cycle and the whole check is worthless.
    let mut noisy = resource("https://api.hash.example/x", "10000");
    noisy.last_updated += 9_999;
    noisy.source_updated_at = Some(1);
    noisy.first_seen = Some(2);
    noisy.last_settled_at = Some(3);
    noisy.settlement_count = Some(17);
    noisy.record_version = 1;
    assert_eq!(a.content_fingerprint(), noisy.content_fingerprint());

    let dearer = resource("https://api.hash.example/x", "10001");
    assert_ne!(a.content_fingerprint(), dearer.content_fingerprint());
}
