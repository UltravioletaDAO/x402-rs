//! Round-trip fixtures for what the Bazaar says a resource costs.
//!
//! # What these pin
//!
//! Importing a catalog used to lose three things, and every one of them changed
//! what a listing *claims about money*:
//!
//! 1. **The scheme.** Every imported option was written as `exact`, whatever the
//!    source published. `exact` means "this is the amount"; `upto` means "this is
//!    a ceiling you authorise and the charge is settled separately". Relabelling
//!    the second as the first is a false statement about someone else's terms.
//! 2. **`extra`.** Dropped wholesale. For `exact` that is the EIP-712 domain a
//!    payer needs; for every other scheme it is the parameters without which the
//!    offer cannot be interpreted at all.
//! 3. **The amount, when it could not be parsed.** It became `0`, and a catalog
//!    entry that says zero says *free*.
//!
//! # Where the fixtures come from
//!
//! `cdp-pricing-page.json` is four entries captured from the live Coinbase CDP
//! discovery feed on 2026-09-10, trimmed (the JSON Schema bodies inside
//! `extensions` and one 700-byte JWT are elided; nothing about price is). On the
//! full page they came from, 27 of 178 payment options carried a scheme that is
//! not `exact`, 178 of 178 carried an `extra`, and 3 of 178 declared a *decimal*
//! amount in a field that holds atomic units.
//!
//! `v1-pricing-page.json` is the x402 v1 spelling of the same problems -- v1
//! network names, `maxAmountRequired` instead of `amount` -- plus an `upto`
//! listing, which no public feed publishes yet and which is the case the old
//! converter was most wrong about.

use std::collections::HashMap;

use x402_rs::discovery_aggregator::{convert_resources, CoinbaseDiscoveryResponse};
use x402_rs::discovery_price::{
    normalize_declared_option, parse_atomic_amount, AmountReject, CatalogScheme,
    DeclaredPaymentOption, OptionReject,
};
use x402_rs::types::{Scheme, TokenAmount};
use x402_rs::types_v2::DiscoveryResource;

const CDP_PAGE: &str = include_str!("fixtures/bazaar/cdp-pricing-page.json");
const V1_PAGE: &str = include_str!("fixtures/bazaar/v1-pricing-page.json");

/// Import a fixture page exactly as the aggregator would.
fn import(page: &str, source: &str) -> (Vec<DiscoveryResource>, HashMap<&'static str, usize>) {
    let parsed: CoinbaseDiscoveryResponse =
        serde_json::from_str(page).expect("fixture must parse as a discovery page");
    convert_resources(parsed.items, source)
}

fn by_url<'a>(items: &'a [DiscoveryResource], needle: &str) -> &'a DiscoveryResource {
    items
        .iter()
        .find(|r| r.url.as_str().contains(needle))
        .unwrap_or_else(|| panic!("fixture resource {needle} not imported"))
}

// ============================================================================
// Scheme is preserved, in both protocol spellings
// ============================================================================

#[test]
fn v2_unknown_scheme_survives_import_and_is_never_exact() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    let r = by_url(&items, "onesource.io");

    let schemes: Vec<String> = r.accepts.iter().map(|a| a.scheme.to_string()).collect();
    assert_eq!(
        schemes,
        vec!["exact", "batch-settlement"],
        "the source published two different schemes and both must survive"
    );
    assert!(
        !r.accepts
            .iter()
            .any(|a| a.scheme == CatalogScheme::Known(Scheme::Exact)
                && a.extra
                    .as_ref()
                    .and_then(|e| e.get("receiverAuthorizer"))
                    .is_some()),
        "the batch-settlement option must not be wearing exact's label"
    );
}

#[test]
fn v1_upto_is_not_flattened_to_exact() {
    let (items, _) = import(V1_PAGE, "fixture");
    let r = by_url(&items, "/upto");

    assert_eq!(r.accepts.len(), 1);
    let option = &r.accepts[0];
    assert_eq!(option.scheme, CatalogScheme::Known(Scheme::Upto));
    assert_ne!(
        option.scheme,
        CatalogScheme::Known(Scheme::Exact),
        "an upto ceiling published as an exact price misstates what the buyer pays"
    );
    // `maxAmountRequired` is the v1 spelling of `amount`. It carries the
    // ceiling for `upto` and the price for `exact`; it is not a range, and it
    // does not decide the scheme.
    assert_eq!(option.amount, TokenAmount::from(100_000u64));
    assert_eq!(
        option
            .extra
            .as_ref()
            .and_then(|e| e.get("pricePerUnit"))
            .and_then(|v| v.as_str()),
        Some("250"),
        "upto is uninterpretable without its rate and unit"
    );
}

#[test]
fn v1_unknown_scheme_survives_import_and_is_never_exact() {
    let (items, _) = import(V1_PAGE, "fixture");
    let r = by_url(&items, "/unknown-scheme");
    assert_eq!(
        r.accepts[0].scheme,
        CatalogScheme::Unsupported("batch-settlement".to_string())
    );
    assert!(r.accepts[0].scheme.as_known().is_none());
}

#[test]
fn unknown_scheme_is_catalogable_but_not_settleable() {
    let (items, _) = import(V1_PAGE, "fixture");
    let mut option = by_url(&items, "/unknown-scheme").accepts[0].clone();
    option.annotate();
    assert_eq!(option.settleable, Some(false));
    assert_eq!(option.unsupported_reason.as_deref(), Some("unknown-scheme"));

    let mut exact = by_url(&items, "/exact").accepts[0].clone();
    exact.annotate();
    assert_eq!(exact.settleable, Some(true));
    assert_eq!(exact.unsupported_reason, None);
}

// ============================================================================
// `extra` is preserved
// ============================================================================

#[test]
fn extra_survives_import_on_every_option() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    let r = by_url(&items, "onesource.io");
    for option in &r.accepts {
        let extra = option
            .extra
            .as_ref()
            .unwrap_or_else(|| panic!("{} lost its extra", option.scheme));
        assert_eq!(
            extra.get("name").and_then(|v| v.as_str()),
            Some("USD Coin"),
            "the EIP-712 domain a payer needs must reach the catalog"
        );
    }
    let batch = r
        .accepts
        .iter()
        .find(|a| a.scheme.to_string() == "batch-settlement")
        .expect("batch-settlement option");
    assert_eq!(
        batch
            .extra
            .as_ref()
            .and_then(|e| e.get("withdrawDelay"))
            .and_then(|v| v.as_u64()),
        Some(86_400),
        "a scheme's own parameters are the whole of its meaning"
    );
}

// ============================================================================
// A price we cannot read is never a price of zero
// ============================================================================

#[test]
fn invalid_amounts_are_rejected_by_cause_and_never_become_zero() {
    let (items, rejected) = import(V1_PAGE, "fixture");
    let r = by_url(&items, "/bad-amounts");

    // Four bad declarations in the fixture, one legitimate explicit zero.
    assert_eq!(
        r.accepts.len(),
        1,
        "only the explicit zero may survive; got {:?}",
        r.accepts.iter().map(|a| a.amount).collect::<Vec<_>>()
    );
    assert_eq!(r.accepts[0].amount, TokenAmount::from(0u64));

    assert_eq!(rejected.get("amount-not-an-integer"), Some(&1));
    assert_eq!(rejected.get("amount-negative"), Some(&1));
    assert_eq!(rejected.get("amount-missing"), Some(&1));
    assert_eq!(rejected.get("amount-overflow"), Some(&1));
}

#[test]
fn a_decimal_in_an_atomic_field_is_refused_not_truncated() {
    // Measured on the live CDP feed: three options declared "0.002", "0.011",
    // "0.016". `U256::from_str` fails on all three, and the old converter mapped
    // that failure to zero.
    for raw in ["0.002", "0.011", "0.016"] {
        assert_eq!(parse_atomic_amount(raw), Err(AmountReject::NotAnInteger));
    }
    assert_eq!(parse_atomic_amount("-1"), Err(AmountReject::Negative));
    assert_eq!(parse_atomic_amount(""), Err(AmountReject::Missing));
    assert_eq!(parse_atomic_amount("1e6"), Err(AmountReject::NotAnInteger));
    // Hex is a second spelling of the same number; two readers that disagree
    // about the spelling disagree about the price.
    assert_eq!(parse_atomic_amount("0x64"), Err(AmountReject::NotAnInteger));
    assert_eq!(
        parse_atomic_amount(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
        ),
        Err(AmountReject::Overflow)
    );

    // And the values that are real prices still parse, at full width.
    assert_eq!(parse_atomic_amount("0"), Ok(TokenAmount::from(0u64)));
    assert_eq!(
        parse_atomic_amount(" 10000 "),
        Ok(TokenAmount::from(10_000u64))
    );
    assert_eq!(
        parse_atomic_amount(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        )
        .expect("U256::MAX is a valid amount")
        .to_string(),
        "115792089237316195423570985008687907853269984665640564039457584007913129639935"
    );
}

#[test]
fn both_amount_spellings_on_one_option_are_one_number_not_a_duplicate() {
    // The live Coinbase page carries `amount` AND `maxAmountRequired` on 55 of
    // its 178 payment options, and at least one such option appeared on every
    // page sampled on 2026-09-10. As a `#[serde(alias)]` the two are the same
    // field, so `serde` called the document a duplicate and the WHOLE page was
    // dropped -- not the entry, the page.
    let (items, rejected) = import(CDP_PAGE, "coinbase");
    assert!(
        !rejected.contains_key("amount-conflicting"),
        "captured page: {rejected:?}"
    );
    let r = by_url(&items, "onesource.io");
    assert_eq!(r.accepts.len(), 2);
    for option in &r.accepts {
        assert_eq!(option.amount, TokenAmount::from(3_000u64));
    }
}

#[test]
fn two_amount_spellings_that_disagree_are_refused_not_averaged() {
    let declared = DeclaredPaymentOption {
        scheme: Some("exact".into()),
        network: Some("base".into()),
        asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
        amount: Some(serde_json::json!("10000")),
        max_amount_required: Some(serde_json::json!("30000")),
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: None,
    };
    assert_eq!(
        normalize_declared_option(declared),
        Err(OptionReject::Amount(AmountReject::Conflicting)),
        "one of them is wrong and nothing here knows which; a range would be invented"
    );
}

#[test]
fn an_explicit_zero_is_not_a_parse_failure() {
    let declared = DeclaredPaymentOption {
        scheme: Some("exact".into()),
        network: Some("base".into()),
        asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
        amount: Some(serde_json::json!("0")),
        max_amount_required: None,
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: None,
    };
    let option = normalize_declared_option(declared).expect("an explicit zero is a declaration");
    assert_eq!(option.amount, TokenAmount::from(0u64));

    // Whether a free listing is publishable stays a catalog policy question
    // (curation's `unpayable` rule), decided after parsing, not during it.
    let missing = DeclaredPaymentOption {
        scheme: Some("exact".into()),
        network: Some("base".into()),
        asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
        amount: None,
        max_amount_required: None,
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: None,
    };
    assert_eq!(
        normalize_declared_option(missing),
        Err(OptionReject::Amount(AmountReject::Missing))
    );
}

#[test]
fn a_missing_scheme_is_refused_rather_than_assumed() {
    let declared = DeclaredPaymentOption {
        scheme: None,
        network: Some("base".into()),
        asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
        amount: Some(serde_json::json!("10000")),
        max_amount_required: None,
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: None,
    };
    assert_eq!(
        normalize_declared_option(declared),
        Err(OptionReject::SchemeMissing),
        "defaulting an absent scheme to exact is the bug, in miniature"
    );
}

// ============================================================================
// Money is never a float
// ============================================================================

#[test]
fn amounts_above_the_js_safe_integer_survive_exactly() {
    // 2^53 is where a JSON `Number` stops being able to hold an integer. An 18
    // decimal asset reaches that at 9.007 tokens.
    let declared = DeclaredPaymentOption {
        scheme: Some("exact".into()),
        network: Some("bsc".into()),
        asset: Some("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d".into()),
        amount: Some(serde_json::json!("123456789012345678901")),
        max_amount_required: None,
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: None,
    };
    let option = normalize_declared_option(declared).expect("a large integer is a valid amount");
    assert_eq!(option.amount.to_string(), "123456789012345678901");

    let json = serde_json::to_string(&option).expect("serialize");
    assert!(
        json.contains("\"amount\":\"123456789012345678901\""),
        "amounts stay decimal strings on the wire: {json}"
    );
}

// ============================================================================
// Currency and decimals are per deployment, not per catalog
// ============================================================================

#[test]
fn decimals_are_resolved_per_asset_and_network() {
    let (items, _) = import(V1_PAGE, "fixture");
    let r = by_url(&items, "/decimals");
    assert_eq!(r.accepts.len(), 2);

    let mut base = r.accepts[0].clone();
    base.annotate();
    assert_eq!(base.asset_symbol.as_deref(), Some("USDC"));
    assert_eq!(base.asset_decimals, Some(6));

    let mut bsc = r.accepts[1].clone();
    bsc.annotate();
    assert_eq!(bsc.asset_symbol.as_deref(), Some("USDC"));
    assert_eq!(
        bsc.asset_decimals,
        Some(18),
        "USDC on BSC is 18 decimals; assuming 6 overstates it by 10^12"
    );

    // The two options carry the same money at different scales.
    assert_eq!(base.amount, TokenAmount::from(10_000u64));
    assert_eq!(bsc.amount, TokenAmount::from(10_000_000_000_000_000u64));
}

#[test]
fn an_unregistered_asset_reports_unknown_rather_than_guessing_dollars() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    let r = by_url(&items, "stableenrich.dev");
    let mut solana = r
        .accepts
        .iter()
        .find(|a| a.network.to_string().starts_with("solana:"))
        .expect("the Solana option must survive import")
        .clone();
    solana.annotate();
    assert_eq!(
        solana.asset_symbol.as_deref(),
        Some("USDC"),
        "the Solana USDC mint is a registered deployment"
    );

    let mut unregistered = solana.clone();
    unregistered.asset = serde_json::from_value(serde_json::json!(
        "So11111111111111111111111111111111111111112"
    ))
    .unwrap();
    unregistered.annotate();
    assert_eq!(unregistered.asset_symbol, None);
    assert_eq!(unregistered.asset_decimals, None);
}

// ============================================================================
// Every ingestion route reads the same JSON the same way
// ============================================================================

#[test]
fn register_and_aggregate_agree_on_the_same_option() {
    // The exact bytes an upstream feed publishes, handed instead to
    // `POST /discovery/register`'s body type.
    let body = serde_json::json!({
        "url": "https://api.example-v1.test/unknown-scheme",
        "type": "http",
        "description": "same option, other door",
        "accepts": [{
            "scheme": "batch-settlement",
            "network": "base",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "maxAmountRequired": "3000",
            "payTo": "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea",
            "maxTimeoutSeconds": 3600,
            "extra": { "name": "USD Coin", "version": "2", "withdrawDelay": 86400 }
        }]
    });
    let request: x402_rs::types_v2::RegisterResourceRequest =
        serde_json::from_value(body).expect("register must accept a scheme it cannot settle");
    let registered = request.into_resource();

    let (aggregated, _) = import(V1_PAGE, "fixture");
    let via_feed = by_url(&aggregated, "/unknown-scheme");

    assert_eq!(registered.accepts[0].scheme, via_feed.accepts[0].scheme);
    assert_eq!(registered.accepts[0].amount, via_feed.accepts[0].amount);
    assert_eq!(registered.accepts[0].network, via_feed.accepts[0].network);
    assert_eq!(
        registered.accepts[0]
            .extra
            .as_ref()
            .and_then(|e| e.get("withdrawDelay")),
        via_feed.accepts[0]
            .extra
            .as_ref()
            .and_then(|e| e.get("withdrawDelay")),
        "one route must not preserve what another erases"
    );
}

#[test]
fn a_registrant_cannot_assert_our_own_capabilities() {
    // The response-only fields are accepted on the wire so a client can
    // round-trip a listing we emitted. They must not be *storable*: whether this
    // facilitator can settle an option is our statement, not the registrant's.
    let body = serde_json::json!({
        "url": "https://api.example-v1.test/liar",
        "type": "http",
        "description": "",
        "accepts": [{
            "scheme": "batch-settlement",
            "network": "eip155:8453",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount": "10000",
            "payTo": "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea",
            "maxTimeoutSeconds": 300,
            "settleable": true,
            "assetSymbol": "USD",
            "assetDecimals": 2
        }]
    });
    let request: x402_rs::types_v2::RegisterResourceRequest =
        serde_json::from_value(body).expect("the fields are accepted on the wire");
    let stored = request.into_resource();
    assert_eq!(stored.accepts[0].settleable, None);
    assert_eq!(stored.accepts[0].asset_symbol, None);
    assert_eq!(stored.accepts[0].asset_decimals, None);

    // And the resolved answers overrule the attempt.
    let mut resolved = stored.accepts[0].clone();
    resolved.annotate();
    assert_eq!(resolved.settleable, Some(false));
    assert_eq!(resolved.asset_symbol.as_deref(), Some("USDC"));
    assert_eq!(resolved.asset_decimals, Some(6));
}

#[test]
fn register_refuses_an_amount_it_cannot_read() {
    let body = serde_json::json!({
        "url": "https://api.example-v1.test/bad",
        "type": "http",
        "description": "",
        "accepts": [{
            "scheme": "exact",
            "network": "eip155:8453",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount": "0.002",
            "payTo": "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea",
            "maxTimeoutSeconds": 300
        }]
    });
    let parsed: Result<x402_rs::types_v2::RegisterResourceRequest, _> =
        serde_json::from_value(body);
    assert!(
        parsed.is_err(),
        "a decimal in an atomic-units field is a 400, never a free listing"
    );
}

// ============================================================================
// JSON round trip
// ============================================================================

#[test]
fn a_catalog_record_round_trips_through_json_unchanged() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    for original in &items {
        let json = serde_json::to_string(original).expect("serialize");
        let back: DiscoveryResource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            back.accepts.len(),
            original.accepts.len(),
            "{} lost options on the way through JSON",
            original.url
        );
        for (a, b) in original.accepts.iter().zip(back.accepts.iter()) {
            assert_eq!(a.scheme, b.scheme);
            assert_eq!(a.amount, b.amount);
            assert_eq!(a.network, b.network);
            assert_eq!(a.extra, b.extra);
        }
        assert_eq!(back.source_updated_at, original.source_updated_at);
        assert_eq!(back.extensions, original.extensions);
    }
}

#[test]
fn a_known_scheme_serializes_to_the_string_it_always_did() {
    let (items, _) = import(V1_PAGE, "fixture");
    let json = serde_json::to_value(by_url(&items, "/exact")).unwrap();
    assert_eq!(json["accepts"][0]["scheme"], "exact");
    assert_eq!(json["accepts"][0]["amount"], "10000");
    assert_eq!(json["accepts"][0]["network"], "eip155:8453");
}

// ============================================================================
// Unknown dates stay unknown
// ============================================================================

#[test]
fn an_undated_feed_entry_is_not_stamped_with_now() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    let undated = by_url(&items, "example-nodate.test");
    assert_eq!(
        undated.source_updated_at, None,
        "the feed declared no date, so we have none"
    );

    let dated = by_url(&items, "onesource.io");
    assert_eq!(
        dated.source_updated_at,
        Some(1_789_045_870),
        "2026-09-10T13:11:10.809Z in Unix seconds"
    );
    assert_eq!(dated.last_updated, 1_789_045_870);
}

/// A verbose third party must not be able to fail a page of prices.
#[test]
fn a_deep_extensions_blob_is_kept_and_an_absurd_one_is_dropped() {
    // This is not hypothetical. The live Coinbase feed publishes a `bazaar`
    // extension whose JSON Schema nests 26 levels deep, and applying the payment
    // `extra` depth bound (16) to it rejected the ENTIRE 100-item page.
    fn page(depth: usize) -> String {
        let blob = (0..depth).fold(
            serde_json::json!("leaf"),
            |acc, _| serde_json::json!({ "n": acc }),
        );
        serde_json::to_string(&serde_json::json!({
            "items": [{
                "resource": "https://api.deep.test/x",
                "type": "http",
                "description": "",
                "lastUpdated": 1789000000u64,
                "extensions": { "bazaar": blob },
                "accepts": [{
                    "scheme": "exact",
                    "network": "eip155:8453",
                    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                    "amount": "10000",
                    "payTo": "0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea",
                    "maxTimeoutSeconds": 300
                }]
            }]
        }))
        .unwrap()
    }

    // A real JSON Schema's depth is carried, not refused.
    let (items, _) = import(&page(26), "deep");
    assert_eq!(items.len(), 1);
    assert!(
        items[0].extensions.is_some(),
        "26 levels is an ordinary schema"
    );

    // Past the catalog's own bound the blob is dropped -- and the page, and the
    // price on it, still import.
    let (items, _) = import(&page(70), "deep");
    assert_eq!(items.len(), 1, "the page must still import");
    assert_eq!(items[0].accepts.len(), 1, "the price must still be there");
    assert_eq!(
        items[0].extensions, None,
        "dropping decoration costs nothing that a price depends on"
    );
}

/// The same tolerance on a payment option's own `extra`.
#[test]
fn an_out_of_bounds_extra_is_dropped_not_fatal() {
    let deep = (0..40).fold(
        serde_json::json!("leaf"),
        |acc, _| serde_json::json!({ "n": acc }),
    );
    let declared = DeclaredPaymentOption {
        scheme: Some("exact".into()),
        network: Some("base".into()),
        asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
        amount: Some(serde_json::json!("10000")),
        max_amount_required: None,
        pay_to: Some("0x52E29e0d2Aa49bfBfC548C0A9F2196F4aa51f3ea".into()),
        max_timeout_seconds: Some(300),
        extra: serde_json::from_str(&serde_json::to_string(&deep).unwrap()).ok(),
    };
    let option = normalize_declared_option(declared).expect("the price survives");
    assert_eq!(option.amount, TokenAmount::from(10_000u64));
    assert_eq!(option.extra, None);
}

#[test]
fn resource_level_extensions_survive_import() {
    let (items, _) = import(CDP_PAGE, "coinbase");
    let r = by_url(&items, "onesource.io");
    let extensions = r
        .extensions
        .as_ref()
        .expect("the bazaar extension says what is being sold");
    assert!(extensions.get("bazaar").is_some());
}
