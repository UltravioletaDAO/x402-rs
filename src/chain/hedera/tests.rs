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

    // An already-confirmed HBAR payment returns its original receipt without
    // reserving, co-signing or broadcasting another transaction.
    let (_, intent) = provider.inspect(&request, false).unwrap();
    let transaction_id = intent.transaction_id.clone();
    provider.store = Arc::new(ReadOnlyPolicyStore(Some(Record {
        intent, owner: "historical".into(), lease_until: 0,
        state: State::Confirmed, signed: None, consensus_status: Some("SUCCESS".into()),
    })));
    let response = provider.settle(&request).await.unwrap();
    assert!(response.success);
    assert_eq!(response.transaction.unwrap().to_string(), transaction_id);
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
    let retry = restarted.settle(&request.request).await.unwrap();
    assert!(retry.success);
    assert_eq!(
        retry.transaction.unwrap().to_string(),
        intent.transaction_id
    );
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
