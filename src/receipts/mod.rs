//! Portable facilitator receipts for exact Arc and native Hedera payments.
//! A receipt attests payment state, never merchant delivery. Admission is
//! atomically reserved before the provider can broadcast. Uncertainty is sticky:
//! only chain evidence can turn it into confirmation, never a new authorization.
pub mod store;

use crate::{
    chain::NetworkProvider,
    facilitator::Facilitator,
    network::{Network, NetworkFamily},
    provider_cache::{HasProviderMap, ProviderMap},
    types::{ExactPaymentPayload, Scheme, VerifyRequest, VerifyResponse},
    types_v2::VerifyRequestEnvelope,
};
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ed25519_dalek::{Signer, SigningKey};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, String>;
const ISSUER: &str = "https://facilitator.ultravioletadao.xyz";
static SERVICE: OnceCell<Arc<Service>> = OnceCell::new();
tokio::task_local! { static ACTIVE: Arc<tokio::sync::Mutex<Record>>; }
#[cfg(test)]
tokio::task_local! { static TEST_SERVICE: Arc<Service>; }
#[cfg(test)]
tokio::task_local! { static TEST_LEGACY_RECORD: crate::idempotency_store::IdempotencyRecord; }
// A network the tests drive through admission before it is announced.
#[cfg(test)]
tokio::task_local! { static TEST_CANDIDATE: Network; }
fn service() -> Option<Arc<Service>> {
    #[cfg(test)]
    if let Ok(service) = TEST_SERVICE.try_with(Arc::clone) {
        return Some(service);
    }
    SERVICE.get().cloned()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FacilitatorReceipt {
    pub schema_version: u8,
    pub receipt_id: String,
    pub revision: u64,
    pub issuer: String,
    pub issued_at: u64,
    pub operation: String,
    pub purchase_id: Option<String>,
    pub network: String,
    pub scheme: String,
    pub x402_version: u8,
    pub asset: String,
    pub amount: String,
    pub decimals: Option<u8>,
    pub pay_to: String,
    pub payer: Option<String>,
    pub request_hash: String,
    pub request_hash_version: String,
    pub request: Value,
    pub payment_request_hash: String,
    pub authorization_id: String,
    pub status: String,
    pub settlement: Option<Value>,
    pub refusal_reason: Option<String>,
    pub diagnostic_code: Option<String>,
    pub retry: Value,
    pub proof: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub receipt: FacilitatorReceipt,
    pub token_hash: String,
    pub response: Value,
    pub http_status: u16,
    // Private recovery material, never serialized into a public receipt.
    pub prepared: Option<Value>,
    pub authorization_expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurchaseContext {
    pub purchase_id: String,
    pub access_token: String,
    pub method: String,
    pub url: String,
    pub body_sha256: String,
}

pub struct Service {
    pub store: Arc<dyn store::Store>,
    signing_key: Option<SigningKey>,
}

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// JCS for this schema: objects have ASCII keys; arbitrary text is a string,
/// amounts are strings, and the only JSON numbers are bounded integers.
/// Reject floats and non-ASCII keys rather than silently implement partial JCS.
pub fn canonical(value: &Value) -> Result<String> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let mut fields = Vec::new();
            for key in keys {
                if !key.is_ascii() {
                    return Err("non_ascii_canonical_key".into());
                }
                fields.push(format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    canonical(&map[key])?
                ));
            }
            Ok(format!("{{{}}}", fields.join(",")))
        }
        Value::Array(values) => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(canonical)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        )),
        Value::Number(n) if n.as_u64().is_none_or(|n| n > 9_007_199_254_740_991) => {
            Err("unsupported_canonical_number".into())
        }
        _ => serde_json::to_string(value).map_err(|_| "canonical_encoding_failed".into()),
    }
}

pub fn commitment(domain: &str, value: &Value) -> Result<String> {
    Ok(hash(format!("{domain}\n{}", canonical(value)?).as_bytes()))
}

/// Networks are announced one at a time: `capability()` lists exactly these.
/// Base's exact path is exercised by the tests but not admitted yet.
pub fn supported(network: Network) -> bool {
    #[cfg(test)]
    if TEST_CANDIDATE
        .try_with(|candidate| *candidate == network)
        .unwrap_or(false)
    {
        return true;
    }
    matches!(network, Network::Arc | Network::ArcTestnet) || network.is_hedera()
}

/// `post_settle` hands these to their own settlement paths before the exact
/// path runs: x402r escrow/commerce, the `refund` extension, upto and FHE.
/// Their inner requirements can still read `exact`; they are never admitted.
fn alternative_route(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return true;
    };
    let payload = &value["paymentPayload"];
    let not_exact = |scheme: &Value| scheme.as_str().is_some_and(|s| s != "exact");
    not_exact(&value["scheme"])
        || not_exact(&payload["scheme"])
        || not_exact(&payload["accepted"]["scheme"])
        || payload["extensions"].get("refund").is_some()
}

pub fn parse_request(headers: &HeaderMap, raw: &Bytes) -> Option<VerifyRequest> {
    let decoded;
    let bytes = if let Some(header) = headers.get("payment-signature") {
        decoded = STANDARD.decode(header.as_bytes()).ok()?;
        decoded.as_slice()
    } else {
        raw.as_ref()
    };
    let envelope: VerifyRequestEnvelope = serde_json::from_slice(bytes).ok()?;
    if alternative_route(bytes) {
        return None;
    }
    let mut request = envelope.to_v1().ok()?;
    // to_v1 normalizes the processing envelope, including its version field.
    // A receipt must attest the protocol version actually received.
    request.x402_version = envelope.version();
    (supported(request.network()) && request.payment_requirements.scheme == Scheme::Exact)
        .then_some(request)
}

pub fn purchase_context(headers: &HeaderMap) -> Result<Option<PurchaseContext>> {
    let Some(header) = headers.get("x-uvd-purchase") else {
        return Ok(None);
    };
    if header.len() > 8192 {
        return Err("invalid_receipt_context".into());
    }
    let bytes = STANDARD
        .decode(header.as_bytes())
        .map_err(|_| "invalid_receipt_context")?;
    let context: PurchaseContext =
        serde_json::from_slice(&bytes).map_err(|_| "invalid_receipt_context")?;
    let token = hex::decode(&context.access_token).map_err(|_| "invalid_receipt_context")?;
    let body_hash = hex::decode(&context.body_sha256).map_err(|_| "invalid_receipt_context")?;
    let url = url::Url::parse(&context.url).map_err(|_| "invalid_receipt_context")?;
    if token.len() != 32
        || body_hash.len() != 32
        || context.purchase_id.is_empty()
        || context.purchase_id.len() > 128
        || context.method.is_empty()
        || context.method.len() > 16
        || !context.method.bytes().all(|b| b.is_ascii_uppercase())
        || !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || context.url.len() > 2048
    {
        return Err("invalid_receipt_context".into());
    }
    Ok(Some(context))
}

fn authorization(request: &VerifyRequest) -> Result<String> {
    let network = request.network().to_caip2();
    match &request.payment_payload.payload {
        ExactPaymentPayload::Evm(evm) => commitment(
            "uvd-x402-authorization-v1",
            &json!({
                "network": network, "asset": request.payment_requirements.asset.to_string().to_lowercase(),
                "payer": evm.authorization.from.to_string().to_lowercase(),
                "nonce": serde_json::to_value(&evm.authorization.nonce).map_err(|_| "invalid_authorization")?
            }),
        ),
        #[cfg(feature = "hedera")]
        ExactPaymentPayload::Hedera(payload) => {
            let decoded = crate::chain::hedera::codec::Decoded::from_base64(&payload.transaction)?;
            let tx = decoded
                .variants
                .first()
                .and_then(|v| v.body.transaction_id.as_ref())
                .ok_or("missing_transaction_id")?;
            let account = crate::chain::hedera::codec::account(tx.account_id.as_ref())?;
            let start = tx
                .transaction_valid_start
                .as_ref()
                .ok_or("missing_transaction_start")?;
            commitment(
                "uvd-x402-authorization-v1",
                &json!({"network":network,
                "transactionId":format!("{account}@{}.{:09}", start.seconds, start.nanos)}),
            )
        }
        _ => Err("receipt_network_not_supported".into()),
    }
}

fn initial(
    request: &VerifyRequest,
    context: Option<&PurchaseContext>,
    operation: &str,
) -> Result<Record> {
    let terms = &request.payment_requirements;
    let evm = matches!(NetworkFamily::from(request.network()), NetworkFamily::Evm);
    let normalize = |s: String| if evm { s.to_lowercase() } else { s };
    let descriptor = json!({
        "purchaseId":context.map(|c| &c.purchase_id), "method":context.map(|c| &c.method),
        "url":context.map(|c| c.url.as_str()).unwrap_or(terms.resource.as_str()),
        "bodySha256":context.map(|c| &c.body_sha256),
        "network":terms.network.to_caip2(), "scheme":"exact", "asset":normalize(terms.asset.to_string()),
        "amount":terms.max_amount_required.to_string(), "payTo":normalize(terms.pay_to.to_string()),
    });
    let payment = serde_json::to_value(request).map_err(|_| "invalid_payment")?;
    let decimals = crate::network::exact_payment_tokens(terms.network)
        .iter()
        .find(|t| {
            t.address
                .to_string()
                .eq_ignore_ascii_case(&terms.asset.to_string())
        })
        .map(|t| t.decimals);
    Ok(Record {
        receipt: FacilitatorReceipt {
            schema_version: 1,
            receipt_id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            issuer: ISSUER.into(),
            issued_at: now(),
            operation: operation.into(),
            purchase_id: context.map(|c| c.purchase_id.clone()),
            network: terms.network.to_caip2(),
            scheme: "exact".into(),
            x402_version: request.x402_version.as_u8(),
            asset: normalize(terms.asset.to_string()),
            amount: terms.max_amount_required.to_string(),
            decimals,
            pay_to: normalize(terms.pay_to.to_string()),
            payer: None,
            request_hash: commitment("uvd-x402-request-v1", &descriptor)?,
            request_hash_version: "uvd-x402-request-v1".into(),
            request: descriptor,
            payment_request_hash: commitment("uvd-x402-payment-v1", &payment)?,
            authorization_id: authorization(request)?,
            status: "unknown".into(),
            settlement: None,
            refusal_reason: None,
            diagnostic_code: Some("operation_in_progress".into()),
            retry: json!({"action":"poll","afterSeconds":2}),
            proof: None,
        },
        token_hash: context
            .map(|c| hash(c.access_token.as_bytes()))
            .unwrap_or_default(),
        response: json!({"success":false,"error":"settlement_in_progress","retryable":true,"safeToReplay":false}),
        http_status: 202,
        prepared: None,
        authorization_expires_at: match &request.payment_payload.payload {
            ExactPaymentPayload::Evm(evm) => evm.authorization.valid_before.seconds_since_epoch(),
            _ => 0,
        },
    })
}

impl Service {
    fn sign(&self, receipt: &mut FacilitatorReceipt) -> Result<()> {
        receipt.proof = None;
        if let Some(key) = &self.signing_key {
            let kid = hash(key.verifying_key().as_bytes());
            let protected = URL_SAFE_NO_PAD.encode(canonical(
                &json!({"alg":"EdDSA","kid":kid,"typ":"uvd-facilitator-receipt+jws"}),
            )?);
            let payload = URL_SAFE_NO_PAD.encode(canonical(
                &serde_json::to_value(&receipt).map_err(|_| "receipt_encoding_failed")?,
            )?);
            let signed = format!("{protected}.{payload}");
            receipt.proof = Some(
                json!({"type":"jws","jws":format!("{signed}.{}", URL_SAFE_NO_PAD.encode(key.sign(signed.as_bytes()).to_bytes()))}),
            );
        }
        Ok(())
    }

    async fn save(&self, record: &mut Record) -> Result<()> {
        let previous = record.receipt.revision;
        let mut next = record.clone();
        next.receipt.revision += 1;
        next.receipt.issued_at = now();
        self.sign(&mut next.receipt)?;
        if !self.store.save(&next, previous).await? {
            return Err("receipt_revision_conflict".into());
        }
        *record = next;
        Ok(())
    }
}

pub async fn init() -> Result<()> {
    let Some(store) = store::DynamoStore::from_env().await else {
        return Ok(());
    };
    let signing_key = match std::env::var("RECEIPT_SIGNING_KEY") {
        Ok(secret) => {
            let decoded = hex::decode(secret.trim()).map_err(|_| "invalid_receipt_signing_key")?;
            let bytes: [u8; 32] = decoded
                .try_into()
                .map_err(|_| "invalid_receipt_signing_key")?;
            Some(SigningKey::from_bytes(&bytes))
        }
        Err(_) => None,
    };
    SERVICE
        .set(Arc::new(Service { store, signing_key }))
        .map_err(|_| "receipt_service_already_initialized".into())
}

pub async fn keys() -> Json<Value> {
    let keys: Vec<Value> = SERVICE.get().and_then(|s| s.signing_key.as_ref()).map(|key| json!({
        "kty":"OKP","crv":"Ed25519","use":"sig","alg":"EdDSA", "kid":hash(key.verifying_key().as_bytes()),
        "x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())
    })).into_iter().collect();
    Json(json!({"keys":keys}))
}

pub fn capability() -> Value {
    json!({"schemaVersion":1,"available":SERVICE.get().is_some(),"networks":["eip155:5042","eip155:5042002","hedera:mainnet","hedera:testnet"],
        "schemes":["exact"],"contextHeader":"X-UVD-Purchase", "lookup":"/receipts/{receiptId}",
        "proof":if SERVICE.get().is_some_and(|s| s.signing_key.is_some()) {"jws-ed25519"} else {"https"},
        "keys":"/.well-known/receipt-keys.json"})
}

/// Keep Swagger and the root OpenAPI alias on the same additive contract.
pub fn document_api(api: &mut utoipa::openapi::OpenApi) {
    let mut doc = serde_json::to_value(&*api).expect("OpenAPI serializes");
    let mut schema: Value = serde_json::from_str(include_str!(
        "../../static/schemas/facilitator-receipt-v1.json"
    ))
    .expect("receipt schema");
    schema.as_object_mut().unwrap().remove("$id");
    schema.as_object_mut().unwrap().remove("$schema");
    doc["components"]["schemas"]["FacilitatorReceipt"] = schema;
    for path in ["/verify", "/settle"] {
        let operation = &mut doc["paths"][path]["post"];
        let description = operation["description"].as_str().unwrap_or("").to_owned();
        operation["description"] = json!(format!("{description}\n\nArc exact (USDC/EURC) and native Hedera USDC include an additive `receipt`: network, asset, atomic amount, payTo, requestHash, settlement ID, status and refusalReason. See /schemas/facilitator-receipt-v1.json. Send X-UVD-Purchase (base64 JSON with purchaseId, secret accessToken, method, url, bodySha256) for private lookup and restart-safe purchase retries. The merchant must validate the actual HTTP request. Preserve the same context and authorization after uncertainty; never sign a replacement. Payment confirmation does not prove merchant delivery. Other networks retain their existing responses."));
        let params = operation
            .as_object_mut()
            .unwrap()
            .entry("parameters")
            .or_insert_with(|| json!([]));
        params.as_array_mut().unwrap().push(json!({"name":"X-UVD-Purchase","in":"header","required":false,"schema":{"type":"string"},"description":"Private purchase context; never log its accessToken or signed authorization."}));
        operation["responses"]["200"]["content"]["application/json"]["schema"] = json!({"type":"object","properties":{"receipt":{"$ref":"#/components/schemas/FacilitatorReceipt"}},"additionalProperties":true});
        if path == "/settle" {
            operation["responses"]["202"] = json!({"description":"Reserved payment remains pending or unknown. Returned to the X-UVD-Purchase or Idempotency-Key that admitted it. Poll the private receipt or retry exactly the same request; no new payment is admitted."});
            operation["responses"]["409"] = json!({"description":"Purchase, authorization or idempotency key conflicts with the original request, or the authorization was already admitted and this request lacks the X-UVD-Purchase or Idempotency-Key that admitted it: `authorization_already_settled` or `authorization_in_flight`, with the receipt when the payment has no purchase context. No replacement payment is admitted and no success is repeated."});
        }
    }
    for (path, summary) in [
        ("/receipts", "Discover facilitator receipt capabilities"),
        (
            "/.well-known/receipt-keys.json",
            "Trusted issuer public Ed25519 JWK set",
        ),
        (
            "/schemas/facilitator-receipt-v1.json",
            "Portable facilitator receipt JSON Schema",
        ),
    ] {
        doc["paths"][path] = json!({"get":{"tags":["Core"],"summary":summary,"responses":{"200":{"description":summary,"content":{"application/json":{"schema":{"type":"object"}}}}}}});
    }
    doc["paths"]["/receipts/{receiptId}"] = json!({"get":{"tags":["Core"],"summary":"Read and reconcile a private receipt; never broadcasts a payment",
        "parameters":[{"name":"receiptId","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},
            {"name":"Authorization","in":"header","required":true,"schema":{"type":"string"},"description":"Bearer followed by the original purchase accessToken"}],
        "responses":{"200":{"description":"Latest durable receipt","content":{"application/json":{"schema":{"type":"object","properties":{"receipt":{"$ref":"#/components/schemas/FacilitatorReceipt"}}}}}},
            "404":{"description":"Unknown receipt or incorrect capability"},"503":{"description":"Receipt storage unavailable"}}}});
    *api = serde_json::from_value(doc).expect("valid receipt OpenAPI additions");
}

fn response(record: &Record, replayed: bool) -> Response {
    let mut body = record.response.clone();
    body["receipt"] = serde_json::to_value(&record.receipt).unwrap();
    let mut response = (
        StatusCode::from_u16(record.http_status).unwrap_or(StatusCode::BAD_GATEWAY),
        Json(body),
    )
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    if replayed {
        response
            .headers_mut()
            .insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    response
}

fn failure(code: &str, status: StatusCode) -> Response {
    (status, Json(json!({"success":false,"error":code,"retryable":status.is_server_error(),"safeToReplay":false}))).into_response()
}

async fn finish(service: &Service, record: &mut Record, raw: Response, durable: bool) -> Response {
    let durable_before = record.clone();
    let (parts, body) = raw.into_parts();
    let bytes = match to_bytes(body, 65536).await {
        Ok(bytes) => bytes,
        Err(_) => return failure("receipt_response_unreadable", StatusCode::BAD_GATEWAY),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    record.response = value.clone();
    record.http_status = parts.status.as_u16();
    record.receipt.payer = value
        .get("payer")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(record.receipt.payer.clone());
    let tx = value
        .get("transaction")
        .or_else(|| value.get("transactionHash"))
        .or_else(|| value.get("transaction_hash"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if let Some(tx) = tx {
        record.receipt.settlement = Some(
            json!({"id":tx, "idType":if record.receipt.network.starts_with("hedera:") {"hedera-transaction-id"} else {"evm-transaction-hash"}, "paymentId":value.get("paymentId")}),
        );
    }
    let verify = record.receipt.operation == "verify";
    let confirmed =
        !verify && value.get("success").and_then(Value::as_bool) == Some(true) && tx.is_some();
    let verified = verify && value.get("isValid").and_then(Value::as_bool) == Some(true);
    // Only an explicit verification verdict or mined failure is a refusal.
    // HTTP 4xx/5xx, exceptions and timeouts cannot prove no funds moved.
    let refusal = if parts.status.is_success()
        && value.get("isValid").and_then(Value::as_bool) == Some(false)
    {
        value.get("invalidReason").and_then(Value::as_str)
    } else if !verify
        && parts.status.is_success()
        && value.get("success").and_then(Value::as_bool) == Some(false)
    {
        value.get("errorReason").and_then(Value::as_str)
    } else {
        None
    };
    record.receipt.status = if confirmed {
        "confirmed"
    } else if verified {
        "verified"
    } else if refusal.is_some() {
        "rejected"
    } else {
        "unknown"
    }
    .into();
    record.receipt.refusal_reason = refusal.map(str::to_owned);
    record.receipt.diagnostic_code = (record.receipt.status == "unknown").then(|| {
        value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("settlement_unconfirmed")
            .to_owned()
    });
    record.receipt.retry =
        json!({"action":if record.receipt.status == "unknown" {"poll"} else {"none"}});
    if matches!(record.receipt.status.as_str(), "confirmed" | "rejected") {
        if let Some(prepared) = record.prepared.as_mut().and_then(Value::as_object_mut) {
            prepared.remove("signedTransaction");
        }
    }
    let saved = if durable {
        service.save(record).await
    } else {
        service.sign(&mut record.receipt)
    };
    if saved.is_err() && durable {
        let stable = service
            .store
            .get(&format!("receipt:v1:{}", record.receipt.receipt_id))
            .await
            .ok()
            .flatten()
            .unwrap_or(durable_before);
        let mut body = record.response.clone();
        body["receipt"] = serde_json::to_value(stable.receipt).unwrap();
        body["receiptWarning"] = json!("receipt_persistence_unavailable");
        return (parts.status, Json(body)).into_response();
    }
    let mut result = response(record, false);
    for (key, val) in &parts.headers {
        if key != "content-length" && key != "content-type" {
            result.headers_mut().insert(key.clone(), val.clone());
        }
    }
    result
}

pub async fn verify<F: Future<Output = Response>>(
    headers: &HeaderMap,
    raw: &Bytes,
    call: F,
) -> Response {
    let Some(service) = service() else {
        return call.await;
    };
    let Some(request) = parse_request(headers, raw) else {
        return call.await;
    };
    let context = match purchase_context(headers) {
        Ok(c) => c,
        Err(e) => return failure(&e, StatusCode::BAD_REQUEST),
    };
    let mut record = match initial(&request, context.as_ref(), "verify") {
        Ok(r) => r,
        Err(_) => return call.await,
    };
    if let Ok(Some(existing)) = service
        .store
        .get(&format!(
            "receipt:auth:v1:{}",
            record.receipt.authorization_id
        ))
        .await
    {
        let same = same_request(&existing, &record);
        let rejected = existing.receipt.status == "rejected";
        let is_bound = if same && !rejected {
            match bound(&service, &existing, headers).await {
                Ok(is_bound) => is_bound,
                // A store fault is no verdict, exactly as on /settle.
                Err(_) => {
                    return failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE)
                }
            }
        } else {
            false
        };
        if same && (rejected || is_bound) {
            // The authorization was already verified before durable admission.
            // Do not reject its consumed nonce and invite a fresh signature.
            let mut body = json!({"isValid":!rejected,"payer":existing.receipt.payer,"receipt":existing.receipt});
            if rejected {
                body["invalidReason"] = json!(existing.receipt.refusal_reason);
            }
            return (StatusCode::OK, Json(body)).into_response();
        }
        if !rejected {
            // Admitted for another purchase, or resent without the binding that
            // admitted it: never valid again, and never simulated again.
            let mut body = json!({"isValid":false,"invalidReason":admitted_reason(&existing),"payer":existing.receipt.payer});
            if same && existing.token_hash.is_empty() {
                body["receipt"] = serde_json::to_value(&existing.receipt).unwrap();
            }
            return (StatusCode::OK, Json(body)).into_response();
        }
    }
    finish(&service, &mut record, call.await, false).await
}

fn aliases(record: &Record, headers: &HeaderMap) -> Vec<String> {
    let mut keys = vec![format!(
        "receipt:auth:v1:{}",
        record.receipt.authorization_id
    )];
    if !record.token_hash.is_empty() {
        // Possession of a random 256-bit capability defines the namespace.
        // A self-declared merchantId or an enumerable order number never does.
        keys.push(format!("receipt:purchase:v1:{}", record.token_hash));
    }
    keys.extend(idempotency_alias(record, headers));
    keys
}

fn idempotency_alias(record: &Record, headers: &HeaderMap) -> Option<String> {
    let key = headers
        .get("idempotency-key")
        .filter(|key| !key.is_empty())?;
    Some(format!(
        "receipt:idem:v1:{}",
        hash(
            format!(
                "{}:{}:{}:{}",
                record.receipt.network,
                record.receipt.pay_to,
                record.receipt.payer.as_deref().unwrap_or_default(),
                hash(key.as_bytes())
            )
            .as_bytes()
        )
    ))
}

/// Whether a resend carries the purchase binding that admitted `existing`: its
/// `X-UVD-Purchase` capability (already matched by `same_request`) or its
/// Idempotency-Key. The signed payment alone proves possession of the payment,
/// not of the purchase, so it never earns the original answer back.
async fn bound(service: &Service, existing: &Record, headers: &HeaderMap) -> Result<bool> {
    if !existing.token_hash.is_empty() {
        return Ok(true);
    }
    let Some(key) = idempotency_alias(existing, headers) else {
        return Ok(false);
    };
    Ok(service
        .store
        .get(&key)
        .await?
        .is_some_and(|r| r.receipt.receipt_id == existing.receipt.receipt_id))
}

fn admitted_reason(record: &Record) -> &'static str {
    if record.receipt.status == "confirmed" {
        "authorization_already_settled"
    } else {
        "authorization_in_flight"
    }
}

/// A resend of an admitted authorization. Recovery material (reconciliation,
/// rebroadcast of saved bytes) runs as before. The original answer, including
/// a 202 in flight, goes only to the binding that admitted the payment. A bare
/// resend learns the outcome from the receipt and is never answered as a
/// repeated success.
async fn replay<A: HasProviderMap>(
    service: &Service,
    facilitator: &A,
    mut existing: Record,
    headers: &HeaderMap,
) -> Response
where
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    reconcile(service, facilitator, &mut existing).await;
    rebroadcast_prepared(facilitator, &existing).await;
    if existing.receipt.status == "rejected" {
        return response(&existing, true);
    }
    match bound(service, &existing, headers).await {
        Ok(true) => response(&existing, true),
        Ok(false) => {
            let mut body = json!({"success":false,"error":admitted_reason(&existing),"retryable":false,"safeToReplay":false});
            // Capability-scoped receipts stay private; `bound` admits those.
            if existing.token_hash.is_empty() {
                body["receipt"] = serde_json::to_value(&existing.receipt).unwrap();
            }
            let mut response = (StatusCode::CONFLICT, Json(body)).into_response();
            response
                .headers_mut()
                .insert("cache-control", HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn same_request(a: &Record, b: &Record) -> bool {
    a.receipt.request_hash == b.receipt.request_hash
        && a.receipt.payment_request_hash == b.receipt.payment_request_hash
        && a.token_hash == b.token_hash
}

fn legacy_response(
    record: crate::idempotency_store::IdempotencyRecord,
    request_hash: &str,
) -> Response {
    if record.request_hash != request_hash {
        return failure("idempotency_key_conflict", StatusCode::CONFLICT);
    }
    // Preserve responses created before portable receipts existed. Rechecking
    // their consumed nonce would turn a paid retry into a false rejection.
    let Ok(result) = serde_json::from_str::<crate::types::SettleResponse>(&record.response_json)
    else {
        return failure("idempotency_cache_corrupt", StatusCode::SERVICE_UNAVAILABLE);
    };
    let mut response = (StatusCode::OK, Json(result)).into_response();
    response
        .headers_mut()
        .insert("idempotent-replayed", HeaderValue::from_static("true"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn legacy_replay(headers: &HeaderMap, raw: &Bytes) -> Option<Response> {
    let key = headers.get("idempotency-key")?.to_str().ok()?.trim();
    if key.is_empty() {
        return None;
    }
    let decoded;
    let bytes = if let Some(encoded) = headers.get("payment-signature") {
        decoded = STANDARD.decode(encoded.as_bytes()).ok()?;
        decoded.as_slice()
    } else {
        raw.as_ref()
    };
    let request_hash = crate::idempotency_store::hash_request_body(bytes);
    #[cfg(test)]
    if let Ok(record) = TEST_LEGACY_RECORD.try_with(Clone::clone) {
        assert_eq!(record.idempotency_key, key);
        return Some(legacy_response(record, &request_hash));
    }
    match crate::idempotency_store::lookup_record(key.to_owned()).await {
        Ok(Some(record)) => Some(legacy_response(record, &request_hash)),
        Ok(None) => None,
        Err(_) => Some(failure(
            "idempotency_cache_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        )),
    }
}

/// The closure is invoked only by the owner of an atomic admission. Existing
/// rows, including an abandoned reservation, NEVER invoke it again.
pub async fn settle<A, F>(facilitator: &A, headers: &HeaderMap, raw: &Bytes, call: F) -> Response
where
    A: Facilitator + HasProviderMap + Sync,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
    F: Future<Output = Response>,
{
    // The legacy cache accepts caller-chosen table keys. Protect receipt rows
    // even on unsupported chains or instances without receipt initialization.
    if headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|key| key.trim().starts_with("receipt:"))
    {
        return failure("reserved_idempotency_key", StatusCode::BAD_REQUEST);
    }
    let Some(service) = service() else {
        return if headers.contains_key("x-uvd-purchase") {
            failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE)
        } else {
            call.await
        };
    };
    let Some(request) = parse_request(headers, raw) else {
        return if headers.contains_key("x-uvd-purchase") {
            failure("receipt_request_not_supported", StatusCode::BAD_REQUEST)
        } else {
            call.await
        };
    };
    let context = match purchase_context(headers) {
        Ok(c) => c,
        Err(e) => return failure(&e, StatusCode::BAD_REQUEST),
    };
    let mut candidate = match initial(&request, context.as_ref(), "settle") {
        Ok(r) => r,
        Err(_) if context.is_some() => {
            return failure("receipt_request_not_supported", StatusCode::BAD_REQUEST)
        }
        Err(_) => return call.await,
    };
    if let Some(context) = &context {
        if url::Url::parse(&context.url).ok().as_ref()
            != Some(&request.payment_requirements.resource)
        {
            return failure("receipt_resource_mismatch", StatusCode::CONFLICT);
        }
    }
    // Check the authorization before re-verifying: a successful settlement has
    // consumed its nonce and would now fail an otherwise valid signature check.
    let auth_key = format!("receipt:auth:v1:{}", candidate.receipt.authorization_id);
    match service.store.get(&auth_key).await {
        Ok(Some(existing)) => {
            if !same_request(&existing, &candidate) {
                return failure("receipt_request_conflict", StatusCode::CONFLICT);
            }
            return replay(&service, facilitator, existing, headers).await;
        }
        Err(_) => return failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE),
        _ => {}
    }
    if let Some(replay) = legacy_replay(headers, raw).await {
        return replay;
    }
    // Verify before claiming a nonce: an invalid signature cannot squat another
    // payer's authorization. This is read-only and cannot move principal.
    let verification_failure = match facilitator.verify(&request).await {
        Ok(VerifyResponse::Valid { payer }) => {
            candidate.receipt.payer = Some(payer.to_string());
            None
        }
        Ok(VerifyResponse::Invalid { reason, payer }) => {
            let body = json!({"success":false,"errorReason":reason,"payer":payer});
            Some((StatusCode::OK, Json(body)).into_response())
        }
        Err(error) => Some(error.into_response()),
    };
    if let Some(failure) = verification_failure {
        return finish(&service, &mut candidate, failure, false).await;
    }
    let keys = aliases(&candidate, headers);
    if service.sign(&mut candidate.receipt).is_err() {
        return failure(
            "receipt_signing_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        );
    }
    match service.store.reserve(&candidate, &keys).await {
        Ok(true) => {}
        Ok(false) => {
            for key in &keys {
                match service.store.get(key).await {
                    Ok(Some(existing)) => {
                        if !same_request(&existing, &candidate) {
                            return failure("receipt_request_conflict", StatusCode::CONFLICT);
                        }
                        return replay(&service, facilitator, existing, headers).await;
                    }
                    Err(_) => {
                        return failure(
                            "receipt_store_unavailable",
                            StatusCode::SERVICE_UNAVAILABLE,
                        )
                    }
                    _ => {}
                }
            }
            return failure(
                "receipt_reservation_uncertain",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
        Err(_) => return failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE),
    }
    let active = Arc::new(tokio::sync::Mutex::new(candidate));
    let result = ACTIVE.scope(active.clone(), call).await;
    let mut record = active.lock().await;
    finish(&service, &mut record, result, true).await
}

pub fn active() -> bool {
    ACTIVE.try_with(|_| ()).is_ok()
}

/// Called with the exact signed EVM bytes BEFORE broadcast. A storage failure
/// prevents broadcast; the signed bytes never appear in the public receipt.
pub async fn prepared_evm(hash: String, bytes: Vec<u8>) -> Result<()> {
    let Some(service) = service() else {
        return Ok(());
    };
    let Ok(active) = ACTIVE.try_with(Arc::clone) else {
        return Ok(());
    };
    let mut record = active.lock().await;
    if record.prepared.is_some() {
        return Err("receipt_multiple_transactions_not_supported".into());
    }
    let mut next = record.clone();
    next.prepared =
        Some(json!({"transactionHash":hash,"signedTransaction":STANDARD.encode(bytes)}));
    next.receipt.settlement =
        Some(json!({"id":hash,"idType":"evm-transaction-hash","paymentId":Value::Null}));
    next.receipt.status = "pending".into();
    next.receipt.diagnostic_code = Some("transaction_prepared".into());
    service.save(&mut next).await?;
    *record = next;
    Ok(())
}

/// Hedera already owns its durable signed bytes and recovery. Persist only the
/// native transaction ID here; there must be no second co-signing mechanism.
pub async fn prepared_hedera(id: &str) -> Result<()> {
    let Some(service) = service() else {
        return Ok(());
    };
    let Ok(active) = ACTIVE.try_with(Arc::clone) else {
        return Ok(());
    };
    let mut record = active.lock().await;
    let mut next = record.clone();
    next.prepared = Some(json!({"transactionId":id}));
    next.receipt.settlement =
        Some(json!({"id":id,"idType":"hedera-transaction-id","paymentId":Value::Null}));
    next.receipt.status = "pending".into();
    next.receipt.diagnostic_code = Some("native_transaction_identified".into());
    service.save(&mut next).await?;
    *record = next;
    Ok(())
}

/// POST retries may rebroadcast ONLY persisted bytes with the original nonce.
/// GET remains read-only. No replacement transaction or new signature is made.
async fn rebroadcast_prepared<A: HasProviderMap>(facilitator: &A, record: &Record)
where
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    if !matches!(record.receipt.status.as_str(), "pending" | "unknown")
        || now() < record.receipt.issued_at.saturating_add(5)
        || now() >= record.authorization_expires_at
    {
        return;
    }
    let Some(prepared) = &record.prepared else {
        return;
    };
    let Some(encoded) = prepared.get("signedTransaction").and_then(Value::as_str) else {
        return;
    };
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return;
    };
    if Some(alloy::primitives::keccak256(&bytes).to_string().as_str())
        != prepared.get("transactionHash").and_then(Value::as_str)
    {
        return;
    }
    let Some(network) = Network::from_caip2(&record.receipt.network) else {
        return;
    };
    let Some(NetworkProvider::Evm(provider)) = facilitator.provider_map().by_network(network)
    else {
        return;
    };
    let Some(_permit) = crate::writer_lease::signing_permit() else {
        return;
    };
    use crate::chain::evm::MetaEvmProvider;
    use alloy::providers::Provider;
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.inner().send_raw_transaction(&bytes),
    )
    .await;
}

async fn reconcile<A: HasProviderMap>(service: &Service, facilitator: &A, record: &mut Record)
where
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    if matches!(record.receipt.status.as_str(), "confirmed" | "rejected") {
        return;
    }
    let Some(prepared) = &record.prepared else {
        return;
    };
    let Some(network) = Network::from_caip2(&record.receipt.network) else {
        return;
    };
    let outcome: Option<(bool, String)> = match facilitator.provider_map().by_network(network) {
        Some(NetworkProvider::Evm(provider)) => {
            use crate::chain::evm::MetaEvmProvider;
            use alloy::providers::Provider;
            let Some(tx) = prepared
                .get("transactionHash")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
            else {
                return;
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                provider.inner().get_transaction_receipt(tx),
            )
            .await
            {
                Ok(Ok(Some(receipt))) if receipt.block_hash.is_some() => {
                    Some((receipt.status(), tx.to_string()))
                }
                _ => None,
            }
        }
        #[cfg(feature = "hedera")]
        Some(NetworkProvider::Hedera(provider)) => {
            let Some(id) = prepared.get("transactionId").and_then(Value::as_str) else {
                return;
            };
            provider
                .receipt_evidence(id)
                .await
                .map(|success| (success, id.to_owned()))
        }
        _ => None,
    };
    if let Some((success, tx)) = outcome {
        let mut updated = record.clone();
        updated.receipt.status = if success { "confirmed" } else { "rejected" }.into();
        updated.receipt.refusal_reason = (!success).then(|| "settlement_failed_on_chain".into());
        updated.receipt.diagnostic_code = None;
        updated.receipt.retry = json!({"action":"none"});
        if let Some(prepared) = updated.prepared.as_mut().and_then(Value::as_object_mut) {
            prepared.remove("signedTransaction");
        }
        let payment_id = crate::dx402::payment_id(network, &tx);
        updated.receipt.settlement.as_mut().unwrap()["paymentId"] = json!(payment_id);
        updated.http_status = 200;
        updated.response = json!({"success":success,"transaction":tx,"transactionHash":tx,"transaction_hash":tx,
            "network":network,"payer":updated.receipt.payer,"paymentId":payment_id});
        if !success {
            updated.response["errorReason"] = json!("settlement_failed_on_chain");
        }
        if service.save(&mut updated).await.is_ok() {
            *record = updated;
        }
    }
}

pub async fn get<A>(
    State(facilitator): State<A>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response
where
    A: HasProviderMap + Send + Sync,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let Some(service) = service() else {
        return failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    else {
        return failure("receipt_not_found", StatusCode::NOT_FOUND);
    };
    if uuid::Uuid::parse_str(&id).is_err() || token.len() != 64 {
        return failure("receipt_not_found", StatusCode::NOT_FOUND);
    }
    let mut record = match service.store.get(&format!("receipt:v1:{id}")).await {
        Ok(Some(record))
            if !record.token_hash.is_empty() && record.token_hash == hash(token.as_bytes()) =>
        {
            record
        }
        Ok(_) => return failure("receipt_not_found", StatusCode::NOT_FOUND),
        Err(_) => return failure("receipt_store_unavailable", StatusCode::SERVICE_UNAVAILABLE),
    };
    reconcile(&service, &facilitator, &mut record).await;
    // The lookup succeeded even if the original payment request failed.
    // Keep that payment state in the signed receipt, not the GET status.
    let mut found = response(&record, true);
    *found.status_mut() = StatusCode::OK;
    found
}

#[cfg(test)]
mod tests;
