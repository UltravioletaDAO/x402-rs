use super::*;
use hiero_sdk_proto::services as pb;
use prost::Message;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn fixtures() -> Vec<(String, Value)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hedera-e2e/vectors");
    let mut paths: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json") && p.file_name().unwrap() != "index.json"
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap(),
            )
        })
        .collect()
}
fn decode(v: &Value) -> Result<Decoded> {
    Decoded::from_base64(v["payload"]["transaction"].as_str().unwrap())
}
fn inspect(d: &Decoded, v: &Value) -> Result<Intent> {
    let r = &v["paymentRequirements"];
    let nodes: BTreeSet<_> = (3..=50)
        .map(|n| format!("0.0.{n}").parse().unwrap())
        .collect();
    let asset = r["asset"].as_str().unwrap().parse().unwrap();
    let to = r["payTo"].as_str().unwrap().parse().unwrap();
    let fee_payer = r["extra"]["feePayer"].as_str().unwrap().parse().unwrap();
    d.inspect(
        &Policy {
            network: "hedera:testnet",
            fee_payer: &fee_payer,
            pay_to: &to,
            asset: &asset,
            amount: r["amount"].as_str().unwrap().parse().unwrap(),
            max_fee: 100_000_000,
            max_duration: 180,
            allowed_nodes: &nodes,
        },
        0,
        false,
    )
}
fn fixture_key(v: &Value) -> pb::Key {
    let key = v["keys"]
        .get("senderAccountKey")
        .unwrap_or(&v["keys"]["sender"]);
    if let Some(encoded) = key["protobufHex"].as_str() {
        return pb::Key::decode(hex::decode(encoded).unwrap().as_slice()).unwrap();
    }
    let public = hex::decode(key["publicKeyRaw"].as_str().unwrap()).unwrap();
    pb::Key {
        key: Some(if public.len() == 32 {
            pb::key::Key::Ed25519(public)
        } else {
            pb::key::Key::EcdsaSecp256k1(public)
        }),
    }
}

#[test]
fn official_valid_vectors_verify_and_cosign_without_changing_authorizations() {
    let seed: [u8; 32] = Sha256::digest(b"x402-rs/hedera-spike/v1/ed25519/facilitator").into();
    let sponsor = hiero_sdk::PrivateKey::from_bytes_ed25519(&seed).unwrap();
    for (name, v) in fixtures().into_iter().take(5) {
        let d = decode(&v).unwrap_or_else(|e| panic!("{name}: {e}"));
        let intent = inspect(&d, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            intent.payer.to_string(),
            v["expected"]["senderAccountId"].as_str().unwrap()
        );
        d.verify_key(&fixture_key(&v))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        d.cosign(&sponsor).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}
#[test]
fn every_adversarial_official_vector_is_rejected_before_sponsorship() {
    for (name, v) in fixtures().into_iter().skip(5) {
        let result = decode(&v).and_then(|d| {
            inspect(&d, &v)?;
            d.verify_key(&fixture_key(&v))
        });
        assert!(result.is_err(), "adversarial vector accepted: {name}");
    }
}
#[test]
fn network_and_fee_policy_are_bound_before_signing() {
    let (_, v) = fixtures().remove(1);
    let d = decode(&v).unwrap();
    let intent = inspect(&d, &v).unwrap();
    let nodes: BTreeSet<_> = d
        .variants
        .iter()
        .map(|v| codec::account(v.body.node_account_id.as_ref()).unwrap())
        .collect();
    let fee: EntityId = "0.0.3003".parse().unwrap();
    let mut p = Policy {
        network: "hedera:testnet",
        fee_payer: &fee,
        pay_to: &intent.pay_to,
        asset: &intent.asset,
        amount: intent.amount,
        max_fee: intent.fee,
        max_duration: 180,
        allowed_nodes: &nodes,
    };
    assert!(d.inspect(&p, intent.expires_at, true).is_err());
    p.max_fee = intent.fee - 1;
    assert!(d.inspect(&p, 0, false).is_err());
    p.max_fee = intent.fee;
    p.network = "hedera:mainnet";
    assert_ne!(
        d.inspect(&p, 0, false).unwrap().fingerprint,
        intent.fingerprint
    );
}
fn envelope() -> Value {
    let (_, fixture) = fixtures().remove(0);
    let requirements = json!({"scheme":"exact", "network":"hedera:testnet", "asset":"0.0.0", "amount":"1000000", "payTo":"0.0.2002", "maxTimeoutSeconds":180,"extra":{"feePayer":"0.0.3003"}});
    json!({"x402Version":2,"paymentPayload":{"x402Version":2,"accepted":requirements,"payload":fixture["payload"]},"paymentRequirements":requirements})
}

struct ReadOnlyPolicyStore(Option<Record>);
#[async_trait::async_trait]
impl Store for ReadOnlyPolicyStore {
    async fn read(&self, _: &str) -> Result<Option<Record>> { Ok(self.0.clone()) }
    async fn reserve(&self, _: &str, _: &str, _: &Intent, _: &str, _: u64) -> Result<Record> {
        panic!("a retired payment asset must not reserve sponsor fees")
    }
    async fn save(&self, _: &str, _: &Record) -> Result<()> { panic!("must not write") }
    async fn pending(&self, _: &str) -> Result<Vec<(String, Record)>> { Ok(vec![]) }
    async fn health(&self) -> Result<()> { Ok(()) }
}

#[tokio::test]
async fn usdc_only_policy_rejects_new_hbar_but_preserves_historical_receipts() {
    let network = Network::HederaTestnet;
    let config = Config {
        network,
        account: "0.0.3003".parse().unwrap(),
        key: hiero_sdk::PrivateKey::from_bytes_ed25519(&[7; 32]).unwrap(),
        mirror: "https://testnet.mirrornode.hedera.com/".parse().unwrap(),
        assets: Config::payment_assets(network, None).unwrap(),
        max_fee: 100_000_000,
        daily_budget: 1_000_000_000,
        settle_timeout: Duration::from_secs(5),
        table: "unused".into(),
        admissions: true,
    };
    let mut provider = HederaProvider {
        client: config.client(),
        mirror: Mirror::new(config.mirror.clone()).unwrap(),
        config,
        store: Arc::new(ReadOnlyPolicyStore(None)),
    };
    let request = serde_json::from_value::<crate::types_v2::VerifyRequestEnvelope>(envelope())
        .unwrap().to_v1().unwrap();
    assert!(matches!(provider.verify(&request).await.unwrap_err(), FacilitatorLocalError::UnsupportedAsset(_, Network::HederaTestnet, _)));
    assert!(matches!(provider.settle(&request).await.unwrap_err(), FacilitatorLocalError::UnsupportedAsset(_, Network::HederaTestnet, _)));
    let supported = provider.supported().await.unwrap();
    let tokens = supported.kinds[0].extra.as_ref().unwrap().tokens.as_ref().unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].address.to_string(), "0.0.429274");
    assert_eq!(tokens[0].decimals, 6);

    // A settle of an already-confirmed HBAR payment finds its record, not the
    // asset policy, and is not a new payment: `transaction_already_submitted`,
    // without reserving, co-signing or broadcasting another transaction. The
    // portable receipt still recovers the confirmed result from the record.
    let (_, intent) = provider.inspect(&request, false).unwrap();
    let transaction_id = intent.transaction_id.clone();
    provider.store = Arc::new(ReadOnlyPolicyStore(Some(Record {
        intent, owner: "historical".into(), lease_until: 0,
        state: State::Confirmed, signed: None, consensus_status: Some("SUCCESS".into()),
    })));
    let response = provider.settle(&request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(response.payer.to_string(), "0.0.1001");
    assert_eq!(provider.receipt_evidence(&transaction_id).await, Some(true));
}

#[test]
fn both_ledgers_allow_only_native_usdc_and_refuse_old_token_overrides() {
    for (network, usdc) in [(Network::Hedera, "0.0.456858"), (Network::HederaTestnet, "0.0.429274")] {
        let assets = Config::payment_assets(network, None).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets.get(&usdc.parse().unwrap()), Some(&6));
        for extra in ["0.0.0:8", "0.0.1234:4", "0.0.456858:6"] {
            assert!(Config::payment_assets(network, Some(extra)).is_err());
        }
    }
}
#[test]
fn standard_http_envelope_uses_native_types_and_retains_v2() {
    let parsed: crate::types_v2::VerifyRequestEnvelope =
        serde_json::from_value(envelope()).unwrap();
    let request = parsed.to_v1().unwrap();
    assert_eq!(request.x402_version, X402Version::V2);
    assert!(matches!(
        request.payment_payload.payload,
        ExactPaymentPayload::Hedera(_)
    ));
    assert!(matches!(
        request.payment_requirements.pay_to,
        MixedAddress::Hedera(_)
    ));
    // The global heuristic must retain the historical NEAR interpretation.
    assert!(matches!(
        serde_json::from_value::<MixedAddress>(json!("0.0.2002")).unwrap(),
        MixedAddress::Near(_)
    ));
}
#[test]
fn mismatched_requirements_versions_and_extensions_cannot_normalize_as_hedera() {
    for (pointer, value) in [
        ("/paymentRequirements/amount", json!("2")),
        ("/paymentPayload/x402Version", json!(1)),
        ("/paymentRequirements/extra/feePayer", json!("0.0.3004")),
        ("/paymentRequirements/network", json!("hedera:mainnet")),
    ] {
        let mut raw = envelope();
        *raw.pointer_mut(pointer).unwrap() = value;
        assert!(
            serde_json::from_value::<wire::HederaRequest>(raw).is_err(),
            "{pointer}"
        );
    }
    let mut raw = envelope();
    raw["paymentPayload"]["extensions"] = json!({"8004-reputation":{}});
    assert!(serde_json::from_value::<wire::HederaRequest>(raw).is_err());
}
#[test]
fn excessive_payload_and_unknown_body_fields_are_rejected() {
    assert!(Decoded::from_base64(&"A".repeat(codec::MAX_BYTES * 2)).is_err());
    let (_, v) = fixtures().remove(1);
    let d = decode(&v).unwrap();
    let mut list = hiero_sdk_proto::sdk::TransactionList::decode(d.bytes.as_slice()).unwrap();
    let mut signed =
        pb::SignedTransaction::decode(list.transaction_list[0].signed_transaction_bytes.as_slice())
            .unwrap();
    // Unknown tag 127 varint value 1: prost normally drops it on decoding.
    signed.body_bytes.extend_from_slice(&[0xf8, 0x07, 0x01]);
    list.transaction_list[0].signed_transaction_bytes = signed.encode_to_vec();
    assert!(Decoded::from_bytes(list.encode_to_vec()).is_err());
}

#[test]
fn native_settlement_wire_uses_caip2_and_actual_sender() {
    let response = SettleResponse {
        success: true,
        error_reason: None,
        payer: MixedAddress::Hedera("0.0.1001".parse().unwrap()),
        transaction: Some(TransactionHash::Hedera(
            "0.0.3003@1789602861.163262387".into(),
        )),
        network: Network::HederaTestnet,
        proof_of_payment: None,
        extensions: None,
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["network"], "hedera:testnet");
    assert_eq!(json["payer"], "0.0.1001");
    assert_eq!(json["transaction"], "0.0.3003@1789602861.163262387");
    assert_eq!(
        serde_json::to_value(Network::Hedera).unwrap(),
        "hedera:mainnet"
    );
}

#[test]
fn hidden_extensions_and_duplicate_outer_requirements_are_rejected() {
    let mut raw = envelope();
    raw["paymentRequirements"]["extensions"] = json!({"refund":{}});
    assert!(serde_json::from_value::<wire::HederaRequest>(raw).is_err());
    let mut raw = envelope();
    raw["accepted"] = raw["paymentRequirements"].clone();
    raw["accepted"]["amount"] = json!("2");
    assert!(serde_json::from_value::<wire::HederaRequest>(raw).is_err());
}

/// This injects failure at the durable terminal-write boundary AFTER a real
/// consensus receipt, then reconstructs the provider and recovers the same ID.
/// It intentionally waits for the real lease to expire: production timings and
/// atomic claims are exercised without weakening them for the test.
#[tokio::test]
#[ignore = "requires explicit testnet signer, AWS credentials and fresh official-client envelope"]
async fn live_receipt_persistence_failure_recovers_original_payment() {
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct FailTerminalOnce {
        inner: Arc<dyn Store>,
        fail: AtomicBool,
    }
    #[async_trait]
    impl Store for FailTerminalOnce {
        async fn read(&self, key: &str) -> Result<Option<Record>> {
            self.inner.read(key).await
        }
        async fn reserve(
            &self,
            key: &str,
            network: &str,
            intent: &Intent,
            owner: &str,
            budget: u64,
        ) -> Result<Record> {
            self.inner
                .reserve(key, network, intent, owner, budget)
                .await
        }
        async fn save(&self, key: &str, record: &Record) -> Result<()> {
            if record.terminal() && self.fail.swap(false, Ordering::SeqCst) {
                return Err("injected terminal-write outage".into());
            }
            self.inner.save(key, record).await
        }
        async fn health(&self) -> Result<()> {
            self.inner.health().await
        }
        async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>> {
            self.inner.pending(network).await
        }
    }
    assert_eq!(std::env::var("HEDERA_LIVE_TEST").as_deref(), Ok("testnet"));
    let request: wire::HederaRequest =
        serde_json::from_str(&std::env::var("HEDERA_TEST_ENVELOPE").unwrap()).unwrap();
    let config = Config::from_env(Network::HederaTestnet).unwrap().unwrap();
    let inner: Arc<dyn Store> = Arc::new(store::DynamoStore::new(config.table.clone()).await);
    let provider = HederaProvider {
        client: config.client(),
        mirror: Mirror::new(config.mirror.clone()).unwrap(),
        config: config.clone(),
        store: Arc::new(FailTerminalOnce {
            inner: inner.clone(),
            fail: AtomicBool::new(true),
        }),
    };
    let (_, intent) = provider.inspect(&request.request, true).unwrap();
    assert!(intent.asset.is_hbar() && intent.amount <= 10000);
    let key = provider.record_key(&intent);
    let result = provider.settle(&request.request).await;
    assert!(
        matches!(
            result,
            Err(FacilitatorLocalError::SettlementUnconfirmed(..))
        ),
        "terminal-write outage must never report success"
    );
    let pending = inner.read(&key).await.unwrap().unwrap();
    assert!(!pending.terminal());
    assert!(pending.signed.is_some());
    println!("Uncertain original payment: {}", intent.transaction_id);
    let restarted = HederaProvider {
        client: config.client(),
        mirror: Mirror::new(config.mirror.clone()).unwrap(),
        config,
        store: inner.clone(),
    };
    tokio::time::sleep(Duration::from_secs(
        pending.lease_until.saturating_sub(store::now()) + 2,
    ))
    .await;
    // The background worker discovers it through the GSI after restart.
    restarted.start_recovery();
    for _ in 0..45 {
        if inner.read(&key).await.unwrap().unwrap().terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let recovered = inner.read(&key).await.unwrap().unwrap();
    assert_eq!(recovered.state, State::Confirmed);
    assert_eq!(
        recovered.signed, pending.signed,
        "no re-signing or new ID during recovery"
    );
    // Recovery, not this retry, confirmed it: the retry is not a second payment.
    // The id to look up is the one the unconfirmed answer carried.
    let retry = restarted.settle(&request.request).await.unwrap();
    assert!(already_submitted(&retry), "{retry:?}");
    println!("Recovered original payment: {}", intent.transaction_id);
}

#[tokio::test]
#[ignore = "read-only: requires explicit table and testnet transaction ID"]
async fn live_mirror_matches_persisted_signed_hash() {
    let table = std::env::var("HEDERA_TEST_TABLE").unwrap();
    let tx = std::env::var("HEDERA_TEST_TRANSACTION_ID").unwrap();
    let store = store::DynamoStore::new(table).await;
    let record = store
        .read(&format!("hedera:testnet#{tx}"))
        .await
        .unwrap()
        .unwrap();
    let mirror = Mirror::new("https://testnet.mirrornode.hedera.com/".parse().unwrap()).unwrap();
    let result = mirror
        .settled(&record.intent, record.signed.as_ref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(result.0);
    assert_eq!(result.1, "SUCCESS");
    let mut wrong_id = record.intent.clone();
    wrong_id.transaction_id = "0.0.10576385@1789601396.620842915".into();
    assert!(
        mirror
            .settled(&wrong_id, record.signed.as_ref().unwrap())
            .await
            .unwrap()
            .is_none(),
        "another valid on-chain receipt cannot authenticate these bytes"
    );
}

/// A ledger that fails its health check at startup is still served: `Ok(Some)`
/// from the same call `ProviderCache::from_env` makes, and `supported()` lists
/// it. Through 2.39.1 the failure was an `Err`, which that function turned into
/// `exit(1)` for every network; through 2.39.3 it was `Ok(None)`, which kept the
/// ledger out of /supported until the next deploy (Hedera mainnet, 2026-09-23,
/// after one consensus probe timed out). The Mirror Node here is a closed local
/// port, so the check cannot pass. Sets and clears process env, so it relies on
/// the suite's `--test-threads=1`.
#[tokio::test]
async fn a_ledger_failing_its_startup_health_is_still_served() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mirror = format!("https://127.0.0.1:{}/", closed.local_addr().unwrap().port());
    drop(closed);
    let key = hiero_sdk::PrivateKey::from_bytes_ed25519(&[9; 32]).unwrap();
    let vars = [
        ("HEDERA_ENABLED_TESTNET", "true".to_string()),
        ("HEDERA_ACCOUNT_ID_TESTNET", "0.0.1234".to_string()),
        ("HEDERA_PRIVATE_KEY_TESTNET", key.to_string()),
        ("HEDERA_MIRROR_URL_TESTNET", mirror),
        (
            "HEDERA_DAILY_BUDGET_TINYBARS_TESTNET",
            "1000000000".to_string(),
        ),
        (
            "HEDERA_SETTLEMENT_TABLE_NAME",
            "hedera-startup-health-test".to_string(),
        ),
    ];
    let previous: Vec<_> = vars
        .iter()
        .map(|(name, _)| (*name, std::env::var(name).ok()))
        .collect();
    for (name, value) in &vars {
        std::env::set_var(name, value);
    }

    let built = crate::chain::NetworkProvider::from_env(Network::HederaTestnet).await;

    for (name, value) in previous {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    let provider = match built {
        Ok(Some(provider)) => provider,
        Ok(None) => panic!("a failed startup health check must not take Hedera out of /supported"),
        Err(error) => panic!("a failed startup health check must not fail the build: {error}"),
    };
    let kinds = provider.supported().await.expect("supported").kinds;
    assert_eq!(
        kinds.iter().map(|k| k.network.as_str()).collect::<Vec<_>>(),
        ["hedera:testnet"]
    );
}

/// A closed local port: nothing listens there, so every connection is refused.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// The consensus nodes on a local port, so the probe never leaves this machine.
fn local_nodes(port: u16) -> std::collections::HashMap<String, hiero_sdk::AccountId> {
    std::collections::HashMap::from([(
        format!("127.0.0.1:{port}"),
        hiero_sdk::AccountId::new(0, 0, 3),
    )])
}

/// A Mirror Node that refuses the connection is `rpc_unreachable`; one that
/// accepts it and never answers is cut by the caller's timeout, which
/// `/health/ready` reports as `rpc_timeout` (`src/readiness.rs`).
#[tokio::test]
async fn an_unreachable_ledger_fails_its_health_with_a_bounded_reason() {
    let mirror: url::Url = format!("https://127.0.0.1:{}/", closed_port())
        .parse()
        .unwrap();
    let provider = HederaProvider::for_health_tests(
        Network::HederaTestnet,
        mirror,
        local_nodes(closed_port()),
    );
    let failure = provider.health().await.expect_err("nothing answers");
    assert_eq!(failure.reason(), "rpc_unreachable", "{failure}");

    // Accepts connections and never answers: TLS and gRPC both wait.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let mirror: url::Url = format!("https://127.0.0.1:{port}/").parse().unwrap();
    let provider =
        HederaProvider::for_health_tests(Network::HederaTestnet, mirror, local_nodes(port));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), provider.health())
            .await
            .is_err(),
        "a Mirror Node that never answers holds the check until the caller's timeout"
    );
    drop(silent);
}

/// The deterministic half of the check: the ledger's own answer about the
/// sponsor account. A key that is not ours is `signer_key_mismatch`; a balance
/// below one max fee is zero settles, not a failure.
#[test]
fn the_sponsor_account_answer_is_graded_without_the_network() {
    let key = hiero_sdk::PrivateKey::from_bytes_ed25519(&[7; 32]).unwrap();
    let other = hiero_sdk::PrivateKey::from_bytes_ed25519(&[8; 32]).unwrap();
    let account = |public: &hiero_sdk::PrivateKey, tinybars: u64| {
        json!({
            "account": "0.0.3003",
            "deleted": false,
            "key": {"_type": "ED25519", "key": hex::encode(public.public_key().to_bytes_raw())},
            "balance": {"balance": tinybars},
        })
    };
    let fee = DEFAULT_MAX_TRANSACTION_FEE_TINYBARS;
    assert_eq!(
        sponsor_settles(&account(&key, 250 * fee), &key, fee),
        Ok(250)
    );
    assert_eq!(sponsor_settles(&account(&key, fee - 1), &key, fee), Ok(0));
    let mismatch = sponsor_settles(&account(&other, 250 * fee), &key, fee).unwrap_err();
    assert_eq!(mismatch, HealthFailure::SponsorKeyMismatch);
    assert_eq!(mismatch.reason(), "signer_key_mismatch");
    let threshold = json!({"account": "0.0.3003", "deleted": false, "balance": {"balance": fee},
        "key": {"_type": "ProtobufEncoded", "key": hex::encode(pb::Key {
            key: Some(pb::key::Key::KeyList(pb::KeyList { keys: vec![] })),
        }.encode_to_vec())}});
    assert_eq!(
        sponsor_settles(&threshold, &key, fee),
        Err(HealthFailure::SponsorKeyMismatch),
        "a key this facilitator cannot sign for alone is not ours"
    );
}

#[test]
fn every_health_failure_has_a_bounded_reason() {
    assert_eq!(HealthFailure::ConsensusTimeout.reason(), "rpc_timeout");
    assert_eq!(
        HealthFailure::Unreachable("x".into()).reason(),
        "rpc_unreachable"
    );
    assert_eq!(
        HealthFailure::StoreUnavailable("x".into()).reason(),
        "store_unavailable"
    );
    assert_eq!(
        HealthFailure::SponsorKeyMismatch.reason(),
        "signer_key_mismatch"
    );
}

// --- Settles of a transaction that already has a record ---------------------
//
// No network: a settlement table with the conditional writes of
// `store::DynamoStore`, a Mirror Node on a local port, and consensus nodes on
// closed local ports (a send fails there and the Mirror decides, as `resolve`
// reads it). The Mirror knows the payment's transaction only once the chain
// "has" it: from the start for bytes another settle left, or from the moment a
// settle records them as submitted, just before it sends.

fn reason(response: &SettleResponse) -> Option<&str> {
    match &response.error_reason {
        Some(FacilitatorErrorReason::FreeForm(reason)) => Some(reason.as_str()),
        _ => None,
    }
}

/// Refused as already submitted: not a payment, and no transaction to deliver on.
fn already_submitted(response: &SettleResponse) -> bool {
    !response.success
        && response.transaction.is_none()
        && reason(response) == Some(ALREADY_SUBMITTED)
}

/// Base64 SHA-384 of the first entry of a signed transaction list: the
/// `transaction_hash` the Mirror Node reports for it.
fn first_entry_hash(signed: &str) -> String {
    use sha2::Sha384;
    let list = hiero_sdk_proto::sdk::TransactionList::decode(
        Decoded::from_base64(signed).unwrap().bytes.as_slice(),
    )
    .unwrap();
    STANDARD.encode(Sha384::digest(
        &list.transaction_list[0].signed_transaction_bytes,
    ))
}

/// `<account>@<seconds>.<nanos>` of a decoded transaction, as `codec` names it.
fn transaction_id_of(decoded: &Decoded) -> String {
    let id = decoded.variants[0].body.transaction_id.as_ref().unwrap();
    let start = id.transaction_valid_start.as_ref().unwrap();
    format!(
        "{}@{}.{:09}",
        codec::account(id.account_id.as_ref()).unwrap(),
        start.seconds,
        start.nanos
    )
}

/// What the chain holds of one payment, as the local Mirror Node reports it.
#[derive(Clone)]
struct Ledger {
    /// The hash of the transaction's first entry, once the chain has it.
    known: Arc<std::sync::Mutex<Option<String>>>,
    /// A hash the chain gets 3.5 s after the first question about the
    /// transaction: later than any read a settle makes before sending (3 s
    /// from before its first question), inside the read that follows.
    late: Arc<std::sync::Mutex<Option<String>>>,
    first_question: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    result: &'static str,
    hits: Arc<std::sync::atomic::AtomicUsize>,
}
impl Ledger {
    fn new(result: &'static str) -> Self {
        Self {
            known: Arc::default(),
            late: Arc::default(),
            first_question: Arc::default(),
            result,
            hits: Arc::default(),
        }
    }
    fn learn(&self, signed: &str) {
        *self.known.lock().unwrap() = Some(first_entry_hash(signed));
    }
    fn learn_late(&self, signed: &str) {
        *self.late.lock().unwrap() = Some(first_entry_hash(signed));
    }
    /// Called on every question about the transaction.
    fn asked(&self) {
        let now = std::time::Instant::now();
        let first = *self.first_question.lock().unwrap().get_or_insert(now);
        if now.duration_since(first) >= Duration::from_millis(3500) {
            if let Some(hash) = self.late.lock().unwrap().take() {
                *self.known.lock().unwrap() = Some(hash);
            }
        }
    }
    fn knows(&self) -> bool {
        self.known.lock().unwrap().is_some()
    }
    fn hits(&self) -> usize {
        self.hits.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A Mirror Node on a local port: the accounts and token of a native-USDC
/// payment from 0.0.1001 to 0.0.2002 sponsored by 0.0.3003, and the payment's
/// transaction once `ledger` knows it. Counts every request.
async fn local_mirror(
    ledger: Ledger,
    transaction_id: &str,
    payer: &hiero_sdk::PrivateKey,
    sponsor: &hiero_sdk::PrivateKey,
) -> url::Url {
    use axum::response::IntoResponse;
    let usdc = "0.0.429274";
    let key = |k: &hiero_sdk::PrivateKey| json!({"_type": "ED25519", "key": hex::encode(k.public_key().to_bytes_raw())});
    let payee = hiero_sdk::PrivateKey::from_bytes_ed25519(&[8; 32]).unwrap();
    let association = |balance: u64| json!({"links": {"next": null}, "tokens": [{"token_id": usdc,
        "decimals": 6, "freeze_status": "UNFROZEN", "kyc_status": "GRANTED", "balance": balance}]});
    let fixed: std::collections::HashMap<String, Value> = [
        ("/api/v1/accounts/0.0.1001".to_string(), json!({"account": "0.0.1001", "deleted": false,
            "key": key(payer), "balance": {"balance": 0}})),
        ("/api/v1/accounts/0.0.2002".to_string(), json!({"account": "0.0.2002", "deleted": false,
            "receiver_sig_required": false, "key": key(&payee), "balance": {"balance": 0}})),
        ("/api/v1/accounts/0.0.3003".to_string(), json!({"account": "0.0.3003", "deleted": false,
            "key": key(sponsor), "balance": {"balance": 100 * DEFAULT_MAX_TRANSACTION_FEE_TINYBARS}})),
        (format!("/api/v1/tokens/{usdc}"), json!({"token_id": usdc, "type": "FUNGIBLE_COMMON",
            "deleted": false, "pause_status": "UNPAUSED", "decimals": "6",
            "custom_fees": {"created_timestamp": "1.000000000", "fixed_fees": [], "fractional_fees": []}})),
        ("/api/v1/accounts/0.0.1001/tokens".to_string(), association(1_000_000)),
        ("/api/v1/accounts/0.0.2002/tokens".to_string(), association(0)),
    ]
    .into_iter()
    .collect();
    let (account, start) = transaction_id.split_once('@').unwrap();
    let (seconds, nanos) = start.split_once('.').unwrap();
    let mirror_id = format!("{account}-{seconds}-{nanos}");
    let transaction_path = format!("/api/v1/transactions/{mirror_id}");
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let (fixed, ledger) = (fixed.clone(), ledger.clone());
        let (transaction_path, mirror_id) = (transaction_path.clone(), mirror_id.clone());
        async move {
            ledger.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if uri.path() == transaction_path {
                ledger.asked();
                let known = ledger.known.lock().unwrap().clone();
                let rows: Vec<Value> = known
                    .map(|hash| json!({"transaction_id": mirror_id, "nonce": 0,
                        "transaction_hash": hash, "result": ledger.result}))
                    .into_iter()
                    .collect();
                return axum::Json(json!({"transactions": rows})).into_response();
            }
            match fixed.get(uri.path()) {
                Some(body) => axum::Json(body.clone()).into_response(),
                None => axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// The settlement table, in this process, with `store::DynamoStore`'s
/// conditions: `reserve` creates a record or takes over one whose lease ran
/// out (never a terminal one), and `save` writes only for the record's owner,
/// never over a terminal record. A record saved as submitted is about to be
/// sent: from then on the ledger has it.
struct MemoryTable {
    rows: std::sync::Mutex<std::collections::HashMap<String, Record>>,
    ledger: Ledger,
    /// Records saved as submitted: sends that were about to happen.
    submitted: std::sync::atomic::AtomicUsize,
}
impl MemoryTable {
    fn new(ledger: Ledger) -> Self {
        Self {
            rows: Default::default(),
            ledger,
            submitted: Default::default(),
        }
    }
    fn submitted(&self) -> usize {
        self.submitted.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn put(&self, key: &str, record: Record) {
        self.rows.lock().unwrap().insert(key.into(), record);
    }
    fn get(&self, key: &str) -> Record {
        self.rows.lock().unwrap()[key].clone()
    }
}
#[async_trait::async_trait]
impl Store for MemoryTable {
    async fn read(&self, key: &str) -> Result<Option<Record>> {
        Ok(self.rows.lock().unwrap().get(key).cloned())
    }
    async fn reserve(
        &self,
        key: &str,
        _: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record> {
        let at = store::now();
        let mut rows = self.rows.lock().unwrap();
        if let Some(existing) = rows.get_mut(key) {
            if existing.intent.fingerprint != intent.fingerprint {
                return Err("transaction ID already binds another intent".into());
            }
            if !existing.terminal() && existing.lease_until < at {
                existing.owner = owner.into();
                existing.lease_until = at + store::LEASE_SECONDS;
            }
            return Ok(existing.clone());
        }
        if at.saturating_add(5) > intent.expires_at || intent.fee > budget {
            return Err("expired payment or sponsor quota exceeded".into());
        }
        let record = Record {
            intent: intent.clone(),
            owner: owner.into(),
            lease_until: at + store::LEASE_SECONDS,
            state: State::Reserved,
            signed: None,
            consensus_status: None,
        };
        rows.insert(key.into(), record.clone());
        Ok(record)
    }
    async fn save(&self, key: &str, record: &Record) -> Result<()> {
        let mut rows = self.rows.lock().unwrap();
        match rows.get(key) {
            Some(current)
                if current.owner == record.owner
                    && current.intent.fingerprint == record.intent.fingerprint
                    && !current.terminal() =>
            {
                rows.insert(key.into(), record.clone());
                if record.state == State::Submitted {
                    self.submitted
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    self.ledger.learn(record.signed.as_deref().unwrap());
                }
                Ok(())
            }
            _ => Err("conditional check failed".into()),
        }
    }
    async fn pending(&self, _: &str) -> Result<Vec<(String, Record)>> {
        Ok(vec![])
    }
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// The table, with the settles of a race lined up: all of them read the record
/// before any reserves it. With `second_waits`, the second reservation waits
/// until a terminal record is saved, so it finds the other settle's result.
struct Race {
    table: Arc<MemoryTable>,
    read_together: tokio::sync::Barrier,
    second_waits: bool,
    reserves: std::sync::atomic::AtomicUsize,
    terminal: tokio::sync::Semaphore,
}
impl Race {
    fn new(table: Arc<MemoryTable>, settles: usize, second_waits: bool) -> Self {
        Self {
            table,
            read_together: tokio::sync::Barrier::new(settles),
            second_waits,
            reserves: std::sync::atomic::AtomicUsize::new(0),
            terminal: tokio::sync::Semaphore::new(0),
        }
    }
}
#[async_trait::async_trait]
impl Store for Race {
    async fn read(&self, key: &str) -> Result<Option<Record>> {
        let record = self.table.read(key).await;
        self.read_together.wait().await;
        record
    }
    async fn reserve(
        &self,
        key: &str,
        network: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record> {
        use std::sync::atomic::Ordering;
        if self.second_waits && self.reserves.fetch_add(1, Ordering::SeqCst) == 1 {
            self.terminal.acquire().await.unwrap().forget();
        }
        self.table.reserve(key, network, intent, owner, budget).await
    }
    async fn save(&self, key: &str, record: &Record) -> Result<()> {
        self.table.save(key, record).await?;
        if record.terminal() {
            self.terminal.add_permits(1);
        }
        Ok(())
    }
    async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>> {
        self.table.pending(network).await
    }
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// One native-USDC testnet payment, its settlement table and its ledger.
struct Payment {
    request: VerifyRequest,
    table: Arc<MemoryTable>,
    provider: HederaProvider,
    key: String,
    transaction_id: String,
    ledger: Ledger,
    /// The bytes a settle co-signs: the client's transaction and the sponsor's
    /// signature.
    cosigned: String,
}
impl Payment {
    fn sponsor() -> hiero_sdk::PrivateKey {
        // The key `HederaProvider::for_health_tests` sponsors with.
        hiero_sdk::PrivateKey::from_bytes_ed25519(&[7; 32]).unwrap()
    }
    fn payer() -> hiero_sdk::PrivateKey {
        hiero_sdk::PrivateKey::from_bytes_ed25519(&[9; 32]).unwrap()
    }
    async fn build(transaction: String, result: &'static str) -> Self {
        let requirements = json!({"scheme": "exact", "network": "hedera:testnet",
            "asset": "0.0.429274", "amount": "150000", "payTo": "0.0.2002",
            "maxTimeoutSeconds": 180, "extra": {"feePayer": "0.0.3003"}});
        let request = serde_json::from_value::<crate::types_v2::VerifyRequestEnvelope>(json!({
            "x402Version": 2,
            "paymentPayload": {"x402Version": 2, "accepted": requirements,
                "payload": {"transaction": transaction}},
            "paymentRequirements": requirements,
        }))
        .unwrap()
        .to_v1()
        .unwrap();
        let decoded = Decoded::from_base64(&transaction).unwrap();
        // The transaction's own consensus nodes, each on a closed local port.
        let mut ports = BTreeSet::new();
        while ports.len() < decoded.variants.len() {
            ports.insert(closed_port());
        }
        let nodes: std::collections::HashMap<String, hiero_sdk::AccountId> = decoded
            .variants
            .iter()
            .zip(ports)
            .map(|(variant, port)| {
                let node = codec::account(variant.body.node_account_id.as_ref()).unwrap();
                (format!("127.0.0.1:{port}"), node.to_string().parse().unwrap())
            })
            .collect();
        let transaction_id = transaction_id_of(&decoded);
        let ledger = Ledger::new(result);
        let mirror = local_mirror(
            ledger.clone(),
            &transaction_id,
            &Self::payer(),
            &Self::sponsor(),
        )
        .await;
        let table = Arc::new(MemoryTable::new(ledger.clone()));
        let mut provider = HederaProvider::for_health_tests(Network::HederaTestnet, mirror, nodes);
        provider.config.settle_timeout = Duration::from_secs(15);
        provider.store = table.clone();
        let (_, intent) = provider.inspect(&request, false).unwrap();
        assert_eq!(intent.transaction_id, transaction_id);
        let key = provider.record_key(&intent);
        let cosigned = STANDARD.encode(decoded.cosign(&Self::sponsor()).unwrap());
        Self {
            request,
            table,
            provider,
            key,
            transaction_id,
            ledger,
            cosigned,
        }
    }
    /// The official client's native-USDC transaction (2025, long expired).
    fn vector_transaction() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/hedera-e2e/vectors/03-hts-usdc-ecdsa-multinode.json");
        let vector: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        vector["payload"]["transaction"].as_str().unwrap().to_owned()
    }
    /// A payment the client signed just now: valid, not on chain, no record.
    /// The official client's transaction with only its start moved to now and
    /// every node variant signed again by `payer()`.
    async fn new(result: &'static str) -> Self {
        let payer = Self::payer();
        let bytes = STANDARD.decode(Self::vector_transaction()).unwrap();
        let mut list = hiero_sdk_proto::sdk::TransactionList::decode(bytes.as_slice()).unwrap();
        let start = store::now() as i64 - 5;
        for entry in &mut list.transaction_list {
            let mut signed =
                pb::SignedTransaction::decode(entry.signed_transaction_bytes.as_slice()).unwrap();
            let mut body = pb::TransactionBody::decode(signed.body_bytes.as_slice()).unwrap();
            let valid_start = body
                .transaction_id
                .as_mut()
                .and_then(|id| id.transaction_valid_start.as_mut())
                .unwrap();
            valid_start.seconds = start;
            valid_start.nanos = 0;
            signed.body_bytes = body.encode_to_vec();
            let signature = payer.sign(&signed.body_bytes);
            signed.sig_map = Some(pb::SignatureMap {
                sig_pair: vec![pb::SignaturePair {
                    pub_key_prefix: payer.public_key().to_bytes_raw(),
                    signature: Some(pb::signature_pair::Signature::Ed25519(signature)),
                }],
            });
            entry.signed_transaction_bytes = signed.encode_to_vec();
        }
        Self::build(STANDARD.encode(list.encode_to_vec()), result).await
    }
    /// The vector as it is, long expired: bytes nobody can send any more, left
    /// by a settle that answered `502 settlement_unconfirmed`. With `on_chain`,
    /// the chain has them.
    async fn left_by_another(result: &'static str, on_chain: bool) -> Self {
        let payment = Self::build(Self::vector_transaction(), result).await;
        payment.leave(State::Uncertain, "a settle that answered 502", 0);
        if on_chain {
            payment.ledger.learn(&payment.cosigned);
        }
        payment
    }
    /// Puts the record another settle left: the co-signed bytes, in `state`,
    /// owned by `owner` until `lease_until`.
    fn leave(&self, state: State, owner: &str, lease_until: u64) {
        let (_, intent) = self.provider.inspect(&self.request, false).unwrap();
        self.table.put(
            &self.key,
            Record {
                intent,
                owner: owner.into(),
                lease_until,
                state,
                signed: Some(self.cosigned.clone()),
                consensus_status: None,
            },
        );
    }
    /// The same provider on another view of the table: another task, or replica.
    fn provider_on(&self, store: Arc<dyn Store>) -> HederaProvider {
        let mut provider = self.provider.clone();
        provider.store = store;
        provider
    }
}

/// A legitimate payment settles: admitted, co-signed, sent and confirmed by
/// the one settle that reserved it.
#[tokio::test]
async fn a_new_payment_settles_once_and_answers_its_success() {
    let payment = Payment::new("SUCCESS").await;
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(response.success, "{response:?}");
    assert!(response.error_reason.is_none(), "{response:?}");
    assert_eq!(
        response.transaction.map(|t| t.to_string()),
        Some(payment.transaction_id.clone())
    );
    assert_eq!(response.payer.to_string(), "0.0.1001");
    assert!(payment.ledger.knows(), "the settle sent its co-signed bytes");
    let record = payment.table.get(&payment.key);
    assert_eq!(record.state, State::Confirmed);
    assert_eq!(record.consensus_status.as_deref(), Some("SUCCESS"));
}

/// A second settle of a confirmed payment is not a second payment: it answers
/// `transaction_already_submitted`, asks nothing and writes nothing.
#[tokio::test]
async fn a_second_settle_after_a_confirmed_payment_is_refused_and_sends_nothing() {
    let payment = Payment::new("SUCCESS").await;
    let first = payment.provider.settle(&payment.request).await.unwrap();
    assert!(first.success, "{first:?}");
    let confirmed = payment.table.get(&payment.key);
    let hits = payment.ledger.hits();

    let second = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&second), "{second:?}");
    assert_eq!(second.payer.to_string(), "0.0.1001");
    assert_eq!(second.network, Network::HederaTestnet);
    // On the wire: the token, spelled out, and no transaction to look up and
    // deliver on.
    let wire = serde_json::to_value(&second).unwrap();
    assert_eq!(wire["success"], json!(false));
    assert_eq!(wire["errorReason"], json!("transaction_already_submitted"));
    assert!(wire.get("transaction").is_none_or(Value::is_null), "{wire}");
    assert_eq!(payment.ledger.hits(), hits, "the repeat asked the ledger nothing");
    let after = payment.table.get(&payment.key);
    assert_eq!(after.owner, confirmed.owner);
    assert_eq!(after.state, State::Confirmed);
}

/// Two settles of one new payment at once: the one that reserves it pays;
/// the other finds it held and is refused, without a transaction.
#[tokio::test]
async fn two_settles_of_a_new_payment_at_once_answer_one_success() {
    let payment = Payment::new("SUCCESS").await;
    let race: Arc<dyn Store> = Arc::new(Race::new(payment.table.clone(), 2, false));
    let (a, b) = (payment.provider_on(race.clone()), payment.provider_on(race));
    let (first, second) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::join!(a.settle(&payment.request), b.settle(&payment.request))
    })
    .await
    .expect("both settles answer");
    let answers = [first.unwrap(), second.unwrap()];
    assert_eq!(answers.iter().filter(|r| r.success).count(), 1, "{answers:?}");
    assert_eq!(
        answers.iter().filter(|r| already_submitted(r)).count(),
        1,
        "{answers:?}"
    );
    assert_eq!(payment.table.get(&payment.key).state, State::Confirmed);
}

/// The race between a settle's read and its reservation: the other settle
/// confirms the payment in between. Only that one answers its success.
#[tokio::test]
async fn a_settle_that_read_the_record_before_another_confirmed_it_is_not_a_payment() {
    let payment = Payment::new("SUCCESS").await;
    let race: Arc<dyn Store> = Arc::new(Race::new(payment.table.clone(), 2, true));
    let (a, b) = (payment.provider_on(race.clone()), payment.provider_on(race));
    let (first, second) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::join!(a.settle(&payment.request), b.settle(&payment.request))
    })
    .await
    .expect("one settle confirms the payment and the other then reserves");
    let answers = [first.unwrap(), second.unwrap()];
    assert_eq!(answers.iter().filter(|r| r.success).count(), 1, "{answers:?}");
    assert_eq!(
        answers.iter().filter(|r| already_submitted(r)).count(),
        1,
        "{answers:?}"
    );
    assert_eq!(payment.table.get(&payment.key).state, State::Confirmed);
}

/// The same race, but the chain refuses the payment: both settles answer the
/// chain's verdict, neither is a success nor already submitted.
#[tokio::test]
async fn a_settle_that_read_the_record_before_another_failed_it_answers_the_verdict() {
    let payment = Payment::new("INSUFFICIENT_TOKEN_BALANCE").await;
    let race: Arc<dyn Store> = Arc::new(Race::new(payment.table.clone(), 2, true));
    let (a, b) = (payment.provider_on(race.clone()), payment.provider_on(race));
    let (first, second) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::join!(a.settle(&payment.request), b.settle(&payment.request))
    })
    .await
    .expect("one settle records the verdict and the other then reserves");
    for answer in [first, second] {
        let response = answer.unwrap();
        assert!(!response.success, "{response:?}");
        assert_eq!(
            reason(&response),
            Some("hedera_consensus_INSUFFICIENT_TOKEN_BALANCE")
        );
    }
    assert_eq!(payment.table.get(&payment.key).state, State::Failed);
}

/// Four settles of one new payment at once, on four tasks over one table:
/// exactly one success, and the other three already submitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_settles_of_one_payment_answer_one_success() {
    let payment = Payment::new("SUCCESS").await;
    let race: Arc<dyn Store> = Arc::new(Race::new(payment.table.clone(), 4, false));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let provider = payment.provider_on(race.clone());
        let request = payment.request.clone();
        tasks.spawn(async move { provider.settle(&request).await });
    }
    let (mut successes, mut refused) = (0, 0);
    while let Some(joined) = tokio::time::timeout(Duration::from_secs(90), tasks.join_next())
        .await
        .expect("every settle answers")
    {
        let response = joined.unwrap().unwrap();
        if response.success {
            successes += 1;
        } else {
            assert!(already_submitted(&response), "{response:?}");
            refused += 1;
        }
    }
    assert_eq!((successes, refused), (1, 3));
    assert_eq!(payment.table.get(&payment.key).state, State::Confirmed);
}

/// A settle that stopped after persisting its co-signed bytes and before
/// sending them: its retry takes the record over, finds no outcome, sends the
/// same bytes itself, and that send is the payment. It answers its success.
#[tokio::test]
async fn a_retry_after_a_settle_that_stopped_before_sending_sends_and_answers_its_success() {
    let payment = Payment::new("SUCCESS").await;
    payment.leave(State::Prepared, "a settle that stopped before sending", 0);
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(response.success, "{response:?}");
    assert_eq!(
        response.transaction.map(|t| t.to_string()),
        Some(payment.transaction_id.clone())
    );
    assert!(payment.ledger.knows(), "the retry sent the persisted bytes");
    let record = payment.table.get(&payment.key);
    assert_eq!(record.state, State::Confirmed);
    assert_eq!(record.signed.as_deref(), Some(payment.cosigned.as_str()));
}

/// A settle that takes over bytes another settle left and finds their outcome
/// on chain before sending anything: it records the verdict, and the outcome
/// is that settle's, not a payment for this one.
#[tokio::test]
async fn a_settle_that_resumes_another_settles_bytes_and_finds_their_outcome_is_refused() {
    let payment = Payment::left_by_another("SUCCESS", true).await;
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    let record = payment.table.get(&payment.key);
    assert_eq!(record.state, State::Confirmed, "the verdict is still recorded");
    assert_eq!(record.consensus_status.as_deref(), Some("SUCCESS"));
    assert_ne!(record.owner, "a settle that answered 502");
}

/// A settle that takes over bytes another settle left, cannot send them any
/// more and finds no outcome: still unknown, and not this settle's either.
#[tokio::test]
async fn a_settle_that_resumes_another_settles_expired_bytes_without_an_outcome_is_refused() {
    let mut payment = Payment::left_by_another("SUCCESS", false).await;
    payment.provider.config.settle_timeout = Duration::from_secs(4);
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert!(!payment.ledger.knows(), "nothing was sent");
    let record = payment.table.get(&payment.key);
    assert_eq!(record.state, State::Uncertain);
    assert!(record.lease_until > store::now(), "recovery takes it from here");
}

/// A record another settle still holds: this settle reserves, asks and sends
/// nothing, and its answer names no transaction to deliver on.
#[tokio::test]
async fn a_settle_of_a_payment_another_settle_holds_is_refused_and_sends_nothing() {
    let payment = Payment::left_by_another("SUCCESS", true).await;
    payment.leave(
        State::Submitted,
        "a settle still running",
        store::now() + store::LEASE_SECONDS,
    );
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(payment.ledger.hits(), 0);
    let after = payment.table.get(&payment.key);
    assert_eq!(after.owner, "a settle still running");
    assert_eq!(after.state, State::Submitted);
}

/// A payment that failed on chain is not a success either way: its repeat
/// still answers the chain's verdict with its transaction, as before.
#[tokio::test]
async fn a_repeat_of_a_payment_that_failed_on_chain_answers_its_verdict() {
    let payment = Payment::new("INSUFFICIENT_TOKEN_BALANCE").await;
    for _ in 0..2 {
        let response = payment.provider.settle(&payment.request).await.unwrap();
        assert!(!response.success, "{response:?}");
        assert_eq!(
            reason(&response),
            Some("hedera_consensus_INSUFFICIENT_TOKEN_BALANCE")
        );
        assert_eq!(
            response.transaction.map(|t| t.to_string()),
            Some(payment.transaction_id.clone())
        );
    }
    assert_eq!(payment.table.get(&payment.key).state, State::Failed);

    // Found at the first read, the verdict is answered without reserving.
    let failed = payment.table.get(&payment.key);
    let read_only = payment.provider_on(Arc::new(ReadOnlyPolicyStore(Some(failed))));
    let response = read_only.settle(&payment.request).await.unwrap();
    assert!(!response.success, "{response:?}");
    assert_eq!(
        reason(&response),
        Some("hedera_consensus_INSUFFICIENT_TOKEN_BALANCE")
    );
}

/// The table, except that the write before a send is refused.
struct NoSubmit(Arc<MemoryTable>);
#[async_trait::async_trait]
impl Store for NoSubmit {
    async fn read(&self, key: &str) -> Result<Option<Record>> {
        self.0.read(key).await
    }
    async fn reserve(
        &self,
        key: &str,
        network: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record> {
        self.0.reserve(key, network, intent, owner, budget).await
    }
    async fn save(&self, key: &str, record: &Record) -> Result<()> {
        if record.state == State::Submitted {
            return Err("injected outage before the send".into());
        }
        self.0.save(key, record).await
    }
    async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>> {
        self.0.pending(network).await
    }
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// When the write before a send fails, nothing is sent. The settle that
/// co-signed the bytes reports them under the id, since recovery may still
/// send them; one that took over another settle's bytes is refused.
#[tokio::test]
async fn a_settle_whose_write_before_sending_fails_sends_nothing() {
    let payment = Payment::new("SUCCESS").await;
    let blocked = payment.provider_on(Arc::new(NoSubmit(payment.table.clone())));
    let error = blocked.settle(&payment.request).await.unwrap_err();
    assert!(
        matches!(error, FacilitatorLocalError::SettlementUnconfirmed(..)),
        "{error:?}"
    );

    payment.leave(State::Prepared, "a settle that stopped before sending", 0);
    let response = blocked.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert!(!payment.ledger.knows(), "nothing was sent");
}

/// A settle that takes over bytes another settle left, still valid, whose
/// outcome is already on chain: it sends nothing (it never records them as
/// submitted) and the outcome is not its own.
#[tokio::test]
async fn a_settle_that_resumes_valid_bytes_already_on_chain_sends_nothing_and_is_refused() {
    let payment = Payment::new("SUCCESS").await;
    payment.leave(State::Uncertain, "a settle that answered 502", 0);
    payment.ledger.learn(&payment.cosigned);
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(payment.table.submitted(), 0, "the settle sent nothing");
    let record = payment.table.get(&payment.key);
    assert_eq!(record.state, State::Confirmed, "the verdict is still recorded");
}

/// A settle that takes over bytes it can no longer send, whose outcome only
/// shows after its read before sending: the outcome is still not its own.
#[tokio::test]
async fn a_settle_that_resumes_expired_bytes_whose_outcome_shows_late_is_refused() {
    let mut payment = Payment::left_by_another("SUCCESS", false).await;
    payment.provider.config.settle_timeout = Duration::from_secs(12);
    payment.ledger.learn_late(&payment.cosigned);
    let response = payment.provider.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(payment.table.get(&payment.key).state, State::Confirmed);
}

/// The table, except that a terminal record is never saved.
struct NoTerminal(Arc<MemoryTable>);
#[async_trait::async_trait]
impl Store for NoTerminal {
    async fn read(&self, key: &str) -> Result<Option<Record>> {
        self.0.read(key).await
    }
    async fn reserve(
        &self,
        key: &str,
        network: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record> {
        self.0.reserve(key, network, intent, owner, budget).await
    }
    async fn save(&self, key: &str, record: &Record) -> Result<()> {
        if record.terminal() {
            return Err("injected outage at the verdict".into());
        }
        self.0.save(key, record).await
    }
    async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>> {
        self.0.pending(network).await
    }
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// A settle that takes over another settle's bytes, finds their outcome
/// (before sending, or later) and cannot record it: still not its own, and
/// not an unconfirmed send of its own either.
#[tokio::test]
async fn a_settle_that_resumes_another_settles_bytes_and_cannot_record_their_outcome_is_refused() {
    let payment = Payment::left_by_another("SUCCESS", true).await;
    let blocked = payment.provider_on(Arc::new(NoTerminal(payment.table.clone())));
    let response = blocked.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(payment.table.get(&payment.key).state, State::Uncertain);

    let payment = Payment::left_by_another("SUCCESS", false).await;
    payment.ledger.learn_late(&payment.cosigned);
    let mut blocked = payment.provider_on(Arc::new(NoTerminal(payment.table.clone())));
    blocked.config.settle_timeout = Duration::from_secs(12);
    let response = blocked.settle(&payment.request).await.unwrap();
    assert!(already_submitted(&response), "{response:?}");
    assert_eq!(payment.table.get(&payment.key).state, State::Uncertain);
}

/// A transaction id the table binds to another intent is refused, as before,
/// whatever state that record is in.
#[tokio::test]
async fn a_settle_whose_transaction_id_binds_another_intent_is_refused_as_before() {
    let payment = Payment::left_by_another("SUCCESS", true).await;
    for state in [State::Uncertain, State::Confirmed] {
        let mut other = payment.table.get(&payment.key);
        other.intent.fingerprint = "another intent".into();
        other.state = state;
        payment.table.put(&payment.key, other);
        let error = payment.provider.settle(&payment.request).await.unwrap_err();
        assert!(
            error.to_string().contains("transaction ID binds another intent"),
            "{error}"
        );
    }
    assert_eq!(payment.ledger.hits(), 0);
}
