//! The phase-0 verdict, as assertions.
//!
//! Every test here reads the committed vectors and runs the real experiment
//! against the published `hiero-sdk 0.45.0`. A red test is a NO-GO for the
//! property it names; the handoff quotes this file, not a reading of the SDK.

use hedera_spike::{
    derive_ecdsa, derive_ed25519, load_vector, parse_variants, policy, run, vector_paths,
    vectors_dir, Result3, Vector,
};

fn all() -> Vec<(Vector, hedera_spike::Report)> {
    vector_paths(&vectors_dir())
        .expect("vectors directory")
        .into_iter()
        .map(|p| {
            let v = load_vector(&p).expect("vector parses");
            let r = run(&v).expect("experiment runs");
            (v, r)
        })
        .collect()
}

/// The five payloads a well-behaved payer produces.
const LEGITIMATE: &[&str] = &[
    "01-hbar-ed25519-official-client",
    "02-hbar-ed25519-multinode",
    "03-hts-usdc-ecdsa-multinode",
    "04-hbar-threshold-2of3",
    "05-hbar-keylist-all",
];

/// Lists the SDK refuses to decode at all.
const REFUSED_BY_SDK: &[&str] = &[
    "06-adversarial-second-variant-repointed",
    "07-adversarial-second-variant-stale-signature",
    "09-adversarial-second-variant-other-transaction-id",
    "10-adversarial-empty-pubkey-prefix",
];

/// Payloads the SDK decodes, verifies and happily co-signs, and that only a
/// facilitator-side policy rejects. This set is the reason phase 2 exists.
const ONLY_POLICY_REJECTS: &[&str] = &[
    "11-adversarial-is-approval-debit",
    "12-adversarial-nft-rider",
    "13-adversarial-allowance-hook",
    "14-adversarial-duplicate-node",
    "15-adversarial-fee-payer-debited",
];

#[test]
fn vectors_are_present() {
    let names: Vec<String> = all().into_iter().map(|(v, _)| v.name).collect();
    assert_eq!(names.len(), 15, "expected 15 vectors, found {names:?}");
}

/// Cross-SDK binary compatibility: the bodies and signature pairs the Rust
/// protobuf layer reads are, byte for byte, the ones the TypeScript SDK wrote.
#[test]
fn rust_reads_exactly_what_typescript_wrote() {
    for (v, r) in all() {
        assert!(r.cross_sdk_bodies_match, "{}: bodies or signatures differ", v.name);
    }
}

#[test]
fn published_crate_decodes_a_real_official_client_payload() {
    let (v, r) = all()
        .into_iter()
        .find(|(v, _)| v.name == "01-hbar-ed25519-official-client")
        .expect("vector 01");
    assert_eq!(r.sdk_decode, Result3::Pass, "{}: {:?}", v.name, r.sdk_decode_error);
    assert_eq!(r.downcast_transfer, Result3::Pass);
    assert_eq!(r.raw_variant_count, 7, "the official client froze over 7 nodes");
    assert_eq!(r.sdk_variant_count, Some(7), "and the SDK sees all 7");
}

/// The question the spike exists to answer.
#[test]
fn co_signing_leaves_every_body_byte_for_byte_intact() {
    for (v, r) in all() {
        if !LEGITIMATE.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(r.bodies_preserved_after_cosign, Result3::Pass, "{}", v.name);
        assert_eq!(r.prior_signatures_preserved, Result3::Pass, "{}", v.name);
        assert_eq!(r.cosignature_on_every_variant, Result3::Pass, "{} {:?}", v.name, r.notes);
        assert_eq!(r.cosignature_verifies, Result3::Pass, "{} {:?}", v.name, r.notes);
    }
}

/// Stronger than required, and worth pinning: decode and re-encode with no
/// signing reproduces the input exactly, so nothing is rebuilt from fields.
#[test]
fn decode_then_encode_without_signing_is_the_same_bytes() {
    for (v, r) in all() {
        if !LEGITIMATE.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(r.roundtrip_identical, Result3::Pass, "{} {:?}", v.name, r.notes);
    }
}

#[test]
fn payer_signatures_verify_for_ed25519_ecdsa_keylist_and_threshold() {
    for (v, r) in all() {
        if !LEGITIMATE.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(r.payer_signature, Result3::Pass, "{}: {:?}", v.name, r.payer_signature_error);
    }
}

/// A 2-of-3 threshold must pass with two signatures, and the absent third key
/// must not be demanded.
#[test]
fn threshold_passes_with_the_quorum_and_no_more() {
    let (v, r) = all()
        .into_iter()
        .find(|(v, _)| v.name == "04-hbar-threshold-2of3")
        .expect("vector 04");
    assert_eq!(r.payer_signature, Result3::Pass, "{}: {:?}", v.name, r.payer_signature_error);
    let bytes = hedera_spike::decode_base64(&v.payload.transaction).unwrap();
    for variant in parse_variants(&bytes).unwrap() {
        assert_eq!(variant.signatures.len(), 2, "quorum is two signatures, not three");
    }
}

#[test]
fn a_signature_from_an_unrelated_key_fails_verification() {
    let (_, r) = all()
        .into_iter()
        .find(|(v, _)| v.name == "08-adversarial-wrong-signer")
        .expect("vector 08");
    // It decodes: nothing is malformed about it.
    assert_eq!(r.sdk_decode, Result3::Pass);
    // It is verification, not decoding, that catches it.
    assert_eq!(r.payer_signature, Result3::Fail);
}

/// The SDK compares the per-node bodies itself and refuses lists that disagree.
#[test]
fn sdk_refuses_lists_whose_variants_disagree() {
    for (v, r) in all() {
        if !REFUSED_BY_SDK.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(r.sdk_decode, Result3::Fail, "{} decoded when it should not have", v.name);
        assert!(r.sdk_decode_error.is_some(), "{}", v.name);
    }
}

/// And the ones it does not refuse. Each of these is a valid Hedera transfer
/// that the SDK decodes, that the payer really signed, and that the SDK will
/// co-sign without a word -- and that we must never sponsor.
#[test]
fn sdk_co_signs_payloads_that_only_our_policy_rejects() {
    for (v, r) in all() {
        if !ONLY_POLICY_REJECTS.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(r.sdk_decode, Result3::Pass, "{}", v.name);
        assert_eq!(r.payer_signature, Result3::Pass, "{}", v.name);
        assert_eq!(
            r.cosignature_verifies,
            Result3::Pass,
            "{}: the SDK co-signed it",
            v.name
        );
        assert_eq!(
            r.facilitator_policy,
            Result3::Fail,
            "{}: only our own inspection stands between this and a sponsored signature",
            v.name
        );
    }
}

/// hiero-sdk 0.45.0 refuses the empty-prefix list on its own. That is a
/// property of this version, so the policy must reject it independently.
#[test]
fn policy_rejects_the_empty_prefix_list_without_help_from_the_sdk() {
    let (v, r) = all()
        .into_iter()
        .find(|(v, _)| v.name == "10-adversarial-empty-pubkey-prefix")
        .expect("vector 10");
    assert_eq!(r.sdk_decode, Result3::Fail, "{}: the SDK still refuses it", v.name);
    let bytes = hedera_spike::decode_base64(&v.payload.transaction).unwrap();
    let variants = parse_variants(&bytes).unwrap();
    let want = policy::PolicyInput {
        asset: v.payment_requirements.asset.clone(),
        amount: v.payment_requirements.amount.parse().unwrap(),
        pay_to: v.payment_requirements.pay_to.clone(),
        fee_payer: v.payment_requirements.extra.fee_payer.clone(),
        sender: v.expected.sender_account_id.clone(),
    };
    let err = policy::check(&variants, &want).expect_err("policy must reject it too");
    assert!(err.contains("empty public key prefix"), "unexpected reason: {err}");
}

#[test]
fn policy_accepts_every_legitimate_payload() {
    for (v, r) in all() {
        if !LEGITIMATE.contains(&v.name.as_str()) {
            continue;
        }
        assert_eq!(
            r.facilitator_policy,
            Result3::Pass,
            "{}: {:?}",
            v.name,
            r.facilitator_policy_error
        );
    }
}

/// The SDK's aggregated getters return `HashMap<AccountId, Hbar>` and
/// `HashMap<TokenId, HashMap<AccountId, i64>>`; `isApproval` and hook calls
/// have nowhere to live in either. Read from protobuf or do not read at all.
#[test]
fn aggregated_getters_cannot_express_is_approval_or_hooks() {
    for (v, r) in all() {
        match v.name.as_str() {
            "11-adversarial-is-approval-debit" | "13-adversarial-allowance-hook" => {
                assert!(!r.sdk_getters_hide.is_empty(), "{}", v.name);
            }
            "12-adversarial-nft-rider" => {
                assert!(
                    r.sdk_getters_hide.iter().any(|s| s.contains("NFT")),
                    "{}",
                    v.name
                );
            }
            _ => {}
        }
    }
}

/// Both sides derive their keys from the same ASCII labels. If this drifts the
/// whole cross-SDK comparison is measuring two different things.
#[test]
fn both_sdks_derive_the_same_keys_from_the_same_labels() {
    let (v, _) = all()
        .into_iter()
        .find(|(v, _)| v.name == "02-hbar-ed25519-multinode")
        .expect("vector 02");
    let sender = v.keys.get("sender").expect("sender key record");
    let label = sender.get("label").unwrap().as_str().unwrap();
    let der = sender.get("publicKeyDer").unwrap().as_str().unwrap();
    let rust = derive_ed25519(label).unwrap().public_key().to_string_der();
    assert_eq!(rust, der, "ed25519 derivation differs between the SDKs");

    let (v3, _) = all()
        .into_iter()
        .find(|(v, _)| v.name == "03-hts-usdc-ecdsa-multinode")
        .expect("vector 03");
    let s3 = v3.keys.get("sender").unwrap();
    let label3 = s3.get("label").unwrap().as_str().unwrap();
    let der3 = s3.get("publicKeyDer").unwrap().as_str().unwrap();
    let rust3 = derive_ecdsa(label3).unwrap().public_key().to_string_der();
    assert_eq!(rust3, der3, "ecdsa derivation differs between the SDKs");
}

/// The policy has to be wrong in the obvious direction too: a payment that
/// matches its requirements exactly must survive it.
#[test]
fn policy_is_not_vacuously_strict() {
    let (v, _) = all()
        .into_iter()
        .find(|(v, _)| v.name == "03-hts-usdc-ecdsa-multinode")
        .expect("vector 03");
    let bytes = hedera_spike::decode_base64(&v.payload.transaction).unwrap();
    let variants = parse_variants(&bytes).unwrap();
    let want = policy::PolicyInput {
        asset: v.payment_requirements.asset.clone(),
        amount: v.payment_requirements.amount.parse().unwrap(),
        pay_to: v.payment_requirements.pay_to.clone(),
        fee_payer: v.payment_requirements.extra.fee_payer.clone(),
        sender: v.expected.sender_account_id.clone(),
    };
    policy::check(&variants, &want).expect("the honest payload passes");

    let mut wrong = want.clone();
    wrong.amount += 1;
    assert!(policy::check(&variants, &wrong).is_err(), "one unit off must fail");
}
