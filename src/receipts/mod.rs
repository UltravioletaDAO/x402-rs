//! Portable facilitator receipts for exact EVM payments in ERC-3009 tokens
//! (`evm_exact_tokens`) and native Hedera payments.
//! A receipt attests payment state, never merchant delivery. Admission is
//! atomically reserved before the provider can broadcast. Uncertainty is sticky:
//! only chain evidence can turn it into confirmation, never a new authorization.
//! An admission whose provider provably sent nothing is not uncertain: it is
//! released in place and the same request is admitted again (`abandon`).
pub mod admin;
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
    http::{header::RETRY_AFTER, HeaderMap, HeaderValue, StatusCode},
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
/// `refusalReason` of an admission released before anything was sent. Only
/// the request it admitted can be admitted again, under the same receipt.
pub const RESERVATION_ABANDONED: &str = "reservation_abandoned";
/// `Retry-After` on answers that sent nothing and invite the same request again.
const RETRY_AFTER_SECS: u64 = 2;
static SERVICE: OnceCell<Arc<Service>> = OnceCell::new();
tokio::task_local! { static ACTIVE: Arc<Admission>; }
#[cfg(test)]
tokio::task_local! { static TEST_SERVICE: Arc<Service>; }
#[cfg(test)]
tokio::task_local! { static TEST_LEGACY: Arc<dyn crate::idempotency_store::IdempotencyStore>; }
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
    /// The ERC-3009 authorization an EVM admission settles, by the key the
    /// token tracks it under (`authorizationState`). Absent on Hedera, and on
    /// records written before 2.51.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evm_authorization: Option<EvmAuthorization>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvmAuthorization {
    pub from: alloy::primitives::Address,
    pub nonce: alloy::primitives::B256,
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

/// The admission a settle closure runs under: the record it updates, and what
/// the provider has said about sending.
struct Admission {
    record: tokio::sync::Mutex<Record>,
    sending: std::sync::Mutex<Sending>,
}

/// `unsent` names where the provider ended the settlement without sending
/// anything. It is a statement about the past, so it only counts until
/// `latched`: once a transaction may have left, nothing releases the admission.
#[derive(Default)]
struct Sending {
    latched: bool,
    unsent: Option<&'static str>,
}

impl Admission {
    fn new(record: Record) -> Arc<Self> {
        Arc::new(Self {
            record: tokio::sync::Mutex::new(record),
            sending: Default::default(),
        })
    }

    fn sending(&self) -> std::sync::MutexGuard<'_, Sending> {
        self.sending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Where the settlement ended without sending, if it never latched.
    fn unsent(&self) -> Option<&'static str> {
        let state = self.sending();
        state.unsent.filter(|_| !state.latched)
    }

    /// Whether the provider reached the point where a transaction may leave.
    fn latched(&self) -> bool {
        self.sending().latched
    }
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

/// The EVM side of the rail, one matrix for its announcement (`capability`),
/// its admission (`admits`) and its normalization (`initial`): the tokens
/// `/supported` lists for the `exact` kind of an EVM network
/// (`network::exact_payment_tokens`, ERC-3009 deployments only), on the
/// networks where production publishes that kind (`exactServed` in
/// `config/supported_tokens.json`, which leaves out Ethereum Sepolia and the
/// EVM variants nothing serves). Empty for any other network.
pub fn evm_exact_tokens(network: Network) -> Vec<crate::types::SupportedTokenInfo> {
    let served = crate::networks_json::CATALOG
        .as_ref()
        .ok()
        .and_then(|catalog| catalog.network(network))
        .is_some_and(|meta| meta.exact_served);
    if !served || !matches!(NetworkFamily::from(network), NetworkFamily::Evm) {
        return Vec::new();
    }
    crate::network::exact_payment_tokens(network)
}

/// `capability()` lists exactly these networks.
pub fn supported(network: Network) -> bool {
    !evm_exact_tokens(network).is_empty() || network.is_hedera()
}

/// Whether the rail admits an `exact` request: one network on both sides of
/// it, a supported one, and on EVM one of that network's matrix tokens. What
/// it admits is what `initial` can normalize: an EVM receipt always carries
/// its token's decimals.
fn admits(request: &VerifyRequest) -> bool {
    let network = request.network();
    let terms = &request.payment_requirements;
    if terms.scheme != Scheme::Exact || terms.network != network || !supported(network) {
        return false;
    }
    match NetworkFamily::from(network) {
        NetworkFamily::Evm => evm_exact_tokens(network)
            .iter()
            .any(|token| token.address == terms.asset),
        _ => true,
    }
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
    admits(&request).then_some(request)
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
        evm_authorization: match &request.payment_payload.payload {
            ExactPaymentPayload::Evm(evm) => Some(EvmAuthorization {
                from: evm.authorization.from.0,
                nonce: evm.authorization.nonce.0.into(),
            }),
            _ => None,
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

    /// `save` for an admission this process wrote and never ran: written at
    /// the next revision whether the store holds it at one of `held` or not at
    /// all (`store::Store::release`).
    async fn release(&self, record: &Record, held: &[u64]) -> Result<bool> {
        let mut next = record.clone();
        next.receipt.revision += 1;
        next.receipt.issued_at = now();
        self.sign(&mut next.receipt)?;
        self.store.release(&next, held).await
    }
}

fn signing_key_from_env() -> Result<Option<SigningKey>> {
    match std::env::var("RECEIPT_SIGNING_KEY") {
        Ok(secret) => {
            let decoded = hex::decode(secret.trim()).map_err(|_| "invalid_receipt_signing_key")?;
            let bytes: [u8; 32] = decoded
                .try_into()
                .map_err(|_| "invalid_receipt_signing_key")?;
            Ok(Some(SigningKey::from_bytes(&bytes)))
        }
        Err(_) => Ok(None),
    }
}

pub async fn init() -> Result<()> {
    let Some(store) = store::DynamoStore::from_env().await else {
        return Ok(());
    };
    let signing_key = signing_key_from_env()?;
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
    let networks: Vec<String> = Network::variants()
        .iter()
        .filter(|network| supported(**network))
        .map(Network::to_caip2)
        .collect();
    json!({"schemaVersion":1,"available":SERVICE.get().is_some(),"networks":networks,
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
        operation["description"] = json!(format!("{description}\n\nExact payments on the networks GET /receipts lists include an additive `receipt`: on an EVM network, in the ERC-3009 tokens /supported lists for its `exact` kind; on Hedera, native USDC. It carries network, asset, atomic amount, payTo, requestHash, settlement ID, status and refusalReason. See /schemas/facilitator-receipt-v1.json. Send X-UVD-Purchase (base64 JSON with purchaseId, secret accessToken, method, url, bodySha256) for private lookup and restart-safe purchase retries, until the receipt rows expire (see the receipt guide). The merchant must validate the actual HTTP request. Preserve the same context and authorization after uncertainty; never sign a replacement. Payment confirmation does not prove merchant delivery. Other networks retain their existing responses."));
        let params = operation
            .as_object_mut()
            .unwrap()
            .entry("parameters")
            .or_insert_with(|| json!([]));
        params.as_array_mut().unwrap().push(json!({"name":"X-UVD-Purchase","in":"header","required":false,"schema":{"type":"string"},"description":"Private purchase context; never log its accessToken or signed authorization."}));
        operation["responses"]["200"]["content"]["application/json"]["schema"] = json!({"type":"object","properties":{"receipt":{"$ref":"#/components/schemas/FacilitatorReceipt"}},"additionalProperties":true});
        if path == "/verify" {
            operation["responses"]["503"] = json!({"description":"On the receipt rail, `receipt_store_unavailable` with `Retry-After` and `safeToRetry: true`: receipt storage did not answer, so no verdict is given. Resend the same request; never sign a replacement."});
        }
        if path == "/settle" {
            operation["responses"]["202"] = json!({"description":"Reserved payment remains pending or unknown. Returned to the X-UVD-Purchase or Idempotency-Key that admitted it. Poll the private receipt or retry exactly the same request; no new payment is admitted."});
            operation["responses"]["409"] = json!({"description":"Purchase, authorization or idempotency key conflicts with the original request, or the authorization was already admitted and this request lacks the X-UVD-Purchase or Idempotency-Key that admitted it: `authorization_already_settled` or `authorization_in_flight`, with the receipt when the payment has no purchase context. No replacement payment is admitted and no success is repeated."});
            let unavailable = operation["responses"]["503"]["description"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            operation["responses"]["503"]["description"] = json!(format!("{unavailable}\n\nOn the receipt rail, a 503 with `safeToRetry: true` and `Retry-After` sent nothing: receipt storage or signing failed before admission (`receipt_store_unavailable`, `receipt_signing_unavailable`, `receipt_reservation_uncertain`), or the admission was released before any transaction existed (receipt `rejected` with `refusalReason: reservation_abandoned`). Resend the same request; it is admitted again under the same receipt. Never sign a replacement."));
            let gateway = operation["responses"]["502"]["description"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            operation["responses"]["502"]["description"] = json!(format!("{gateway}\n\nOn the receipt rail, a failure answered after the send latched or its bytes were prepared, and `receipt_response_unreadable` once the settlement ran, carry `retryable: false`, no `Retry-After`, the prepared `transaction` with its `paymentId`, and the receipt (`status: unknown`). Poll the receipt or resend the same request with the binding that admitted it; never sign a replacement."));
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

/// A 503 before this request sent anything: resending the same request is
/// safe and cannot create a second payment.
fn unavailable(code: &str) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"success":false,"error":code,"retryable":true,"safeToRetry":true,"safeToReplay":false})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECS));
    response
}

/// Closes an admission under which nothing was sent. The receipt says so, and
/// the request it admitted is admitted again under it (`readmit`).
fn abandon(record: &mut Record, diagnostic: &str, after: u64, mut body: Value) {
    if !body.is_object() {
        body = json!({ "error": diagnostic });
    }
    body["success"] = json!(false);
    body["retryable"] = json!(true);
    body["safeToRetry"] = json!(true);
    body["safeToReplay"] = json!(false);
    record.response = body;
    record.http_status = StatusCode::SERVICE_UNAVAILABLE.as_u16();
    record.receipt.status = "rejected".into();
    record.receipt.refusal_reason = Some(RESERVATION_ABANDONED.into());
    record.receipt.diagnostic_code = Some(diagnostic.into());
    record.receipt.retry = json!({"action":"resend","afterSeconds":after});
}

/// Released before anything was sent: nothing prepared, nothing named.
fn is_abandoned(record: &Record) -> bool {
    record.receipt.status == "rejected"
        && record.receipt.refusal_reason.as_deref() == Some(RESERVATION_ABANDONED)
        && record.prepared.is_none()
        && record.receipt.settlement.is_none()
}

/// An abandoned admission met outside `readmit`, in a race: resend.
fn resend_later(record: &Record) -> Response {
    let mut response = response(record, false);
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECS));
    response
}

/// Whether a transaction may have left under this admission: the provider
/// latched its send, bytes were prepared, or a transaction was named.
fn may_have_sent(record: &Record, latched: bool) -> bool {
    latched || record.prepared.is_some() || record.receipt.settlement.is_some()
}

/// Rewrites a failure answered after a transaction may have left, so that it
/// says so: `retryable: false`, and the transaction and its `paymentId` when
/// the admission knows them and the answer did not name them. A client reads a
/// `5xx` that says neither as transient and resends; for a payment that did
/// mine, that resend fails verification and the buyer signs a second one.
fn not_retryable(body: &mut Value, record: &Record) {
    if !body.is_object() {
        *body = json!({"success": false, "error": "settlement_unconfirmed"});
    }
    body["retryable"] = json!(false);
    let named = body
        .get("transaction")
        .and_then(Value::as_str)
        .is_some_and(|tx| !tx.is_empty());
    if named {
        return;
    }
    let settlement = record.receipt.settlement.as_ref();
    let Some(tx) = settlement.and_then(|s| s.get("id")).and_then(Value::as_str) else {
        return;
    };
    body["transaction"] = json!(tx);
    if let Some(network) = Network::from_caip2(&record.receipt.network) {
        body["paymentId"] = json!(crate::dx402::payment_id(network, tx));
    }
}

async fn finish(
    service: &Service,
    record: &mut Record,
    raw: Response,
    durable: bool,
    unsent: Option<&'static str>,
    latched: bool,
) -> Response {
    let durable_before = record.clone();
    let (mut parts, body) = raw.into_parts();
    let bytes = match to_bytes(body, 65536).await {
        Ok(bytes) => bytes,
        // The settlement ran and its answer is gone. When a transaction may
        // have left, the caller is not invited to retry: it gets the receipt
        // and the transaction, if one was prepared, to look up.
        Err(_) if durable && may_have_sent(record, latched) => {
            let mut body = json!({
                "success": false,
                "error": "receipt_response_unreadable",
                "safeToReplay": false,
            });
            not_retryable(&mut body, record);
            body["receipt"] = serde_json::to_value(&record.receipt).unwrap();
            return (StatusCode::BAD_GATEWAY, Json(body)).into_response();
        }
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
    // A transaction may have left and the chain has given no verdict: the
    // failure says so, whatever the provider's own body said, and carries no
    // invitation to retry. The stored answer is the same one, so a bound
    // resend is told the same thing.
    let mut value = value;
    if durable
        && record.receipt.status == "unknown"
        && !parts.status.is_success()
        && may_have_sent(record, latched)
    {
        not_retryable(&mut value, record);
        record.response = value.clone();
        parts.headers.remove(RETRY_AFTER);
    }
    // Nothing left this process: the provider ended the settlement before its
    // send latch, no bytes were prepared and its answer names no transaction.
    // That is not uncertainty, so the admission is not left `unknown` for
    // ever: it is closed in place, the same request is admitted again, and the
    // caller hears 503, resend this request, never sign another. The save
    // below is a CAS on the owner's revision: bytes persisted by an ambiguous
    // write, or any other writer, make it fail and nothing is released.
    let provider_status = parts.status;
    let released = durable
        && record.receipt.status == "unknown"
        && record.prepared.is_none()
        && record.receipt.settlement.is_none();
    let released = unsent.filter(|_| released);
    if let Some(site) = released {
        let after = parts
            .headers
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|after| *after > 0)
            .unwrap_or(RETRY_AFTER_SECS);
        let diagnostic = value.get("error").and_then(Value::as_str).unwrap_or(site);
        abandon(record, diagnostic, after, value.clone());
        parts.status = StatusCode::SERVICE_UNAVAILABLE;
        parts.headers.insert(RETRY_AFTER, HeaderValue::from(after));
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
        if is_abandoned(&stable) {
            // Released by another writer while this owner ran (`release_unrun`,
            // the operator command): the revision it held could not store
            // prepared bytes, so nothing was sent under it.
            return resend_later(&stable);
        }
        if released.is_some() {
            // Nothing was sent and the release was not stored either: the
            // store that refused the bytes refused the close. Resending is as
            // safe as after any failure before the send, so the answer says
            // so, not the provider's 4xx, with the receipt as stored.
            let mut body = json!({"success":false,"error":"receipt_store_unavailable",
                "retryable":true,"safeToRetry":true,"safeToReplay":false,
                "receiptWarning":"receipt_persistence_unavailable"});
            body["receipt"] = serde_json::to_value(stable.receipt).unwrap();
            let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECS));
            return response;
        }
        // The provider's own answer: a release that was not stored did not happen.
        let mut body = value;
        body["receipt"] = serde_json::to_value(stable.receipt).unwrap();
        body["receiptWarning"] = json!("receipt_persistence_unavailable");
        return (provider_status, Json(body)).into_response();
    }
    if let Some(site) = released {
        tracing::warn!(
            receipt_id = %record.receipt.receipt_id,
            network = %record.receipt.network,
            site,
            "receipt admission released: the settlement ended before anything was sent"
        );
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
        Err(_) => {
            return match unnormalized_admission(&service, &request).await {
                // Admitted under another request: never valid again, and never
                // simulated again, as below. A rejected one, a released
                // admission included, is verified as before.
                Ok(Some(existing)) if existing.receipt.status != "rejected" => {
                    let body = json!({"isValid":false,"invalidReason":admitted_reason(&existing),"payer":existing.receipt.payer});
                    (StatusCode::OK, Json(body)).into_response()
                }
                Ok(_) => call.await,
                Err(answer) => answer,
            };
        }
    };
    // An abandoned admission sent nothing: verify as if it did not exist. A
    // store fault is no verdict: simulating a consumed nonce would invite a new
    // signature for a payment that may have settled.
    let existing = match service
        .store
        .get(&format!(
            "receipt:auth:v1:{}",
            record.receipt.authorization_id
        ))
        .await
    {
        Ok(found) => found.filter(|existing| !is_abandoned(existing)),
        Err(_) => return unavailable("receipt_store_unavailable"),
    };
    if let Some(existing) = existing {
        let same = same_request(&existing, &record);
        let rejected = existing.receipt.status == "rejected";
        let is_bound = if same && !rejected {
            match bound(&service, &existing, headers).await {
                Ok(is_bound) => is_bound,
                // A store fault is no verdict, exactly as on /settle.
                Err(_) => return unavailable("receipt_store_unavailable"),
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
    finish(&service, &mut record, call.await, false, None, false).await
}

/// The admission of the authorization a request names, for a request the rail
/// admits but cannot normalize (`initial` failed: its terms carry a float, an
/// integer past 2^53 or a non-ASCII key, which free-form `extra` and
/// `outputSchema` may). Such a request cannot be the one an admission holds,
/// but it can name the same authorization, and an admitted authorization never
/// goes around the rail. `Ok(None)`: no admission, or an authorization that
/// cannot be read, which stays the provider's to refuse. `Err` is the answer:
/// the store gave none.
async fn unnormalized_admission(
    service: &Service,
    request: &VerifyRequest,
) -> std::result::Result<Option<Record>, Response> {
    let Ok(id) = authorization(request) else {
        return Ok(None);
    };
    service
        .store
        .get(&format!("receipt:auth:v1:{id}"))
        .await
        .map_err(|_| unavailable("receipt_store_unavailable"))
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
    keys.extend(idempotency_conflict_alias(record, headers));
    keys
}

/// The binding of an Idempotency-Key: what a resend of the same request is
/// recognised by (`bound`). Too narrow to refuse a second signature under the
/// same key, which may name another network or `payTo`; that is
/// `idempotency_conflict_alias`.
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

/// One Idempotency-Key per payer, on every network, token and `payTo`: a second
/// signature from the same payer under a key already admitted is refused
/// (`receipt_request_conflict`), as the published contract says of the same
/// key with a different body. The key is read as `post_settle` reads it,
/// trimmed. Other payers meet the key in the 24-hour cache once the first
/// payment settles (`remember_settled`).
fn idempotency_conflict_alias(record: &Record, headers: &HeaderMap) -> Option<String> {
    let key = headers.get("idempotency-key")?.as_bytes().trim_ascii();
    if key.is_empty() {
        return None;
    }
    let payer = record.receipt.payer.as_deref().unwrap_or_default();
    Some(format!(
        "receipt:idemkey:v1:{}",
        hash(format!("{}:{}", payer.to_lowercase(), hash(key)).as_bytes())
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
    admitted_with_key(service, existing, headers).await
}

/// Whether the request's Idempotency-Key is one `existing` was admitted with:
/// its binding alias names that admission. A purchase capability binds a
/// resend whatever key it carries, so this is what a key alone proves.
async fn admitted_with_key(
    service: &Service,
    existing: &Record,
    headers: &HeaderMap,
) -> Result<bool> {
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
    raw: &Bytes,
) -> Response
where
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    if is_abandoned(&existing) {
        return resend_later(&existing);
    }
    reconcile(service, facilitator, &mut existing).await;
    rebroadcast_prepared(facilitator, &existing).await;
    if existing.receipt.status == "rejected" {
        return response(&existing, true);
    }
    match bound(service, &existing, headers).await {
        Ok(true) => {
            // Settled while its first answer was out, or reconciled just now:
            // filed under the request's key only if that key admitted it.
            if existing.receipt.status == "confirmed"
                && admitted_with_key(service, &existing, headers).await == Ok(true)
            {
                remember_settled(headers, raw, &existing).await;
            }
            response(&existing, true)
        }
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
        Err(_) => unavailable("receipt_store_unavailable"),
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

/// What `post_settle`'s 24-hour Idempotency-Key cache files a request under:
/// the trimmed key, and the hash of the body as decoded.
fn legacy_entry(headers: &HeaderMap, raw: &Bytes) -> Option<(String, String)> {
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
    Some((
        key.to_owned(),
        crate::idempotency_store::hash_request_body(bytes),
    ))
}

/// A receipt test's own 24-hour cache: the one it installs, or none at all,
/// never the process-wide one the handler tests share.
#[cfg(test)]
fn test_legacy() -> Option<Arc<dyn crate::idempotency_store::IdempotencyStore>> {
    if let Ok(cache) = TEST_LEGACY.try_with(Arc::clone) {
        return Some(cache);
    }
    TEST_SERVICE
        .try_with(|_| {
            Arc::new(crate::idempotency_store::NoopIdempotencyStore)
                as Arc<dyn crate::idempotency_store::IdempotencyStore>
        })
        .ok()
}

async fn legacy_get(
    key: String,
) -> std::result::Result<
    Option<crate::idempotency_store::IdempotencyRecord>,
    crate::idempotency_store::IdempotencyStoreError,
> {
    #[cfg(test)]
    if let Some(cache) = test_legacy() {
        return cache.get(&key).await;
    }
    crate::idempotency_store::lookup_record(key).await
}

async fn legacy_put(
    record: crate::idempotency_store::IdempotencyRecord,
) -> std::result::Result<(), crate::idempotency_store::IdempotencyStoreError> {
    #[cfg(test)]
    if let Some(cache) = test_legacy() {
        return cache.put(record).await;
    }
    crate::idempotency_store::store_record(record).await
}

async fn legacy_replay(headers: &HeaderMap, raw: &Bytes) -> Option<Response> {
    let (key, request_hash) = legacy_entry(headers, raw)?;
    match legacy_get(key).await {
        Ok(Some(record)) => Some(legacy_response(record, &request_hash)),
        Ok(None) => None,
        Err(_) => Some(failure(
            "idempotency_cache_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        )),
    }
}

/// Files a settled admission in `post_settle`'s 24-hour Idempotency-Key cache,
/// under the key and body it was admitted with, as `post_settle` files what it
/// settles. Under an admission the handler keeps out of that cache, so without
/// this row a second signature under the same key would find nothing there: on
/// a path off the rail, from another payer, or on a release before 2.51.0 after
/// a rollback. The caller vouches that the request's key is one the admission
/// was made with. A live row for another body is never replaced, as
/// `post_settle` never replaces one either: it refuses before settling. Best
/// effort: the receipt is the record, this row is a fence.
async fn remember_settled(headers: &HeaderMap, raw: &Bytes, record: &Record) {
    if record.receipt.operation != "settle" || record.receipt.status != "confirmed" {
        return;
    }
    let Some((key, request_hash)) = legacy_entry(headers, raw) else {
        return;
    };
    let within = std::time::Duration::from_secs(5);
    match tokio::time::timeout(within, legacy_get(key.clone())).await {
        Ok(Ok(None)) => {}
        // Filed already, by this payment or by whatever else holds the key.
        Ok(Ok(Some(_))) => return,
        _ => {
            tracing::warn!(
                receipt_id = %record.receipt.receipt_id,
                "settled admission not filed under its Idempotency-Key: the cache did not answer"
            );
            return;
        }
    }
    let settled =
        serde_json::from_value::<crate::types::SettleResponse>(record.response.clone()).ok();
    let Some(response_json) = settled
        .filter(|settled| settled.success)
        .and_then(|settled| serde_json::to_string(&settled).ok())
    else {
        tracing::warn!(
            receipt_id = %record.receipt.receipt_id,
            "settled admission has no settle response to file under its Idempotency-Key"
        );
        return;
    };
    let row = crate::idempotency_store::IdempotencyRecord {
        idempotency_key: key,
        request_hash,
        response_json,
        expires_at: now() + crate::idempotency_store::IDEMPOTENCY_TTL_SECONDS,
    };
    match tokio::time::timeout(within, legacy_put(row)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(
            receipt_id = %record.receipt.receipt_id,
            %error,
            "settled admission not filed under its Idempotency-Key"
        ),
        Err(_) => tracing::warn!(
            receipt_id = %record.receipt.receipt_id,
            "settled admission not filed under its Idempotency-Key: the write timed out"
        ),
    }
}

/// The closure is invoked only by the owner of an atomic admission. Existing
/// rows never invoke it again, with one exception: an admission released
/// before anything was sent (`abandon`), which the request it admitted takes
/// back under the same receipt through a revision CAS (`readmit`).
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
            unavailable("receipt_store_unavailable")
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
        Err(_) => {
            return match unnormalized_admission(&service, &request).await {
                // Another request for an admitted authorization, as below.
                Ok(Some(_)) => failure("receipt_request_conflict", StatusCode::CONFLICT),
                Ok(None) if context.is_some() => {
                    failure("receipt_request_not_supported", StatusCode::BAD_REQUEST)
                }
                Ok(None) => call.await,
                Err(answer) => answer,
            };
        }
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
    let abandoned = match service.store.get(&auth_key).await {
        Ok(Some(existing)) => {
            if !same_request(&existing, &candidate) {
                return failure("receipt_request_conflict", StatusCode::CONFLICT);
            }
            if !is_abandoned(&existing) {
                return replay(&service, facilitator, existing, headers, raw).await;
            }
            Some(existing)
        }
        Ok(None) => None,
        Err(_) => return unavailable("receipt_store_unavailable"),
    };
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
        return finish(&service, &mut candidate, failure, false, None, false).await;
    }
    let keys = aliases(&candidate, headers);
    let admitted = match abandoned {
        Some(abandoned) => {
            readmit(
                &service,
                facilitator,
                abandoned,
                candidate,
                &keys,
                headers,
                raw,
            )
            .await
        }
        None => admit(&service, facilitator, candidate, &keys, headers, raw).await,
    };
    let record = match admitted {
        Ok(record) => record,
        Err(answer) => return *answer,
    };
    let admission = Admission::new(record);
    let result = ACTIVE.scope(admission.clone(), call).await;
    let unsent = admission.unsent();
    let latched = admission.latched();
    let mut record = admission.record.lock().await;
    let answer = finish(&service, &mut record, result, true, unsent, latched).await;
    // This request's key made the admission (`aliases`, `readmit`).
    remember_settled(headers, raw, &record).await;
    answer
}

/// Reserves a new admission: the record and every alias atomically, or none.
async fn admit<A>(
    service: &Service,
    facilitator: &A,
    mut candidate: Record,
    keys: &[String],
    headers: &HeaderMap,
    raw: &Bytes,
) -> std::result::Result<Record, Box<Response>>
where
    A: HasProviderMap,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    if service.sign(&mut candidate.receipt).is_err() {
        return Err(Box::new(unavailable("receipt_signing_unavailable")));
    }
    // One fresh token per write: see `store::Store::reserve`.
    let token = uuid::Uuid::new_v4().to_string();
    match service.store.reserve(&candidate, keys, &token).await {
        Ok(true) => Ok(candidate),
        Ok(false) => {
            for key in keys {
                match service.store.get(key).await {
                    // This request's own reservation, landed by a write the
                    // store could not confirm. Nothing runs under it.
                    Ok(Some(existing))
                        if existing.receipt.receipt_id == candidate.receipt.receipt_id =>
                    {
                        release_unrun(service, &candidate, None).await;
                        return Err(Box::new(unavailable("receipt_store_unavailable")));
                    }
                    Ok(Some(existing)) => {
                        if !same_request(&existing, &candidate) {
                            return Err(Box::new(failure(
                                "receipt_request_conflict",
                                StatusCode::CONFLICT,
                            )));
                        }
                        return Err(Box::new(
                            replay(service, facilitator, existing, headers, raw).await,
                        ));
                    }
                    Err(_) => return Err(Box::new(unavailable("receipt_store_unavailable"))),
                    Ok(None) => {}
                }
            }
            Err(Box::new(unavailable("receipt_reservation_uncertain")))
        }
        Err(_) => {
            release_unrun(service, &candidate, None).await;
            Err(Box::new(unavailable("receipt_store_unavailable")))
        }
    }
}

/// Admits the same request again under the abandoned admission it matches:
/// the same receipt at its next revision, plus any alias this request brings.
/// The revision CAS picks one winner among concurrent resends and fences out
/// anything still holding the abandoned revision: its prepared save fails, so
/// it never sends.
async fn readmit<A>(
    service: &Service,
    facilitator: &A,
    abandoned: Record,
    mut candidate: Record,
    keys: &[String],
    headers: &HeaderMap,
    raw: &Bytes,
) -> std::result::Result<Record, Box<Response>>
where
    A: HasProviderMap,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Only an Idempotency-Key can be new: the authorization and the purchase
    // capability are the abandoned admission's own (`same_request`).
    let mut fresh = Vec::new();
    for key in keys {
        match service.store.get(key).await {
            Ok(Some(found)) if found.receipt.receipt_id == abandoned.receipt.receipt_id => {}
            Ok(Some(_)) => {
                return Err(Box::new(failure(
                    "receipt_request_conflict",
                    StatusCode::CONFLICT,
                )))
            }
            Ok(None) => fresh.push(key.clone()),
            Err(_) => return Err(Box::new(unavailable("receipt_store_unavailable"))),
        }
    }
    candidate.receipt.receipt_id = abandoned.receipt.receipt_id.clone();
    candidate.receipt.revision = abandoned.receipt.revision + 1;
    if service.sign(&mut candidate.receipt).is_err() {
        return Err(Box::new(unavailable("receipt_signing_unavailable")));
    }
    let token = uuid::Uuid::new_v4().to_string();
    match service
        .store
        .readmit(&candidate, abandoned.receipt.revision, &fresh, &token)
        .await
    {
        Ok(true) => {
            tracing::info!(
                receipt_id = %candidate.receipt.receipt_id,
                network = %candidate.receipt.network,
                revision = candidate.receipt.revision,
                "receipt admission taken back by the request it admitted"
            );
            Ok(candidate)
        }
        Ok(false) => {
            let auth_key = format!("receipt:auth:v1:{}", candidate.receipt.authorization_id);
            let current = match service.store.get(&auth_key).await {
                Ok(Some(current)) => current,
                _ => return Err(Box::new(unavailable("receipt_store_unavailable"))),
            };
            if !is_abandoned(&current) {
                // Another resend won: this one is a replay of its admission.
                return Err(Box::new(
                    replay(service, facilitator, current, headers, raw).await,
                ));
            }
            for key in &fresh {
                match service.store.get(key).await {
                    Ok(Some(other)) if other.receipt.receipt_id != current.receipt.receipt_id => {
                        return Err(Box::new(failure(
                            "receipt_request_conflict",
                            StatusCode::CONFLICT,
                        )))
                    }
                    Err(_) => return Err(Box::new(unavailable("receipt_store_unavailable"))),
                    _ => {}
                }
            }
            Err(Box::new(resend_later(&current)))
        }
        Err(_) => {
            release_unrun(service, &candidate, Some(abandoned.receipt.revision)).await;
            Err(Box::new(unavailable("receipt_store_unavailable")))
        }
    }
}

/// Closes an admission this request wrote but never ran: a store write that
/// failed ambiguously can still have landed, or still land. The close is
/// written over the revision this request wrote, over `before` (the revision a
/// re-admission moved on from), or where there is no record yet; the write
/// landing late then finds the record taken and writes nothing. In a race it
/// can instead close another resend's re-admission at the same revision; that
/// one then cannot save prepared bytes, so it never sends, and its caller is
/// told to resend.
async fn release_unrun(service: &Service, record: &Record, before: Option<u64>) {
    let mut closed = record.clone();
    abandon(
        &mut closed,
        "receipt_store_unavailable",
        RETRY_AFTER_SECS,
        json!({}),
    );
    let held: Vec<u64> = std::iter::once(record.receipt.revision)
        .chain(before)
        .collect();
    if service.release(&closed, &held).await == Ok(true) {
        tracing::warn!(
            receipt_id = %closed.receipt.receipt_id,
            network = %closed.receipt.network,
            "receipt admission released after a store write that could not be confirmed"
        );
    }
}

pub fn active() -> bool {
    ACTIVE.try_with(|_| ()).is_ok()
}

/// The provider is ending this admission's settlement and no transaction left
/// this process. Ignored once [`sending`] ran, and outside an admission.
pub fn unsent(site: &'static str) {
    let _ = ACTIVE.try_with(|admission| {
        let mut state = admission.sending();
        if !state.latched && state.unsent.is_none() {
            state.unsent = Some(site);
        }
    });
}

/// Called immediately before a transaction may leave this process. From then
/// on the admission ends only by chain evidence or reconciliation.
pub fn sending() {
    let _ = ACTIVE.try_with(|admission| {
        let mut state = admission.sending();
        state.latched = true;
        state.unsent = None;
    });
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
    let mut record = active.record.lock().await;
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
    if let Err(error) = service.save(&mut next).await {
        // Not confirmed stored, so never sent from here. If an ambiguous write
        // did store them, the owner's closing CAS fails and nothing is released.
        unsent("prepared_evm");
        return Err(error);
    }
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
    let mut record = active.record.lock().await;
    let mut next = record.clone();
    next.prepared = Some(json!({"transactionId":id}));
    next.receipt.settlement =
        Some(json!({"id":id,"idType":"hedera-transaction-id","paymentId":Value::Null}));
    next.receipt.status = "pending".into();
    next.receipt.diagnostic_code = Some("native_transaction_identified".into());
    if let Err(error) = service.save(&mut next).await {
        // The Hedera settle stops here, before its own store or the network.
        unsent("prepared_hedera");
        return Err(error);
    }
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

/// `diagnosticCode` of an EVM admission whose own transaction reverted while
/// its authorization was used: executed by another transaction, which pays the
/// same `payTo`, or cancelled by the payer. The receipt stays `unknown`; never
/// sign a replacement.
pub const AUTHORIZATION_USED_ELSEWHERE: &str = "authorization_used_elsewhere";

/// What the chain says about the authorization behind a reverted transaction.
#[derive(Debug, PartialEq)]
enum Spent {
    /// Unused, and no block can execute it any more: the payer's funds never
    /// moved under it and never will. The only state that refuses the payment.
    Never,
    /// Used, by another transaction or a cancellation.
    Used,
    /// No verdict: unread, no key kept, or unused but still executable.
    Unknown,
}

/// The authorization behind a reverted transaction, read at one block of the
/// chain: the latest the node serves, if it is not behind the reverted one.
/// The revert alone refuses nothing: another transaction may have executed the
/// authorization first, and a mined transaction publishes the signed
/// authorization, which anyone can execute while blocks are stamped before its
/// `validBefore`. So it is `Never` only when that block is stamped at or past
/// `validBefore` (the chain's clock, not this one) and the token reports the
/// authorization unused there (`authorizationState`).
async fn authorization_spent<P: alloy::providers::Provider>(
    provider: &P,
    record: &Record,
    reverted_in: Option<u64>,
) -> Spent {
    let Some(authorization) = &record.evm_authorization else {
        return Spent::Unknown;
    };
    let Ok(token) = record.receipt.asset.parse::<alloy::primitives::Address>() else {
        return Spent::Unknown;
    };
    let token = crate::chain::evm::USDC::new(token, provider);
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let block = provider
            .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
            .await
            .ok()??;
        let (number, stamped) = (block.header.number, block.header.timestamp);
        let used = token
            .authorizationState(authorization.from, authorization.nonce)
            .call()
            .block(alloy::eips::BlockId::number(number))
            .await
            .ok()?;
        Some((number, stamped, used))
    })
    .await;
    match read {
        Ok(Some((_, _, true))) => Spent::Used,
        Ok(Some((number, stamped, false)))
            if stamped >= record.authorization_expires_at
                && reverted_in.is_some_and(|reverted| number >= reverted) =>
        {
            Spent::Never
        }
        _ => Spent::Unknown,
    }
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
    // `Some(true)` settled, `Some(false)` refused, `None` used elsewhere.
    let outcome: Option<(Option<bool>, String)> = match facilitator
        .provider_map()
        .by_network(network)
    {
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
                    let verdict = if receipt.status() {
                        Some(Some(true))
                    } else {
                        // A revert alone is no refusal (`authorization_spent`).
                        match authorization_spent(provider.inner(), record, receipt.block_number)
                            .await
                        {
                            Spent::Never => Some(Some(false)),
                            Spent::Used => Some(None),
                            Spent::Unknown => None,
                        }
                    };
                    verdict.map(|verdict| (verdict, tx.to_string()))
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
                .map(|success| (Some(success), id.to_owned()))
        }
        _ => None,
    };
    if let Some((None, _)) = outcome {
        // Funds may have moved, under a transaction this admission does not
        // name: no verdict to sign, but nothing left to resend or poll for.
        if record.receipt.diagnostic_code.as_deref() == Some(AUTHORIZATION_USED_ELSEWHERE) {
            return;
        }
        let mut updated = record.clone();
        updated.receipt.status = "unknown".into();
        updated.receipt.diagnostic_code = Some(AUTHORIZATION_USED_ELSEWHERE.into());
        updated.receipt.retry = json!({"action":"none"});
        if let Some(prepared) = updated.prepared.as_mut().and_then(Value::as_object_mut) {
            prepared.remove("signedTransaction");
        }
        if service.save(&mut updated).await.is_ok() {
            *record = updated;
        }
        return;
    }
    if let Some((Some(success), tx)) = outcome {
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

/// What a provider did under a test admission.
#[cfg(test)]
pub(crate) struct TestAdmission<T> {
    pub output: T,
    pub unsent: Option<&'static str>,
    pub latched: bool,
    pub prepared: Option<Value>,
}

/// Runs `f` as the owner of a reserved Arc testnet admission on an in-memory
/// store, so provider code can be driven through its real receipt hooks.
#[cfg(test)]
pub(crate) async fn with_test_admission<F: Future>(f: F) -> TestAdmission<F::Output> {
    let service = Arc::new(Service {
        store: Arc::new(store::MemoryStore::default()),
        signing_key: None,
    });
    let record = tests::fixture_record();
    let auth = format!("receipt:auth:v1:{}", record.receipt.authorization_id);
    assert!(service
        .store
        .reserve(&record, &[auth], "test")
        .await
        .unwrap());
    let admission = Admission::new(record);
    let output = TEST_SERVICE
        .scope(service, ACTIVE.scope(admission.clone(), f))
        .await;
    let latched = admission.sending().latched;
    let prepared = admission.record.lock().await.prepared.clone();
    TestAdmission {
        output,
        unsent: admission.unsent(),
        latched,
        prepared,
    }
}

#[cfg(test)]
mod tests;
