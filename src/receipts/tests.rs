use super::*;
use crate::types::{MixedAddress, SettleRequest, SettleResponse, SupportedPaymentKindsResponse};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct MockFacilitator {
    invalid: bool,
}
impl ProviderMap for MockFacilitator {
    type Value = NetworkProvider;
    fn by_network<N: std::borrow::Borrow<Network>>(&self, _: N) -> Option<&Self::Value> {
        None
    }
    fn values(&self) -> impl Iterator<Item = &Self::Value> + Send {
        std::iter::empty()
    }
}
impl HasProviderMap for MockFacilitator {
    type Map = Self;
    fn provider_map(&self) -> &Self {
        self
    }
}
impl Facilitator for MockFacilitator {
    type Error = StatusCode;
    async fn verify(&self, _: &VerifyRequest) -> std::result::Result<VerifyResponse, Self::Error> {
        if self.invalid {
            Ok(VerifyResponse::invalid(
                None,
                crate::types::FacilitatorErrorReason::FreeForm("invalid_signature".into()),
            ))
        } else {
            Ok(VerifyResponse::valid(MixedAddress::Evm(
                "0x1111111111111111111111111111111111111111"
                    .parse()
                    .unwrap(),
            )))
        }
    }
    async fn settle(&self, _: &SettleRequest) -> std::result::Result<SettleResponse, Self::Error> {
        panic!("use the guarded closure")
    }
    async fn supported(&self) -> std::result::Result<SupportedPaymentKindsResponse, Self::Error> {
        unreachable!()
    }
}

fn service_fixture() -> Arc<Service> {
    Arc::new(Service {
        store: Arc::new(store::MemoryStore::default()),
        signing_key: Some(SigningKey::from_bytes(&[7; 32])),
    })
}

#[tokio::test]
async fn pre_receipt_cache_preserves_success_and_conflict_without_a_fabricated_receipt() {
    let old = crate::idempotency_store::IdempotencyRecord {
        idempotency_key: "before-upgrade".into(),
        request_hash: "original-body".into(),
        response_json: serde_json::to_string(&value(success()).await).unwrap(),
        expires_at: now() + 60,
    };
    let replay = legacy_response(old.clone(), "original-body");
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    let cached = value(replay).await;
    assert_eq!(cached["success"], true);
    assert!(cached.get("receipt").is_none());
    let mut migrated = old.clone();
    migrated.request_hash = crate::idempotency_store::hash_request_body(&body(1));
    let mut h = headers();
    h.insert(
        "idempotency-key",
        HeaderValue::from_static("before-upgrade"),
    );
    TEST_SERVICE
        .scope(
            service_fixture(),
            TEST_LEGACY_RECORD.scope(migrated, async {
                let replay = settle(&MockFacilitator { invalid: true }, &h, &body(1), async {
                    panic!("old payment broadcast again")
                })
                .await;
                assert_eq!(
                    value(replay).await["success"],
                    true,
                    "consumed nonce must not override cached success"
                );
            }),
        )
        .await;
    assert_eq!(
        legacy_response(old, "changed-body").status(),
        StatusCode::CONFLICT
    );
}
pub(super) fn fixture_record() -> Record {
    initial(
        &parse_request(&headers(), &body(1)).unwrap(),
        purchase_context(&headers()).unwrap().as_ref(),
        "settle",
    )
    .unwrap()
}

struct UpdateOutage(store::MemoryStore);
#[async_trait::async_trait]
impl store::Store for UpdateOutage {
    async fn get(&self, key: &str) -> Result<Option<Record>> {
        self.0.get(key).await
    }
    async fn reserve(&self, record: &Record, aliases: &[String]) -> Result<bool> {
        self.0.reserve(record, aliases).await
    }
    async fn save(&self, _: &Record, _: u64) -> Result<bool> {
        Err("offline".into())
    }
}

#[tokio::test]
async fn preparation_outage_blocks_broadcast_and_never_returns_an_unstored_revision() {
    let service = Arc::new(Service {
        store: Arc::new(UpdateOutage(store::MemoryStore::default())),
        signing_key: Some(SigningKey::from_bytes(&[7; 32])),
    });
    TEST_SERVICE
        .scope(service.clone(), async {
            let result = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async {
                    assert!(
                        prepared_evm(format!("0x{}", "22".repeat(32)), vec![1, 2, 3])
                            .await
                            .is_err()
                    );
                    let current = ACTIVE.with(Arc::clone);
                    assert!(current.lock().await.prepared.is_none());
                    failure("preparation_unavailable", StatusCode::SERVICE_UNAVAILABLE)
                },
            )
            .await;
            let output = value(result).await;
            let id = output["receipt"]["receiptId"].as_str().unwrap();
            let stored = service
                .store
                .get(&format!("receipt:v1:{id}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                output["receipt"],
                serde_json::to_value(stored.receipt).unwrap()
            );
            assert_eq!(output["receipt"]["revision"], 1);
            assert_eq!(output["receipt"]["status"], "unknown");
        })
        .await;
}

#[tokio::test]
async fn persisted_transaction_is_recoverable_after_lost_response() {
    let service = service_fixture();
    TEST_SERVICE
        .scope(service.clone(), async {
            let result = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async {
                    prepared_evm(format!("0x{}", "22".repeat(32)), vec![1, 2, 3])
                        .await
                        .unwrap();
                    failure("response_lost", StatusCode::BAD_GATEWAY)
                },
            )
            .await;
            let output = value(result).await;
            assert_eq!(output["receipt"]["status"], "unknown");
            let id = output["receipt"]["receiptId"].as_str().unwrap();
            let stored = service
                .store
                .get(&format!("receipt:v1:{id}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.prepared.unwrap()["signedTransaction"],
                STANDARD.encode([1, 2, 3])
            );
            assert!(output["receipt"].get("signedTransaction").is_none());
        })
        .await;
}
fn body(nonce: u8) -> Bytes {
    Bytes::from(serde_json::to_vec(&json!({
        "x402Version":1,
        "paymentPayload":{"x402Version":1,"scheme":"exact","network":"arc-testnet","payload":{
            "signature":format!("0x{}", "11".repeat(65)),"authorization":{
                "from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222",
                "value":"1000","validAfter":"0","validBefore":"2000000000","nonce":format!("0x{}",hex::encode([nonce;32]))}}},
        "paymentRequirements":{"scheme":"exact","network":"arc-testnet","maxAmountRequired":"1000",
            "resource":"https://merchant.example/data","description":"receipt fixture","mimeType":"application/json",
            "payTo":"0x2222222222222222222222222222222222222222","maxTimeoutSeconds":60,
            "asset":"0x3600000000000000000000000000000000000000","extra":{"name":"USDC","version":"2"}}
    })).unwrap())
}
fn headers() -> HeaderMap {
    let context = PurchaseContext {
        purchase_id: "order-fixture".into(),
        access_token: "ab".repeat(32),
        method: "GET".into(),
        url: "https://merchant.example/data".into(),
        body_sha256: hash(b""),
    };
    let mut h = HeaderMap::new();
    h.insert(
        "x-uvd-purchase",
        HeaderValue::from_str(&STANDARD.encode(serde_json::to_vec(&context).unwrap())).unwrap(),
    );
    h
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}
fn success() -> Response {
    (StatusCode::OK, Json(json!({"success":true,"network":"arc-testnet","payer":"0x1111111111111111111111111111111111111111",
        "transaction":format!("0x{}","22".repeat(32)),"paymentId":"fixture-payment"}))).into_response()
}

#[tokio::test]
async fn arc_v2_receipt_keeps_the_wire_version_after_internal_normalization() {
    let v1: Value = serde_json::from_slice(&body(1)).unwrap();
    let r = &v1["paymentRequirements"];
    let v2 = Bytes::from(serde_json::to_vec(&json!({
        "x402Version":2,
        "paymentPayload":{"x402Version":2,"payload":v1["paymentPayload"]["payload"]},
        "resource":{"url":r["resource"],"description":r["description"],"mimeType":r["mimeType"]},
        "accepted":{"network":"eip155:5042002","scheme":"exact","asset":r["asset"],
            "amount":r["maxAmountRequired"],"payTo":r["payTo"],"maxTimeoutSeconds":180,"extra":r["extra"]}
    })).unwrap());
    TEST_SERVICE
        .scope(service_fixture(), async {
            let output = value(
                settle(
                    &MockFacilitator { invalid: false },
                    &headers(),
                    &v2,
                    async { success() },
                )
                .await,
            )
            .await;
            assert_eq!(output["receipt"]["x402Version"], 2);
            assert_eq!(output["receipt"]["status"], "confirmed");
            let replay = value(
                settle(
                    &MockFacilitator { invalid: false },
                    &headers(),
                    &v2,
                    async { panic!("second settlement") },
                )
                .await,
            )
            .await;
            assert_eq!(output["receipt"], replay["receipt"]);
        })
        .await;
}

#[tokio::test]
async fn concurrent_replicas_reserve_one_payment_and_replay_the_same_receipt() {
    let service = service_fixture();
    let sends = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let service = service.clone();
        let sends = sends.clone();
        tasks.push(tokio::spawn(TEST_SERVICE.scope(service, async move {
            settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async {
                    sends.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    success()
                },
            )
            .await
        })));
    }
    let mut receipt_id = None;
    for task in tasks {
        let output = value(task.await.unwrap()).await;
        let id = output["receipt"]["receiptId"].clone();
        if let Some(expected) = &receipt_id {
            assert_eq!(&id, expected);
        } else {
            receipt_id = Some(id);
        }
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let replay = TEST_SERVICE
        .scope(service, async {
            settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async { panic!("second broadcast") },
            )
            .await
        })
        .await;
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    let output = value(replay).await;
    assert_eq!(output["receipt"]["status"], "confirmed");
    assert_eq!(output["receipt"]["receiptId"], receipt_id.unwrap());
    assert_eq!(output["receipt"]["amount"], "1000");
    assert_eq!(output["receipt"]["network"], "eip155:5042002");
}

#[tokio::test]
async fn new_signature_same_purchase_and_changed_terms_never_charge_again() {
    let service = service_fixture();
    TEST_SERVICE
        .scope(service, async {
            let first = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async { success() },
            )
            .await;
            assert_eq!(first.status(), StatusCode::OK);
            let second = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(2),
                async { panic!("new authorization charged same purchase") },
            )
            .await;
            assert_eq!(second.status(), StatusCode::CONFLICT);
            let mut changed: Value = serde_json::from_slice(&body(1)).unwrap();
            changed["paymentRequirements"]["maxAmountRequired"] = json!("2000");
            let changed = Bytes::from(serde_json::to_vec(&changed).unwrap());
            let second = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &changed,
                async { panic!("changed terms charged") },
            )
            .await;
            assert_eq!(second.status(), StatusCode::CONFLICT);
        })
        .await;
}

#[tokio::test]
async fn unknown_survives_restart_and_has_no_refusal_or_second_send() {
    let first = service_fixture();
    TEST_SERVICE
        .scope(first.clone(), async {
            let response = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error":"rpc_timeout"})),
                    )
                        .into_response()
                },
            )
            .await;
            let output = value(response).await;
            assert_eq!(output["receipt"]["status"], "unknown");
            assert!(output["receipt"]["refusalReason"].is_null());
        })
        .await;
    // A new service instance uses only the durable store, no in-memory lock.
    let restarted = Arc::new(Service {
        store: first.store.clone(),
        signing_key: None,
    });
    TEST_SERVICE
        .scope(restarted, async {
            let replay = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async { panic!("uncertainty must not authorize another transfer") },
            )
            .await;
            assert_eq!(value(replay).await["receipt"]["status"], "unknown");
        })
        .await;
}

#[tokio::test]
async fn invalid_signature_cannot_reserve_someone_elses_authorization() {
    let service = service_fixture();
    TEST_SERVICE
        .scope(service.clone(), async {
            let rejected = settle(
                &MockFacilitator { invalid: true },
                &headers(),
                &body(1),
                async { panic!("invalid signature sent") },
            )
            .await;
            assert_eq!(value(rejected).await["receipt"]["status"], "rejected");
            let ok = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async { success() },
            )
            .await;
            assert_eq!(value(ok).await["receipt"]["status"], "confirmed");
        })
        .await;
}

#[test]
fn signature_binds_the_exact_receipt_and_canonical_hash_is_stable() {
    use ed25519_dalek::{Signature, Verifier};
    let service = service_fixture();
    let request = parse_request(&headers(), &body(1)).unwrap();
    let mut record = initial(
        &request,
        purchase_context(&headers()).unwrap().as_ref(),
        "verify",
    )
    .unwrap();
    service.sign(&mut record.receipt).unwrap();
    let proof = record.receipt.proof.as_ref().unwrap()["jws"]
        .as_str()
        .unwrap();
    let parts: Vec<_> = proof.split('.').collect();
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
    let message = format!("{}.{}", parts[0], parts[1]);
    service
        .signing_key
        .as_ref()
        .unwrap()
        .verifying_key()
        .verify(message.as_bytes(), &signature)
        .unwrap();
    let mut unsigned = serde_json::to_value(&record.receipt).unwrap();
    unsigned["proof"] = Value::Null;
    assert_eq!(
        URL_SAFE_NO_PAD.decode(parts[1]).unwrap(),
        canonical(&unsigned).unwrap().as_bytes()
    );
    assert!(canonical(&json!({"price":1.1})).is_err());
    assert_eq!(
        commitment("uvd-x402-request-v1", &json!({"b":"€","a":"1"})).unwrap(),
        commitment("uvd-x402-request-v1", &json!({"a":"1","b":"€"})).unwrap()
    );
}

#[test]
fn shared_python_typescript_vectors_validate_in_rust() {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/facilitator-receipts-v1.json"
    ))
    .unwrap();
    let bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(fixtures["jwks"]["keys"][0]["x"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let key = VerifyingKey::from_bytes(&bytes).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let receipt: FacilitatorReceipt = serde_json::from_value(case["receipt"].clone()).unwrap();
        assert_eq!(
            commitment(&receipt.request_hash_version, &receipt.request).unwrap(),
            receipt.request_hash
        );
        let network = Network::from_caip2(&receipt.network).unwrap();
        assert!(crate::network::exact_payment_tokens(network)
            .iter()
            .any(
                |t| t.address.to_string().eq_ignore_ascii_case(&receipt.asset) && t.decimals == 6
            ));
        let parts: Vec<_> = receipt.proof.as_ref().unwrap()["jws"]
            .as_str()
            .unwrap()
            .split('.')
            .collect();
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
        let mut unsigned = serde_json::to_value(&receipt).unwrap();
        unsigned["proof"] = Value::Null;
        assert_eq!(
            URL_SAFE_NO_PAD.decode(parts[1]).unwrap(),
            canonical(&unsigned).unwrap().as_bytes()
        );
    }
}

#[tokio::test]
async fn lookup_requires_the_private_capability_and_verify_recovers_consumed_nonce() {
    let service = service_fixture();
    TEST_SERVICE
        .scope(service, async {
            let paid = value(
                settle(
                    &MockFacilitator { invalid: false },
                    &headers(),
                    &body(1),
                    async { success() },
                )
                .await,
            )
            .await;
            let id = paid["receipt"]["receiptId"].as_str().unwrap().to_string();
            let verified = value(
                verify(&headers(), &body(1), async {
                    panic!("already-paid nonce reverified")
                })
                .await,
            )
            .await;
            assert_eq!(verified["isValid"], true);
            assert_eq!(verified["receipt"]["receiptId"], id);
            let denied = get(
                State(MockFacilitator { invalid: false }),
                Path(id.clone()),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(denied.status(), StatusCode::NOT_FOUND);
            let mut h = HeaderMap::new();
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {}", "cd".repeat(32))).unwrap(),
            );
            assert_eq!(
                get(
                    State(MockFacilitator { invalid: false }),
                    Path(id.clone()),
                    h.clone()
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
            h.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {}", "ab".repeat(32))).unwrap(),
            );
            let found = get(State(MockFacilitator { invalid: false }), Path(id), h).await;
            assert_eq!(value(found).await["receipt"], paid["receipt"]);
        })
        .await;
}

struct BrokenStore;

#[tokio::test]
async fn user_idempotency_keys_cannot_overwrite_receipt_rows_via_legacy_chains() {
    for key in [
        "receipt:v1:an-id",
        "receipt:auth:v1:an-id",
        " receipt:purchase:v1:an-id ",
    ] {
        let mut h = HeaderMap::new();
        h.insert("idempotency-key", HeaderValue::from_str(key).unwrap());
        let denied = settle(
            &MockFacilitator { invalid: false },
            &h,
            &Bytes::from_static(b"{}"),
            async { panic!("reserved key reached legacy writer") },
        )
        .await;
        assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    }
}
#[async_trait::async_trait]
impl store::Store for BrokenStore {
    async fn get(&self, _: &str) -> Result<Option<Record>> {
        Err("offline".into())
    }
    async fn reserve(&self, _: &Record, _: &[String]) -> Result<bool> {
        panic!("offline admission")
    }
    async fn save(&self, _: &Record, _: u64) -> Result<bool> {
        panic!("offline update")
    }
}
#[tokio::test]
async fn no_storage_no_broadcast() {
    let service = Arc::new(Service {
        store: Arc::new(BrokenStore),
        signing_key: None,
    });
    TEST_SERVICE
        .scope(service, async {
            let refused = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body(1),
                async { panic!("storage outage allowed a send") },
            )
            .await;
            assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        })
        .await;
}
