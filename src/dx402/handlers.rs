//! HTTP surface for the `durable-evidence` extension.
//!
//! Four routes, all of them read-mostly and none of them on the payment path:
//!
//! | Route | Purpose |
//! |---|---|
//! | `POST /dx402/anchor` | a resource server reports an anchor |
//! | `GET /dx402/evidence/{paymentId}` | pointer, hash and receipt |
//! | `GET /dx402/receipt/{paymentId}` | the signed receipt alone |
//! | `POST /dx402/recover` | `escrowed` only: release the wrapped key |
//!
//! Nothing here can fail a payment: settle never calls into these handlers, and
//! the settle path treats an anchoring failure as a skip.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use super::service::Dx402Service;
use super::types::{AnchorRequest, Dx402ErrorCode};

/// Routes for the DX402 extension.
pub fn dx402_routes() -> Router<Arc<Dx402Service>> {
    Router::new()
        .route("/dx402/anchor", post(post_anchor))
        .route("/dx402/evidence/{payment_id}", get(get_evidence))
        .route("/dx402/receipt/{payment_id}", get(get_receipt))
        .route("/dx402/recover", post(post_recover))
        .route("/dx402/stats", get(get_stats))
        .route("/dx402/blob/{payment_id}", get(get_blob))
}

/// Render an error code as the response shape callers branch on.
///
/// `retryable` is emitted explicitly rather than left for the caller to infer
/// from the status. The rule from `/identity/{network}/owner/{address}` applies
/// here too: a caller that records a transient failure as a permanent "no
/// evidence" turns a momentary blip into a wrong answer that never heals.
fn error_response(code: Dx402ErrorCode) -> axum::response::Response {
    let status = match code {
        Dx402ErrorCode::Dx402Disabled => StatusCode::NOT_FOUND,
        Dx402ErrorCode::Dx402UnknownPayment => StatusCode::NOT_FOUND,
        Dx402ErrorCode::Dx402EvidenceExpired => StatusCode::GONE,
        Dx402ErrorCode::Dx402NotPayer => StatusCode::FORBIDDEN,
        Dx402ErrorCode::Dx402ChallengeExpired
        | Dx402ErrorCode::Dx402ChallengeReplayed
        | Dx402ErrorCode::Dx402DirectMode => StatusCode::BAD_REQUEST,
        // 402: the anchor describes a payment we could not confirm happened.
        // Not 403 -- this is not about who the caller is, it is about whether
        // the payment behind the evidence is real.
        Dx402ErrorCode::Dx402ProofRejected => StatusCode::PAYMENT_REQUIRED,
        // 409: the request is well-formed, it just lost the race. A retry will
        // not help, so this must not read as a transient failure.
        Dx402ErrorCode::Dx402AlreadyAnchored => StatusCode::CONFLICT,
        // 422: the request is well-formed and the payment may be perfectly
        // real; what failed is the proof of authorship. Distinct from 409 so
        // the caller looks at its signature rather than at its idempotency.
        Dx402ErrorCode::Dx402SignatureNotVerified => StatusCode::UNPROCESSABLE_ENTITY,
        // Not 500: nothing is broken, the deployment simply does not offer
        // what was asked for. `/dx402/stats` lists what it does offer.
        Dx402ErrorCode::Dx402BackendUnavailable => StatusCode::UNPROCESSABLE_ENTITY,
        Dx402ErrorCode::Dx402SealedTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        Dx402ErrorCode::Dx402StoreUnavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(json!({
            "error": code.as_str(),
            "retryable": code.is_retryable(),
        })),
    )
        .into_response()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `POST /dx402/anchor` — a resource server reports sealed evidence.
///
/// The body carries metadata only. The plaintext never reaches us, and in
/// `direct` mode neither does anything that could decrypt it.
pub async fn post_anchor(
    State(svc): State<Arc<Dx402Service>>,
    Json(req): Json<AnchorRequest>,
) -> axum::response::Response {
    let chain_id = crate::dx402::service::chain_id_of(req.network);
    match svc.anchor(req, chain_id, now_secs()).await {
        Ok(evidence) => (StatusCode::CREATED, Json(evidence)).into_response(),
        Err(code) => error_response(code),
    }
}

/// `GET /dx402/evidence/{paymentId}` — pointer, hash, mode and receipt.
pub async fn get_evidence(
    State(svc): State<Arc<Dx402Service>>,
    Path(payment_id): Path<String>,
) -> axum::response::Response {
    match svc.lookup(&payment_id, now_secs()).await {
        Ok(record) => (
            StatusCode::OK,
            Json(json!({
                "paymentId": record.payment_id,
                "pointer": record.pointer,
                "backend": record.backend,
                "contentHash": record.content_hash,
                "keyAlg": record.key_alg,
                "mode": record.mode,
                "retention": record.retention,
                "anchoredAt": record.anchored_at,
                "retentionUntil": record.retention_until,
                "receipt": record.signature,
                "receiptSigner": svc.receipt_signer().to_string(),
                // Whether the payee proved this anchor is theirs. A consumer
                // that treats a provisional record as proof of who produced the
                // artifact is trusting a claim anyone could have made.
                "verified": record.verified,
                // The rung below `verified`: the claimant committed to an
                // identity, but no chain confirmed it is the payee.
                "signed": record.signed,
            })),
        )
            .into_response(),
        Err(code) => error_response(code),
    }
}

/// `GET /dx402/receipt/{paymentId}` — the signed claim, for offline verification.
pub async fn get_receipt(
    State(svc): State<Arc<Dx402Service>>,
    Path(payment_id): Path<String>,
) -> axum::response::Response {
    match svc.lookup(&payment_id, now_secs()).await {
        Ok(record) => (
            StatusCode::OK,
            Json(json!({
                "receipt": record.receipt,
                "signature": record.signature,
                "signer": svc.receipt_signer().to_string(),
                "domain": {
                    "name": "DX402 Evidence",
                    "version": "1",
                    "chainId": crate::dx402::service::chain_id_of(record.receipt.network),
                },
            })),
        )
            .into_response(),
        Err(code) => error_response(code),
    }
}

/// `GET /dx402/blob/{paymentId}` — the sealed ciphertext itself.
///
/// Unauthenticated on purpose. In `direct` mode the bytes are sealed to the
/// payer's own public key, so handing them to anyone who asks reveals nothing:
/// the access control lives in the cryptography rather than in an ACL that could
/// be misconfigured. That is also why the evidence bucket stays private and is
/// never exposed to the internet — this route is the only way in, and it can
/// only ever serve ciphertext.
///
/// `Cache-Control: public` is deliberate and safe for the same reason. Attack III
/// of *Five Attacks on x402* measured 100% leakage of paid responses through an
/// nginx cache; an intermediary that caches *this* stores something unreadable.
pub async fn get_blob(
    State(svc): State<Arc<Dx402Service>>,
    Path(payment_id): Path<String>,
) -> axum::response::Response {
    match svc.fetch_sealed(&payment_id, now_secs()).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
                (
                    axum::http::header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(code) => error_response(code),
    }
}

/// `GET /dx402/stats` — how much evidence this facilitator has notarised.
///
/// Like every other number this facilitator publishes about itself, this is a
/// **floor**, not a ledger. An anchor whose registry write failed is real
/// evidence that is not counted here.
pub async fn get_stats(State(svc): State<Arc<Dx402Service>>) -> axum::response::Response {
    (
        StatusCode::OK,
        Json(json!({
            "anchored": svc.count().await,
            "mode": svc.config().default_retention.to_string(),
            "backend": svc.config().backend.to_string(),
            // What this deployment can offer, derived from its configuration.
            // The landing page and any SDK read THIS instead of carrying a
            // hardcoded list: what exists depends on the deployment, and an
            // integrator may be pointed at a facilitator that is not ours.
            "backends": svc.config().offers(),
            "receiptSigner": svc.receipt_signer().to_string(),
            "note": "anchored is a floor: records whose index write failed are not counted",
        })),
    )
        .into_response()
}

/// `POST /dx402/recover` — release a wrapped key in `escrowed` mode.
///
/// Not implemented in v0.1. `direct` mode -- the default and the whole point --
/// needs no recovery endpoint at all, because the buyer already holds the only
/// key that opens the payload.
///
/// This returns an honest 501 rather than a stub that appears to work. An
/// endpoint that looked functional here would invite integrators to build an
/// escrowed flow against a check that does not exist yet.
pub async fn post_recover(
    State(_svc): State<Arc<Dx402Service>>,
    body: Option<Json<serde_json::Value>>,
) -> axum::response::Response {
    let _ = body;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "dx402_recover_not_implemented",
            "retryable": false,
            "detail": "escrowed mode is not available in v0.1; direct mode needs no recovery \
                       endpoint because the payer already holds the decryption key",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx402::types::{DurablePointer, EvidenceMode, KeyAlg, Retention, StorageBackend};
    use crate::network::Network;
    use alloy::signers::local::PrivateKeySigner;

    fn addr(s: &str) -> crate::types::MixedAddress {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    fn request() -> AnchorRequest {
        AnchorRequest {
            payment_id: format!("0x{}", "11".repeat(32)),
            network: Network::Base,
            tx_hash: format!("0x{}", "33".repeat(32)),
            payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
            payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
            pointer: Some(DurablePointer("mem://x".into())),
            sealed: None,
            backend: StorageBackend::S3,
            content_hash: format!("0x{}", "22".repeat(32)),
            key_alg: KeyAlg::Secp256k1,
            mode: EvidenceMode::Direct,
            retention: Retention::Days90,
            proof_of_payment: None,
            seller_signature: None,
            wrapped_cek: None,
        }
    }

    #[test]
    fn status_codes_separate_the_failure_kinds() {
        // 404 "never existed" and 410 "expired" are different answers to a
        // dispute, and 503 is the only one a caller should retry.
        let cases = [
            (Dx402ErrorCode::Dx402UnknownPayment, StatusCode::NOT_FOUND),
            (Dx402ErrorCode::Dx402EvidenceExpired, StatusCode::GONE),
            (Dx402ErrorCode::Dx402NotPayer, StatusCode::FORBIDDEN),
            (
                Dx402ErrorCode::Dx402StoreUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (Dx402ErrorCode::Dx402DirectMode, StatusCode::BAD_REQUEST),
        ];
        for (code, expected) in cases {
            assert_eq!(error_response(code).status(), expected, "{code:?}");
        }
    }

    #[tokio::test]
    async fn anchoring_then_looking_up_returns_the_receipt() {
        let signer = PrivateKeySigner::random();
        let expected_signer = signer.address();
        let svc = Arc::new(Dx402Service::in_memory(signer));

        let created = post_anchor(State(svc.clone()), Json(request())).await;
        assert_eq!(created.status(), StatusCode::CREATED);

        let looked_up = get_evidence(State(svc.clone()), Path(request().payment_id)).await;
        assert_eq!(looked_up.status(), StatusCode::OK);

        let record = svc.lookup(&request().payment_id, now_secs()).await.unwrap();
        assert!(crate::dx402::receipt::verify(
            &record.receipt,
            &record.signature,
            expected_signer,
            8453
        ));
    }

    #[tokio::test]
    async fn an_unknown_payment_is_a_404() {
        let svc = Arc::new(Dx402Service::in_memory(PrivateKeySigner::random()));
        let res = get_evidence(State(svc), Path("0xnope".to_string())).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn recover_is_honestly_unimplemented() {
        let svc = Arc::new(Dx402Service::in_memory(PrivateKeySigner::random()));
        let res = post_recover(State(svc), None).await;
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
