//! Conformance fixtures for the wire shape `POST /verify` and `POST /settle`
//! accept.
//!
//! # Why this file exists, and why it is not `crates/x402-compliance`
//!
//! `crates/x402-compliance` is sanctions screening (`checker.rs`, `lists/`,
//! `audit_logger.rs`), not protocol conformance, whatever `CLAUDE.md` calls it.
//! This is the protocol one, and it is deliberately small.
//!
//! # What it is seeded with
//!
//! Three requests measured against production `facilitator.ultravioletadao.xyz`
//! (version 2.10.0) on 2026-09-03, before any of this was fixed:
//!
//! | # | body | measured |
//! |---|---|---|
//! | 1 | the example published in `/skill.md` and `/docs`, verbatim | **400** |
//! | 2 | the v1 shape with `"network": "base"` | 200 |
//! | 3 | the SAME v1 shape with `"network": "eip155:8453"` | **400** |
//!
//! Fixture 1 is the expensive one: the `400` it produced carries a `hint` that
//! sends the integrator to `/skill.md` -- the document holding the body we had
//! just rejected. Fixture 3 is the one the ChatGPT/Paybox experiment hit.
//!
//! Two more measured against the same production build on 2026-09-04, walking
//! the ChatGPT -> Paybox -> MeshRelay flow by hand:
//!
//! | # | body | measured |
//! |---|---|---|
//! | 10 | the x402 **v2** envelope: `paymentPayload` + `resource` + `accepted` | **400** |
//! | 11 | the same body with `resource`/`accepted` ALSO repeated inside `paymentPayload` | `contract_call_failed` |
//!
//! Fixture 11 is not a success -- the signature is invented and the wallet is
//! empty -- but it is the facilitator arguing about the *payment* instead of
//! about the *shape*. The only thing separating the two rows is a duplicate of
//! data already in the request, and nothing published anywhere said so.
//!
//! Every fixture here parses a body and asserts nothing about the chain, so the
//! suite runs in CI with no facilitator, no RPC and no wallet.

use x402_rs::network::Network;
use x402_rs::types::PaymentRequirements;
use x402_rs::types_v2::{SettleRequestEnvelope, VerifyRequestEnvelope};

/// The first fenced ```json block after `heading`, taken out of the document we
/// actually publish.
///
/// Reading the shipped Markdown rather than a copy kept beside the assertion is
/// the whole point: the published example was broken for months precisely
/// because nothing read it.
fn json_block_after(markdown: &str, heading: &str) -> String {
    let after = markdown
        .split_once(heading)
        .unwrap_or_else(|| panic!("no heading `{heading}` in the document"))
        .1;
    let body = after
        .split_once("```json")
        .unwrap_or_else(|| panic!("no json block after `{heading}`"))
        .1;
    body.split_once("```")
        .expect("unterminated json block")
        .0
        .trim()
        .to_string()
}

/// The body `/skill.md` tells an agent to send.
fn published_example() -> String {
    json_block_after(include_str!("../static/skill.md"), "## 3. `POST /verify`")
}

/// The same v1 envelope with both `network` fields rewritten.
fn example_on(network: &str) -> String {
    published_example().replace("\"base\"", &format!("\"{network}\""))
}

// --- fixture 1 -----------------------------------------------------------

/// **The published example deserialises.** Measured `400` against production
/// on 2026-09-03; this is the fixture that must start red.
#[test]
fn the_example_published_in_skill_md_is_a_body_verify_accepts() {
    let example = published_example();
    let parsed: Result<VerifyRequestEnvelope, _> = serde_json::from_str(&example);
    assert!(
        parsed.is_ok(),
        "static/skill.md publishes a /verify example the facilitator rejects: {}",
        parsed.unwrap_err()
    );
}

// --- fixture 2: the control ----------------------------------------------

/// **The v1 spelling still works.** Green before and after -- it is here to
/// prove the other fixtures are measuring the network name and not something
/// else that changed with them.
#[test]
fn the_v1_network_name_is_accepted() {
    let envelope: VerifyRequestEnvelope =
        serde_json::from_str(&example_on("base")).expect("`base` must parse");
    assert_eq!(envelope.network_v1().unwrap(), Network::Base);
}

// --- fixture 3: the ChatGPT/Paybox failure -------------------------------

/// **The CAIP-2 spelling in the v1 shape works.** This is failure 3.1 of the
/// ChatGPT handoff, and the exact body that produced `400 data did not match
/// any variant of untagged enum VerifyRequestEnvelope` in production.
#[test]
fn a_caip2_network_name_in_the_v1_shape_is_accepted() {
    let envelope: VerifyRequestEnvelope = serde_json::from_str(&example_on("eip155:8453"))
        .expect("`eip155:8453` must parse in the v1 shape");
    assert_eq!(envelope.network_v1().unwrap(), Network::Base);
}

// --- fixture 4 -----------------------------------------------------------

/// The two spellings are the same chain, so they must produce the same verdict
/// path. Anything else would make the alias a fork.
#[test]
fn both_spellings_resolve_to_the_same_network() {
    let v1: VerifyRequestEnvelope = serde_json::from_str(&example_on("base")).unwrap();
    let caip2: VerifyRequestEnvelope = serde_json::from_str(&example_on("eip155:8453")).unwrap();
    assert_eq!(v1.network_v1().unwrap(), caip2.network_v1().unwrap());

    let v1_req = v1.to_v1().unwrap();
    let caip2_req = caip2.to_v1().unwrap();
    assert_eq!(
        v1_req.payment_requirements.network,
        caip2_req.payment_requirements.network
    );
    assert_eq!(
        v1_req.payment_payload.network,
        caip2_req.payment_payload.network
    );
}

// --- fixture 5 -----------------------------------------------------------

/// A body may mix the spellings. It happens for real: the payer's own wallet
/// writes the payload while the merchant's 402 -- or our Bazaar -- writes the
/// requirements, and the two need not agree on notation.
#[test]
fn a_body_may_mix_the_two_spellings() {
    let mixed = published_example().replacen("\"base\"", "\"eip155:8453\"", 1);
    assert!(
        mixed.contains("\"eip155:8453\"") && mixed.contains("\"base\""),
        "the fixture must actually contain one of each spelling"
    );
    let envelope: VerifyRequestEnvelope =
        serde_json::from_str(&mixed).expect("a mixed-spelling body must parse");
    assert_eq!(envelope.network_v1().unwrap(), Network::Base);
}

// --- fixture 6 -----------------------------------------------------------

/// `/settle` shares the envelope with `/verify` (`pub type
/// SettleRequestEnvelope = VerifyRequestEnvelope`), so the failure was never
/// `/verify`'s alone and neither is the fix. Asserted rather than assumed: the
/// alias is one `pub type` away from being quietly split.
#[test]
fn settle_accepts_exactly_what_verify_accepts() {
    for network in ["base", "eip155:8453"] {
        let body = example_on(network);
        let verify: Result<VerifyRequestEnvelope, _> = serde_json::from_str(&body);
        let settle: Result<SettleRequestEnvelope, _> = serde_json::from_str(&body);
        assert_eq!(
            verify.is_ok(),
            settle.is_ok(),
            "/verify and /settle disagree about `{network}`"
        );
        assert!(settle.is_ok(), "/settle must accept `{network}`");
    }
}

// --- fixture 7: the guard ------------------------------------------------

/// Widening the field must not empty it. A chain we do not serve is still a
/// hard parse error, not a network silently resolved to something else.
#[test]
fn an_unknown_network_is_still_rejected() {
    for unknown in ["cosmos:hub-4", "eip155:99999999", "not-a-network", ""] {
        let parsed: Result<VerifyRequestEnvelope, _> = serde_json::from_str(&example_on(unknown));
        assert!(
            parsed.is_err(),
            "`{unknown}` is not a network this facilitator serves and must be refused"
        );
    }
}

// --- fixture 8: our own catalog ------------------------------------------

/// An offer taken out of OUR OWN Bazaar becomes a body `/verify` accepts.
///
/// Recorded from `GET /discovery/resources` on 2026-09-03: the catalog
/// announces `x402Version: 2` and every `accepts` entry in it names its chain
/// in CAIP-2. Before this change, discovery and settlement did not speak the
/// same language inside one house -- the catalog produced exactly the input
/// `/verify` rejected.
///
/// Note what the fixture has to add: `resource`, `description`, `mimeType`,
/// `maxTimeoutSeconds`, and `amount` renamed to `maxAmountRequired`. A Bazaar
/// entry is a compact OFFER, not a `PaymentRequirements`; the network name was
/// the half that could not be fixed by the caller, and it is the half fixed
/// here.
#[test]
fn a_caip2_offer_from_our_own_bazaar_becomes_payable() {
    // Verbatim from https://facilitator.ultravioletadao.xyz/discovery/resources
    let offer: serde_json::Value = serde_json::json!({
        "scheme": "exact",
        "network": "eip155:8453",
        "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        "amount": "1000000",
        "payTo": "0x80238a1C73367591BF17e2f4DBAc652e479b077A",
        "maxTimeoutSeconds": 60
    });

    let requirements = serde_json::json!({
        "scheme": offer["scheme"],
        "network": offer["network"],
        "maxAmountRequired": offer["amount"],
        "resource": "https://mcp.402milly.xyz/mcp",
        "description": "402milly MCP server",
        "mimeType": "application/json",
        "payTo": offer["payTo"],
        "maxTimeoutSeconds": offer["maxTimeoutSeconds"],
        "asset": offer["asset"],
    });

    let parsed: PaymentRequirements = serde_json::from_value(requirements)
        .expect("a CAIP-2 offer from our own catalog must be payable");
    assert_eq!(parsed.network, Network::Base);
}

// --- fixture 9: the regression guard -------------------------------------

/// Every v1 name that parsed before still parses, and its CAIP-2 twin resolves
/// to the same chain. The v1 spelling is tried first through the derived impl
/// precisely so this can only add names, never move one.
#[test]
fn every_v1_name_keeps_its_meaning_and_gains_its_twin() {
    for (v1_name, caip2) in [
        ("base", "eip155:8453"),
        ("base-sepolia", "eip155:84532"),
        ("avalanche-fuji", "eip155:43113"),
        ("polygon", "eip155:137"),
        ("solana", "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"),
    ] {
        let from_v1: Network =
            serde_json::from_value(serde_json::Value::String(v1_name.to_string()))
                .unwrap_or_else(|e| panic!("`{v1_name}` must still parse: {e}"));
        assert_eq!(
            from_v1,
            Network::from_caip2(caip2)
                .unwrap_or_else(|| panic!("`{caip2}` must be a known CAIP-2 id")),
            "`{v1_name}` and `{caip2}` must denote the same chain"
        );
    }
}

// --- fixture 10: the x402 v2 envelope ------------------------------------

/// The x402 v2 example `/skill.md` publishes.
fn published_v2_example() -> String {
    json_block_after(
        include_str!("../static/skill.md"),
        "### The same payment in the x402 v2 shape",
    )
}

/// **The v2 example published in `/skill.md` is a body `/verify` accepts.**
///
/// Measured `400 data did not match any variant of untagged enum
/// VerifyRequestEnvelope` against production on 2026-09-04. Same fixture shape
/// as fixture 1, on the shape that was still broken after fixture 1 was fixed:
/// the document had no v2 example at all, so there was nothing to be wrong --
/// and nothing to be right either.
#[test]
fn the_v2_example_published_in_skill_md_is_a_body_verify_accepts() {
    let example = published_v2_example();
    let parsed: Result<VerifyRequestEnvelope, _> = serde_json::from_str(&example);
    assert!(
        parsed.is_ok(),
        "static/skill.md publishes a v2 /verify example the facilitator rejects: {}",
        parsed.unwrap_err()
    );
    assert_eq!(
        parsed.unwrap().network_v1().unwrap(),
        Network::Base,
        "the published v2 example must name a chain we serve"
    );
}

/// **The published v1 and v2 examples are the same payment.**
///
/// Both reduce to one internal `VerifyRequest`, field for field. This is what
/// lets `/skill.md` claim they are two spellings of one thing: if the two ever
/// stop meaning the same payment, the document is lying and this goes red.
#[test]
fn the_two_published_examples_reduce_to_the_same_request() {
    let v1: VerifyRequestEnvelope = serde_json::from_str(&published_example()).unwrap();
    let v2: VerifyRequestEnvelope = serde_json::from_str(&published_v2_example()).unwrap();

    let a = serde_json::to_value(v1.to_v1().unwrap()).unwrap();
    let b = serde_json::to_value(v2.to_v1().unwrap()).unwrap();
    assert_eq!(
        a, b,
        "the v1 and v2 examples in skill.md are not the same payment"
    );
}

// --- fixture 11: the duplication -----------------------------------------

/// The same v2 body with `resource`/`accepted` ALSO written inside
/// `paymentPayload` -- the only shape production accepted before this change.
fn published_v2_example_duplicated() -> String {
    let mut body: serde_json::Value = serde_json::from_str(&published_v2_example()).unwrap();
    let resource = body["resource"].clone();
    let accepted = body["accepted"].clone();
    body["paymentPayload"]["resource"] = resource;
    body["paymentPayload"]["accepted"] = accepted;
    serde_json::to_string(&body).unwrap()
}

/// **The duplicated envelope still parses.** The fix is additive: an integrator
/// already sending the inner copy -- and some are, because it was the only shape
/// that worked -- must not be broken by making it optional.
#[test]
fn the_duplicated_v2_envelope_is_still_accepted() {
    let parsed: Result<VerifyRequestEnvelope, _> =
        serde_json::from_str(&published_v2_example_duplicated());
    assert!(
        parsed.is_ok(),
        "the duplicated v2 envelope must keep working: {}",
        parsed.unwrap_err()
    );
}

/// **Duplicated or not, it is the same request.**
///
/// The lean shape is defined as the duplicated one with the inner pair filled
/// in from the outer, so there is no second conversion path. Asserted rather
/// than trusted: `to_full()` is one careless edit away from dropping a field.
#[test]
fn the_inner_copy_changes_nothing_when_it_agrees() {
    let lean: VerifyRequestEnvelope = serde_json::from_str(&published_v2_example()).unwrap();
    let full: VerifyRequestEnvelope =
        serde_json::from_str(&published_v2_example_duplicated()).unwrap();

    assert!(
        matches!(lean, VerifyRequestEnvelope::V2Lean(_)),
        "a body with one copy of resource/accepted must take the lean variant"
    );
    assert!(
        matches!(full, VerifyRequestEnvelope::V2(_)),
        "a body with the inner copy must still take the standard v2 variant"
    );

    assert_eq!(
        serde_json::to_value(lean.to_v1().unwrap()).unwrap(),
        serde_json::to_value(full.to_v1().unwrap()).unwrap(),
        "the inner copy of resource/accepted must not change the payment"
    );
}

/// `/settle` takes the v2 envelope on the same terms, in both spellings of it.
/// The two endpoints share one `pub type`; nothing else guarantees they agree.
#[test]
fn settle_accepts_the_v2_envelope_too() {
    for (label, body) in [
        ("lean", published_v2_example()),
        ("duplicated", published_v2_example_duplicated()),
    ] {
        let verify: Result<VerifyRequestEnvelope, _> = serde_json::from_str(&body);
        let settle: Result<SettleRequestEnvelope, _> = serde_json::from_str(&body);
        assert_eq!(
            verify.is_ok(),
            settle.is_ok(),
            "/verify and /settle disagree about the {label} v2 envelope"
        );
        assert!(
            settle.is_ok(),
            "/settle must accept the {label} v2 envelope"
        );
    }
}

// --- fixture 12: the guard on the new variant ----------------------------

/// Widening the envelope must not empty it. The lean variant is tried LAST, so
/// a body that is merely broken still fails to parse rather than being read as
/// a v2 request with fields invented for it.
#[test]
fn the_lean_variant_does_not_swallow_broken_bodies() {
    let base: serde_json::Value = serde_json::from_str(&published_v2_example()).unwrap();

    for (what, mutate) in [
        ("no accepted", "accepted"),
        ("no resource", "resource"),
        ("no paymentPayload", "paymentPayload"),
    ] {
        let mut body = base.clone();
        body.as_object_mut().unwrap().remove(mutate);
        let parsed: Result<VerifyRequestEnvelope, _> = serde_json::from_value(body);
        assert!(
            parsed.is_err(),
            "a v2 body with {what} must still be refused, not read as something else"
        );
    }

    // A chain we do not serve never becomes a payment in the v2 shape either.
    //
    // Where it is refused differs, and the difference is worth writing down.
    // `accepted.network` is a `Caip2NetworkId`, which validates the SYNTAX
    // `namespace:reference` and not the chain. So `"base"` -- the bare x402 v1
    // name -- is a hard parse error here, while `"eip155:99999999"` parses
    // cleanly as a CAIP-2 id and is only refused later, when it resolves to no
    // `Network`. Both are refusals; neither is a payment.
    //
    // That asymmetry is the one place the two spellings are NOT
    // interchangeable, and the hint used to tell every caller they were.
    // Measured against production 2.10.0 on 2026-09-04: a v2 body with
    // `"base"` in `accepted.network` is a 400.
    for refused in [
        "cosmos:hub-4",
        "eip155:99999999",
        "base",
        "not-a-network",
        "",
    ] {
        let mut body = base.clone();
        body["accepted"]["network"] = serde_json::Value::String(refused.to_string());
        let parsed: Result<VerifyRequestEnvelope, _> = serde_json::from_value(body);
        match parsed {
            Err(_) => {} // refused at the syntax gate
            Ok(env) => assert!(
                env.network_v1().is_err() && env.to_v1().is_err(),
                "`{refused}` parsed AND resolved to a chain in the v2 shape"
            ),
        }
    }

    // The discriminating half: the loop above would also pass if the v2 shape
    // refused everything. The chain we do serve must still get through.
    let good: VerifyRequestEnvelope = serde_json::from_value(base).unwrap();
    assert_eq!(good.network_v1().unwrap(), Network::Base);
}
