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
    fixture_record_on("arc-testnet", ARC_USDC)
}
pub(super) fn base_fixture_record() -> Record {
    TEST_CANDIDATE.sync_scope(Network::Base, || fixture_record_on("base", BASE_USDC))
}
fn fixture_record_on(network: &str, asset: &str) -> Record {
    initial(
        &parse_request(&headers(), &body_on(network, asset, 1)).unwrap(),
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
const ARC_USDC: &str = "0x3600000000000000000000000000000000000000";
const BASE_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const BASE_EURC: &str = "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42";
/// EVM networks driven through admission: the announced Arc networks and Base,
/// the candidate. The v1 name a body carries, the CAIP-2 id its receipt must
/// carry, and an exact asset on it.
const EVM_RECEIPT_NETWORKS: [(&str, &str, &str); 3] = [
    ("arc", "eip155:5042", ARC_USDC),
    ("arc-testnet", "eip155:5042002", ARC_USDC),
    ("base", "eip155:8453", BASE_USDC),
];
fn body(nonce: u8) -> Bytes {
    body_on("arc-testnet", ARC_USDC, nonce)
}
fn body_on(network: &str, asset: &str, nonce: u8) -> Bytes {
    Bytes::from(serde_json::to_vec(&json!({
        "x402Version":1,
        "paymentPayload":{"x402Version":1,"scheme":"exact","network":network,"payload":{
            "signature":format!("0x{}", "11".repeat(65)),"authorization":{
                "from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222",
                "value":"1000","validAfter":"0","validBefore":"2000000000","nonce":format!("0x{}",hex::encode([nonce;32]))}}},
        "paymentRequirements":{"scheme":"exact","network":network,"maxAmountRequired":"1000",
            "resource":"https://merchant.example/data","description":"receipt fixture","mimeType":"application/json",
            "payTo":"0x2222222222222222222222222222222222222222","maxTimeoutSeconds":60,
            "asset":asset,"extra":{"name":"USDC","version":"2"}}
    })).unwrap())
}
fn headers() -> HeaderMap {
    purchase("ab")
}
/// The fixture purchase context, with its capability built from `token`.
fn purchase(token: &str) -> HeaderMap {
    let context = PurchaseContext {
        purchase_id: "order-fixture".into(),
        access_token: token.repeat(32),
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
    success_on("arc-testnet")
}
fn success_on(network: &str) -> Response {
    (StatusCode::OK, Json(json!({"success":true,"network":network,"payer":"0x1111111111111111111111111111111111111111",
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
    for (network, caip2, asset) in EVM_RECEIPT_NETWORKS {
        let candidate = Network::from_caip2(caip2).unwrap();
        let service = service_fixture();
        let sends = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let service = service.clone();
            let sends = sends.clone();
            tasks.push(tokio::spawn(TEST_CANDIDATE.scope(
                candidate,
                TEST_SERVICE.scope(service, async move {
                    settle(
                        &MockFacilitator { invalid: false },
                        &headers(),
                        &body_on(network, asset, 1),
                        async {
                            sends.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            success_on(network)
                        },
                    )
                    .await
                }),
            )));
        }
        let mut receipt_id = None;
        for task in tasks {
            let output = value(task.await.unwrap()).await;
            let id = output["receipt"]["receiptId"].clone();
            if let Some(expected) = &receipt_id {
                assert_eq!(&id, expected, "{network}");
            } else {
                receipt_id = Some(id);
            }
        }
        assert_eq!(sends.load(Ordering::SeqCst), 1, "{network}");
        let replay = TEST_CANDIDATE
            .scope(
                candidate,
                TEST_SERVICE.scope(service, async {
                    settle(
                        &MockFacilitator { invalid: false },
                        &headers(),
                        &body_on(network, asset, 1),
                        async { panic!("second broadcast on {network}") },
                    )
                    .await
                }),
            )
            .await;
        assert_eq!(replay.headers()["idempotent-replayed"], "true", "{network}");
        let output = value(replay).await;
        assert_eq!(output["receipt"]["status"], "confirmed", "{network}");
        assert_eq!(
            output["receipt"]["receiptId"],
            receipt_id.unwrap(),
            "{network}"
        );
        assert_eq!(output["receipt"]["amount"], "1000", "{network}");
        assert_eq!(output["receipt"]["decimals"], 6, "{network}");
        assert_eq!(output["receipt"]["network"], caip2);
        // EVM addresses are lowercase in the receipt and its request hash,
        // whatever checksum casing the payment requirements used.
        assert_eq!(
            output["receipt"]["asset"],
            asset.to_lowercase(),
            "{network}"
        );
        assert_eq!(
            output["receipt"]["request"]["asset"],
            asset.to_lowercase(),
            "{network}"
        );
    }
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
async fn private_lookup_returns_unknown_receipts_after_a_payment_http_error() {
    for status in [StatusCode::BAD_REQUEST, StatusCode::BAD_GATEWAY] {
        TEST_SERVICE
            .scope(service_fixture(), async {
                let failed = settle(
                    &MockFacilitator { invalid: false },
                    &headers(),
                    &body(1),
                    async {
                        (status, Json(json!({"error":"sponsor_storage_unavailable"})))
                            .into_response()
                    },
                )
                .await;
                assert_eq!(failed.status(), status);
                let original = value(failed).await;
                assert_eq!(original["receipt"]["status"], "unknown");
                let id = original["receipt"]["receiptId"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let mut h = HeaderMap::new();
                h.insert(
                    "authorization",
                    HeaderValue::from_str(&format!("Bearer {}", "ab".repeat(32))).unwrap(),
                );
                let found = get(State(MockFacilitator { invalid: false }), Path(id), h).await;
                assert_eq!(found.status(), StatusCode::OK);
                assert_eq!(found.headers()["cache-control"], "no-store");
                assert_eq!(
                    value(found).await["receipt"],
                    original["receipt"],
                    "lookup must preserve the signed receipt"
                );
                let retry = settle(
                    &MockFacilitator { invalid: false },
                    &headers(),
                    &body(1),
                    async { panic!("lookup must not enable a second payment") },
                )
                .await;
                assert_eq!(
                    retry.status(),
                    status,
                    "POST keeps its original HTTP result"
                );
            })
            .await;
    }
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

#[tokio::test]
async fn base_escrow_and_refund_requests_keep_their_own_settlement_path() {
    TEST_CANDIDATE
        .scope(
            Network::Base,
            base_escrow_and_refund_stay_out_of_admission(),
        )
        .await;
}
async fn base_escrow_and_refund_stay_out_of_admission() {
    let exact: Value = serde_json::from_slice(&body_on("base", BASE_USDC, 1)).unwrap();
    assert!(parse_request(&HeaderMap::new(), &body_on("base", BASE_USDC, 1)).is_some());
    let mut refund = exact.clone();
    refund["paymentPayload"]["extensions"] = json!({"refund":{"info":{}}});
    let mut escrow = exact.clone();
    escrow["scheme"] = json!("escrow");
    let mut commerce = exact.clone();
    commerce["paymentPayload"]["scheme"] = json!("commerce");
    for (route, request) in [
        ("refund", refund),
        ("escrow", escrow),
        ("commerce", commerce),
    ] {
        let raw = Bytes::from(serde_json::to_vec(&request).unwrap());
        assert!(parse_request(&HeaderMap::new(), &raw).is_none(), "{route}");
        let store = Arc::new(store::MemoryStore::default());
        let service = Arc::new(Service {
            store: store.clone(),
            signing_key: Some(SigningKey::from_bytes(&[7; 32])),
        });
        TEST_SERVICE
            .scope(service, async {
                let own_path =
                    || async { (StatusCode::OK, Json(json!({"route": route}))).into_response() };
                let settled = settle(
                    &MockFacilitator { invalid: false },
                    &HeaderMap::new(),
                    &raw,
                    own_path(),
                )
                .await;
                assert_eq!(value(settled).await, json!({"route": route}));
                let verified = verify(&HeaderMap::new(), &raw, own_path()).await;
                assert_eq!(value(verified).await, json!({"route": route}));
            })
            .await;
        assert!(
            store.0.lock().await.is_empty(),
            "{route} reserved a receipt"
        );
    }
}

#[test]
fn capability_lists_exactly_the_supported_networks() {
    let mut listed: Vec<String> = capability()["networks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let mut expected: Vec<String> = Network::variants()
        .iter()
        .filter(|n| supported(**n))
        .map(|n| n.to_caip2())
        .collect();
    listed.sort();
    expected.sort();
    assert_eq!(listed, expected);
}

/// Base's exact path is ready and tested above, but it is announced on its
/// own: until then a Base settle is untouched, with no receipt and no replay.
#[tokio::test]
async fn base_is_not_admitted_until_it_is_announced() {
    assert!(!supported(Network::Base));
    assert!(!capability()["networks"]
        .as_array()
        .unwrap()
        .contains(&json!("eip155:8453")));
    let store = Arc::new(store::MemoryStore::default());
    let service = Arc::new(Service {
        store: store.clone(),
        signing_key: Some(SigningKey::from_bytes(&[7; 32])),
    });
    let sends = AtomicUsize::new(0);
    TEST_SERVICE
        .scope(service, async {
            for _ in 0..2 {
                let settled = settle(
                    &MockFacilitator { invalid: false },
                    &HeaderMap::new(),
                    &body_on("base", BASE_USDC, 1),
                    async {
                        sends.fetch_add(1, Ordering::SeqCst);
                        success_on("base")
                    },
                )
                .await;
                assert!(settled.headers().get("idempotent-replayed").is_none());
                assert!(value(settled).await.get("receipt").is_none());
            }
        })
        .await;
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert!(store.0.lock().await.is_empty());
}

/// The shared vectors are synthetic: fixed IDs, placeholder hashes and the
/// test key. None of them describes a payment.
fn synthetic_vector(network: &str, asset: &str, pay_to: &str, payer: &str) -> FacilitatorReceipt {
    let request = json!({"purchaseId":"purchase-fixture","method":"GET","url":"https://merchant.example/data",
        "bodySha256":hash(b""),"network":network,"scheme":"exact","asset":asset,"amount":"1000","payTo":pay_to});
    let mut receipt = FacilitatorReceipt {
        schema_version: 1,
        receipt_id: "00000000-0000-4000-8000-000000000001".into(),
        revision: 1,
        issuer: ISSUER.into(),
        issued_at: 1_700_000_000,
        operation: "verify".into(),
        purchase_id: Some("purchase-fixture".into()),
        network: network.into(),
        scheme: "exact".into(),
        x402_version: 2,
        asset: asset.into(),
        amount: "1000".into(),
        decimals: Some(6),
        pay_to: pay_to.into(),
        payer: Some(payer.into()),
        request_hash: commitment("uvd-x402-request-v1", &request).unwrap(),
        request_hash_version: "uvd-x402-request-v1".into(),
        request,
        payment_request_hash: "a".repeat(64),
        authorization_id: "b".repeat(64),
        status: "verified".into(),
        settlement: None,
        refusal_reason: None,
        diagnostic_code: None,
        retry: json!({"action":"none"}),
        proof: None,
    };
    service_fixture().sign(&mut receipt).unwrap();
    receipt
}

#[test]
fn shared_vectors_are_synthetic_receipts_and_cover_base() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/facilitator-receipts-v1.json"
    ))
    .unwrap();
    let cases = fixtures["cases"].as_array().unwrap();
    for case in cases {
        let r = &case["receipt"];
        let field = |name: &str| r[name].as_str().unwrap();
        assert_eq!(case["network"], r["network"]);
        assert_eq!(
            serde_json::to_value(synthetic_vector(
                field("network"),
                field("asset"),
                field("payTo"),
                field("payer")
            ))
            .unwrap(),
            *r,
            "{} {}",
            case["network"],
            case["symbol"]
        );
    }
    let evm_pay_to = "0x2222222222222222222222222222222222222222";
    let evm_payer = "0x1111111111111111111111111111111111111111";
    for (symbol, asset) in [("USDC", BASE_USDC), ("EURC", BASE_EURC)] {
        let expected =
            synthetic_vector("eip155:8453", &asset.to_lowercase(), evm_pay_to, evm_payer);
        assert!(
            cases.iter().any(|c| c["symbol"] == symbol
                && c["receipt"] == serde_json::to_value(&expected).unwrap()),
            "missing Base {symbol} vector"
        );
    }
}

// --- Replays of an admitted authorization -----------------------------------
//
// The original answer goes back only to the binding that admitted the payment:
// its X-UVD-Purchase capability or its Idempotency-Key. A bare resend of the
// signed payment learns the outcome from the receipt but is never answered as
// a (repeated) success. Every network in `supported()` is covered, plus Base as
// the candidate.

#[cfg(feature = "hedera")]
fn hedera_body(network: &str, asset: &str) -> Bytes {
    let vector: Value = serde_json::from_str(include_str!(
        "../../tests/hedera-e2e/vectors/03-hts-usdc-ecdsa-multinode.json"
    ))
    .unwrap();
    let accepted = json!({"scheme":"exact","network":network,"asset":asset,"amount":"150000",
        "payTo":"0.0.2002","maxTimeoutSeconds":60,"extra":{"feePayer":"0.0.3003"}});
    Bytes::from(
        serde_json::to_vec(&json!({"x402Version":2,
            "paymentPayload":{"x402Version":2,"accepted":accepted,
                "payload":{"transaction":vector["payload"]["transaction"]},
                "resource":{"url":"https://merchant.example/data","description":"receipt fixture",
                    "mimeType":"application/json"}},
            "accepted":accepted}))
        .unwrap(),
    )
}

/// CAIP-2 id and a synthetic request for one authorization, per network.
fn admitted_networks() -> Vec<(&'static str, Bytes)> {
    #[allow(unused_mut)]
    let mut cases = vec![
        ("eip155:5042", body_on("arc", ARC_USDC, 1)),
        ("eip155:5042002", body_on("arc-testnet", ARC_USDC, 1)),
        ("eip155:8453", body_on("base", BASE_USDC, 1)),
    ];
    #[cfg(feature = "hedera")]
    cases.extend([
        (
            "hedera:mainnet",
            hedera_body("hedera:mainnet", "0.0.456858"),
        ),
        (
            "hedera:testnet",
            hedera_body("hedera:testnet", "0.0.429274"),
        ),
    ]);
    cases
}

/// Runs `f` with the receipt service and `network` admitted (Base is only a
/// candidate; the others are already in `supported()`).
async fn admitted<F: Future>(network: &str, service: Arc<Service>, f: F) -> F::Output {
    let network = Network::from_caip2(network).unwrap();
    TEST_CANDIDATE
        .scope(network, TEST_SERVICE.scope(service, f))
        .await
}

fn keyed(key: &'static str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("idempotency-key", HeaderValue::from_static(key));
    h
}

async fn settle_again(h: &HeaderMap, body: &Bytes) -> Response {
    settle(&MockFacilitator { invalid: false }, h, body, async {
        panic!("an admitted authorization reached the chain again")
    })
    .await
}

async fn verify_again(h: &HeaderMap, body: &Bytes) -> Value {
    value(
        verify(h, body, async {
            panic!("an admitted authorization was simulated again")
        })
        .await,
    )
    .await
}

#[test]
fn every_network_in_supported_is_covered_by_the_replay_tests() {
    let covered: Vec<_> = admitted_networks().into_iter().map(|(n, _)| n).collect();
    for network in Network::variants().iter().filter(|n| supported(**n)) {
        assert!(covered.contains(&network.to_caip2().as_str()), "{network}");
    }
    assert!(covered.contains(&"eip155:8453"));
}

#[tokio::test]
async fn a_bare_resend_of_a_settled_authorization_is_refused_with_its_receipt() {
    for (network, body) in admitted_networks() {
        admitted(network, service_fixture(), async {
            let paid = settle(
                &MockFacilitator { invalid: false },
                &HeaderMap::new(),
                &body,
                async { success_on(network) },
            )
            .await;
            assert_eq!(paid.status(), StatusCode::OK, "{network}");
            let paid = value(paid).await;
            assert_eq!(paid["receipt"]["status"], "confirmed", "{network}");
            // Nothing binds a bare admission: no key, no capability. An
            // unrelated key does not bind it either.
            for h in [HeaderMap::new(), keyed("someone-else")] {
                let refused = settle_again(&h, &body).await;
                assert_eq!(refused.status(), StatusCode::CONFLICT, "{network}");
                assert!(!refused.headers().contains_key("idempotent-replayed"));
                assert_eq!(refused.headers()["cache-control"], "no-store");
                let refused = value(refused).await;
                assert_eq!(refused["error"], "authorization_already_settled");
                assert_eq!(refused["success"], false);
                assert!(refused.get("transaction").is_none());
                // The payer can still prove the payment from the receipt.
                assert_eq!(refused["receipt"], paid["receipt"], "{network}");
            }
            let verified = verify_again(&HeaderMap::new(), &body).await;
            assert_eq!(verified["isValid"], false, "{network}");
            assert_eq!(verified["invalidReason"], "authorization_already_settled");
            assert_eq!(
                verified["receipt"]["receiptId"],
                paid["receipt"]["receiptId"]
            );
        })
        .await;
    }
}

#[tokio::test]
async fn the_binding_that_admitted_a_payment_recovers_its_lost_response() {
    for (network, body) in admitted_networks() {
        admitted(network, service_fixture(), async {
            // Idempotency-Key: the response is lost, the same request with the
            // same key gets it back.
            let original = settle(
                &MockFacilitator { invalid: false },
                &keyed("purchase-7"),
                &body,
                async { success_on(network) },
            )
            .await;
            let original = value(original).await;
            let replay = settle_again(&keyed("purchase-7"), &body).await;
            assert_eq!(replay.status(), StatusCode::OK, "{network}");
            assert_eq!(replay.headers()["idempotent-replayed"], "true");
            assert_eq!(value(replay).await, original, "{network}");
            let verified = verify_again(&keyed("purchase-7"), &body).await;
            assert_eq!(verified["isValid"], true, "{network}");
            assert_eq!(
                verified["receipt"]["receiptId"],
                original["receipt"]["receiptId"]
            );
            for h in [HeaderMap::new(), keyed("purchase-8")] {
                let refused = settle_again(&h, &body).await;
                assert_eq!(refused.status(), StatusCode::CONFLICT, "{network}");
                assert_eq!(
                    value(refused).await["error"],
                    "authorization_already_settled"
                );
            }
        })
        .await;
        admitted(network, service_fixture(), async {
            // X-UVD-Purchase: the same capability recovers; no capability, or
            // another one, is the conflict it is today and never sees the
            // private receipt.
            let original = settle(
                &MockFacilitator { invalid: false },
                &headers(),
                &body,
                async { success_on(network) },
            )
            .await;
            let original = value(original).await;
            let replay = settle_again(&headers(), &body).await;
            assert_eq!(replay.status(), StatusCode::OK, "{network}");
            assert_eq!(replay.headers()["idempotent-replayed"], "true");
            assert_eq!(value(replay).await, original, "{network}");
            for h in [HeaderMap::new(), purchase("cd")] {
                let refused = settle_again(&h, &body).await;
                assert_eq!(refused.status(), StatusCode::CONFLICT, "{network}");
                let refused = value(refused).await;
                assert_eq!(refused["error"], "receipt_request_conflict", "{network}");
                assert!(refused.get("receipt").is_none());
                let verified = verify_again(&h, &body).await;
                assert_eq!(verified["isValid"], false, "{network}");
                assert_eq!(verified["invalidReason"], "authorization_already_settled");
                assert!(verified.get("receipt").is_none());
            }
        })
        .await;
    }
}

#[tokio::test]
async fn a_bare_resend_in_flight_is_refused_and_only_the_binding_gets_the_202() {
    for (network, body) in admitted_networks() {
        let service = service_fixture();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let owner = tokio::spawn({
            let (service, body) = (service.clone(), body.clone());
            admitted(network, service, async move {
                settle(
                    &MockFacilitator { invalid: false },
                    &keyed("in-flight"),
                    &body,
                    async move {
                        released.await.unwrap();
                        success_on(network)
                    },
                )
                .await
            })
        });
        admitted(network, service.clone(), async {
            let request = parse_request(&HeaderMap::new(), &body).unwrap();
            let id = initial(&request, None, "settle")
                .unwrap()
                .receipt
                .authorization_id;
            while service
                .store
                .get(&format!("receipt:auth:v1:{id}"))
                .await
                .unwrap()
                .is_none()
            {
                tokio::task::yield_now().await;
            }
            let running = settle_again(&keyed("in-flight"), &body).await;
            assert_eq!(running.status(), StatusCode::ACCEPTED, "{network}");
            assert_eq!(running.headers()["idempotent-replayed"], "true");
            assert_eq!(value(running).await["error"], "settlement_in_progress");
            let refused = settle_again(&HeaderMap::new(), &body).await;
            assert_eq!(refused.status(), StatusCode::CONFLICT, "{network}");
            assert!(!refused.headers().contains_key("idempotent-replayed"));
            let refused = value(refused).await;
            assert_eq!(refused["error"], "authorization_in_flight", "{network}");
            assert_eq!(refused["receipt"]["status"], "unknown");
            let verified = verify_again(&HeaderMap::new(), &body).await;
            assert_eq!(verified["isValid"], false, "{network}");
            assert_eq!(verified["invalidReason"], "authorization_in_flight");
            release.send(()).unwrap();
            assert_eq!(owner.await.unwrap().status(), StatusCode::OK, "{network}");
            let refused = settle_again(&HeaderMap::new(), &body).await;
            assert_eq!(
                value(refused).await["error"],
                "authorization_already_settled"
            );
        })
        .await;
    }
}

#[tokio::test]
async fn an_uncertain_outcome_is_replayed_to_its_binding_and_refused_bare() {
    for (network, body) in admitted_networks() {
        admitted(network, service_fixture(), async {
            let uncertain = settle(
                &MockFacilitator { invalid: false },
                &keyed("uncertain"),
                &body,
                async {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error":"rpc_timeout"})),
                    )
                        .into_response()
                },
            )
            .await;
            assert_eq!(uncertain.status(), StatusCode::BAD_GATEWAY);
            let replay = settle_again(&keyed("uncertain"), &body).await;
            assert_eq!(replay.status(), StatusCode::BAD_GATEWAY, "{network}");
            assert_eq!(replay.headers()["idempotent-replayed"], "true");
            let refused = settle_again(&HeaderMap::new(), &body).await;
            assert_eq!(refused.status(), StatusCode::CONFLICT, "{network}");
            assert_eq!(value(refused).await["error"], "authorization_in_flight");
        })
        .await;
    }
}

#[tokio::test]
async fn concurrent_bare_resends_admit_one_payment_and_one_success() {
    for (network, body) in admitted_networks() {
        let service = service_fixture();
        let sends = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let (service, sends, body) = (service.clone(), sends.clone(), body.clone());
            tasks.push(tokio::spawn(admitted(network, service, async move {
                settle(
                    &MockFacilitator { invalid: false },
                    &HeaderMap::new(),
                    &body,
                    async {
                        sends.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        success_on(network)
                    },
                )
                .await
            })));
        }
        let mut successes = 0;
        for task in tasks {
            let response = task.await.unwrap();
            if response.status() == StatusCode::OK {
                successes += 1;
                assert_eq!(value(response).await["success"], true);
            } else {
                assert_eq!(response.status(), StatusCode::CONFLICT, "{network}");
                let error = value(response).await["error"].clone();
                assert!(
                    error == "authorization_in_flight" || error == "authorization_already_settled",
                    "{network}: {error}"
                );
            }
        }
        assert_eq!(sends.load(Ordering::SeqCst), 1, "{network}");
        assert_eq!(successes, 1, "{network}");
    }
}
