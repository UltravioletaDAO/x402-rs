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
    json_block_after(
        include_str!("../static/skill.md"),
        "## 3. `POST /verify`",
    )
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
    assert_eq!(v1_req.payment_payload.network, caip2_req.payment_payload.network);
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
        let parsed: Result<VerifyRequestEnvelope, _> =
            serde_json::from_str(&example_on(unknown));
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
