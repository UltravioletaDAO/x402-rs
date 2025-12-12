//! HTTP endpoints implemented by the x402 **facilitator**.
//!
//! These are the server-side handlers for processing client-submitted x402 payments.
//! They include both protocol-critical endpoints (`/verify`, `/settle`) and discovery endpoints (`/supported`, etc).
//!
//! All payloads follow the types defined in the `x402-rs` crate, and are compatible
//! with the TypeScript and Go client SDKs.
//!
//! Each endpoint consumes or produces structured JSON payloads defined in `x402-rs`,
//! and is compatible with official x402 client SDKs.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{response::IntoResponse, Json, Router};
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

use crate::chain::FacilitatorLocalError;
use crate::facilitator::Facilitator;
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, VerifyRequest,
    VerifyResponse,
};

/// `GET /verify`: Returns a machine-readable description of the `/verify` endpoint.
///
/// This is served by the facilitator to help clients understand how to construct
/// a valid [`VerifyRequest`] for payment verification.
///
/// This is optional metadata and primarily useful for discoverability and debugging tools.
#[instrument(skip_all)]
pub async fn get_verify_info() -> impl IntoResponse {
    Json(json!({
        "endpoint": "/verify",
        "description": "POST to verify x402 payments",
        "body": {
            "paymentPayload": "PaymentPayload",
            "paymentRequirements": "PaymentRequirements",
        }
    }))
}

/// `GET /settle`: Returns a machine-readable description of the `/settle` endpoint.
///
/// This is served by the facilitator to describe the structure of a valid
/// [`SettleRequest`] used to initiate on-chain payment settlement.
#[instrument(skip_all)]
pub async fn get_settle_info() -> impl IntoResponse {
    Json(json!({
        "endpoint": "/settle",
        "description": "POST to settle x402 payments",
        "body": {
            "paymentPayload": "PaymentPayload",
            "paymentRequirements": "PaymentRequirements",
        }
    }))
}

pub fn routes<A>() -> Router<A>
where
    A: Facilitator + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
{
    Router::new()
        .route("/", get(get_root))
        .route("/verify", get(get_verify_info))
        .route("/verify", post(post_verify::<A>))
        .route("/settle", get(get_settle_info))
        .route("/settle", post(post_settle::<A>))
        .route("/health", get(get_health::<A>))
        .route("/version", get(get_version))
        .route("/supported", get(get_supported::<A>))
        .route("/blacklist", get(get_blacklist::<A>))
        .route("/logo.png", get(get_logo))
        .route("/favicon.ico", get(get_favicon))
        .route("/celo-colombia.png", get(get_celo_colombia_logo))
        .route("/avalanche.png", get(get_avalanche_logo))
        .route("/base.png", get(get_base_logo))
        .route("/celo.png", get(get_celo_logo))
        .route("/hyperevm.png", get(get_hyperevm_logo))
        .route("/polygon.png", get(get_polygon_logo))
        .route("/solana.png", get(get_solana_logo))
        .route("/optimism.png", get(get_optimism_logo))
        .route("/ethereum.png", get(get_ethereum_logo))
        .route("/arbitrum.png", get(get_arbitrum_logo))
        .route("/unichain.png", get(get_unichain_logo))
        .route("/monad.png", get(get_monad_logo))
        .route("/near.png", get(get_near_logo))
        .route("/stellar.png", get(get_stellar_logo))
        .route("/fogo.png", get(get_fogo_logo))
}

/// `GET /`: Returns the Ultravioleta DAO branded landing page.
#[instrument(skip_all)]
pub async fn get_root() -> impl IntoResponse {
    let html = include_str!("../static/index.html");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
}

/// Alias for `get_root` to match main.rs routing.
pub async fn get_index() -> impl IntoResponse {
    get_root().await
}

/// `GET /logo.png`: Returns Ultravioleta DAO logo.
#[instrument(skip_all)]
pub async fn get_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/logo.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /favicon.ico`: Returns favicon.
#[instrument(skip_all)]
pub async fn get_favicon() -> impl IntoResponse {
    let bytes = include_bytes!("../static/favicon.ico");
    (
        StatusCode::OK,
        [("content-type", "image/x-icon")],
        bytes.as_slice(),
    )
}

/// `GET /celo-colombia.png`: Returns Celo Colombia logo.
#[instrument(skip_all)]
pub async fn get_celo_colombia_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/celo-colombia.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /avalanche.png`: Returns Avalanche logo.
#[instrument(skip_all)]
pub async fn get_avalanche_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/avalanche.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /base.png`: Returns Base logo.
#[instrument(skip_all)]
pub async fn get_base_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/base.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /celo.png`: Returns Celo logo.
#[instrument(skip_all)]
pub async fn get_celo_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/celo.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /hyperevm.png`: Returns HyperEVM logo.
#[instrument(skip_all)]
pub async fn get_hyperevm_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/hyperevm.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /polygon.png`: Returns Polygon logo.
#[instrument(skip_all)]
pub async fn get_polygon_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/polygon.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /solana.png`: Returns Solana logo.
#[instrument(skip_all)]
pub async fn get_solana_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/solana.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /optimism.png`: Returns Optimism logo.
#[instrument(skip_all)]
pub async fn get_optimism_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/optimism.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /ethereum.png`: Returns Ethereum logo.
#[instrument(skip_all)]
pub async fn get_ethereum_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/ethereum.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

pub async fn get_arbitrum_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/arbitrum.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

pub async fn get_unichain_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/unichain.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

pub async fn get_monad_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/monad.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /near.png`: Returns NEAR Protocol logo.
#[instrument(skip_all)]
pub async fn get_near_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/near.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /stellar.png`: Returns Stellar logo.
#[instrument(skip_all)]
pub async fn get_stellar_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/stellar.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /fogo.png`: Returns FOGO logo.
pub async fn get_fogo_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/fogo.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /supported`: Lists the x402 payment schemes and networks supported by this facilitator.
///
/// Facilitators may expose this to help clients dynamically configure their payment requests
/// based on available network and scheme support.
#[instrument(skip_all)]
pub async fn get_supported<A>(State(facilitator): State<A>) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    match facilitator.supported().await {
        Ok(supported) => (StatusCode::OK, Json(json!(supported))).into_response(),
        Err(error) => error.into_response(),
    }
}

#[instrument(skip_all)]
pub async fn get_health<A>(State(facilitator): State<A>) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    get_supported(State(facilitator)).await
}

/// `GET /version`: Returns the current version of the facilitator.
///
/// This endpoint returns the version from Cargo.toml for operational visibility.
#[instrument(skip_all)]
pub async fn get_version() -> impl IntoResponse {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// `GET /blacklist`: Returns the current blacklist configuration being enforced.
///
/// This endpoint provides runtime visibility into which addresses are blocked from
/// using the facilitator. Critical for security auditing and verifying blacklist
/// enforcement is working correctly.
///
/// Response format:
/// ```json
/// {
///   "total_blocked": 2,
///   "evm_count": 1,
///   "solana_count": 1,
///   "entries": [
///     {
///       "account_type": "solana",
///       "wallet": "41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az",
///       "reason": "spam"
///     }
///   ],
///   "source": "config/blacklist.json",
///   "loaded_at_startup": true
/// }
/// ```
#[instrument(skip_all)]
pub async fn get_blacklist<A>(State(facilitator): State<A>) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    match facilitator.blacklist_info().await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// `POST /verify`: Facilitator-side verification of a proposed x402 payment.
///
/// This endpoint checks whether a given payment payload satisfies the declared
/// [`PaymentRequirements`], including signature validity, scheme match, and fund sufficiency.
///
/// Responds with a [`VerifyResponse`] indicating whether the payment can be accepted.
#[instrument(skip_all)]
pub async fn post_verify<A>(
    State(facilitator): State<A>,
    Json(body): Json<VerifyRequest>,
) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    match facilitator.verify(&body).await {
        Ok(valid_response) => (StatusCode::OK, Json(valid_response)).into_response(),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                body = %serde_json::to_string(&body).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Verification failed"
            );
            error.into_response()
        }
    }
}

/// `POST /settle`: Facilitator-side execution of a valid x402 payment on-chain.
///
/// Given a valid [`SettleRequest`], this endpoint attempts to execute the payment
/// via ERC-3009 `transferWithAuthorization`, and returns a [`SettleResponse`] with transaction details.
///
/// This endpoint is typically called after a successful `/verify` step.
#[instrument(skip_all)]
pub async fn post_settle<A>(State(facilitator): State<A>, raw_body: Bytes) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    // Log the raw JSON body
    let body_str = match std::str::from_utf8(&raw_body) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to decode body as UTF-8: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid UTF-8 in request body"
                })),
            )
                .into_response();
        }
    };

    debug!("=== SETTLE REQUEST DEBUG ===");
    debug!("Raw JSON body: {}", body_str);

    // Attempt to deserialize the SettleRequest
    let body: SettleRequest = match serde_json::from_str::<SettleRequest>(body_str) {
        Ok(req) => {
            // Deserialization succeeded - log the parsed authorization details with types
            debug!("[OK] Deserialization SUCCEEDED");
            debug!("Parsed SettleRequest:");
            debug!("  - x402_version: {:?}", req.x402_version);
            debug!(
                "  - payment_payload.scheme: {:?}",
                req.payment_payload.scheme
            );
            debug!(
                "  - payment_payload.network: {:?}",
                req.payment_payload.network
            );

            // Log the authorization details based on payload type
            match &req.payment_payload.payload {
                crate::types::ExactPaymentPayload::Evm(evm_payload) => {
                    debug!("  - payload type: EVM");
                    debug!(
                        "  - authorization.from: {} (type: EvmAddress)",
                        evm_payload.authorization.from
                    );
                    debug!(
                        "  - authorization.to: {} (type: EvmAddress)",
                        evm_payload.authorization.to
                    );
                    debug!(
                        "  - authorization.value: {} (type: TokenAmount/U256 string)",
                        evm_payload.authorization.value
                    );
                    debug!("  - authorization.validAfter: {} (type: UnixTimestamp u64 string, parsed to: {})",
                        evm_payload.authorization.valid_after.seconds_since_epoch(),
                        evm_payload.authorization.valid_after.seconds_since_epoch());
                    debug!("  - authorization.validBefore: {} (type: UnixTimestamp u64 string, parsed to: {})",
                        evm_payload.authorization.valid_before.seconds_since_epoch(),
                        evm_payload.authorization.valid_before.seconds_since_epoch());
                    debug!(
                        "  - authorization.nonce: {:?} (type: HexEncodedNonce, 32-byte hex string)",
                        evm_payload.authorization.nonce
                    );
                    debug!(
                        "  - signature: {:?} (type: EvmSignature, hex bytes)",
                        evm_payload.signature
                    );
                }
                crate::types::ExactPaymentPayload::Solana(solana_payload) => {
                    debug!("  - payload type: Solana");
                    debug!(
                        "  - transaction: {} (truncated)",
                        &solana_payload.transaction[..solana_payload.transaction.len().min(100)]
                    );
                }
                crate::types::ExactPaymentPayload::Near(near_payload) => {
                    debug!("  - payload type: NEAR");
                    debug!(
                        "  - signed_delegate_action: {} (truncated)",
                        &near_payload.signed_delegate_action[..near_payload.signed_delegate_action.len().min(100)]
                    );
                }
                crate::types::ExactPaymentPayload::Stellar(stellar_payload) => {
                    debug!("  - payload type: Stellar");
                    debug!(
                        "  - from: {}",
                        stellar_payload.from
                    );
                    debug!(
                        "  - to: {}",
                        stellar_payload.to
                    );
                    debug!(
                        "  - amount: {}",
                        stellar_payload.amount
                    );
                }
            }

            req
        }
        Err(e) => {
            // Deserialization failed - log detailed error information
            error!("[FAIL] Deserialization FAILED");
            error!("Serde error: {}", e);
            error!("Error details:");
            error!("  - Error message: {}", e.to_string());

            // Try to extract more specific information about the error
            let error_msg = e.to_string();
            if error_msg.contains("invalid type") {
                error!("  [WARN] TYPE MISMATCH detected");
            }
            if error_msg.contains("missing field") {
                error!("  [WARN] MISSING FIELD detected");
            }
            if error_msg.contains("unknown field") {
                error!("  [WARN] UNKNOWN/EXTRA FIELD detected");
            }

            // Try to parse as generic JSON to identify which field is problematic
            match serde_json::from_str::<serde_json::Value>(body_str) {
                Ok(json_value) => {
                    error!("Raw JSON parsed successfully as generic Value. Checking structure...");

                    // Check paymentPayload.payload.authorization fields
                    if let Some(payment_payload) = json_value.get("paymentPayload") {
                        error!("Found paymentPayload");

                        if let Some(payload) = payment_payload.get("payload") {
                            error!("Found paymentPayload.payload");

                            if let Some(authorization) = payload.get("authorization") {
                                error!("Found paymentPayload.payload.authorization");
                                error!("Authorization fields:");

                                // Check each field and its type
                                for (key, value) in
                                    authorization.as_object().unwrap_or(&serde_json::Map::new())
                                {
                                    let value_type = match value {
                                        serde_json::Value::String(_) => "string",
                                        serde_json::Value::Number(_) => "number",
                                        serde_json::Value::Bool(_) => "bool",
                                        serde_json::Value::Array(_) => "array",
                                        serde_json::Value::Object(_) => "object",
                                        serde_json::Value::Null => "null",
                                    };
                                    error!("  - {}: {} = {:?}", key, value_type, value);

                                    // Highlight specific problematic fields
                                    if key == "validAfter" || key == "validBefore" {
                                        if value.is_number() {
                                            error!("    [WARN] EXPECTED: string, RECEIVED: number");
                                            error!("    [WARN] This field should be a STRING like \"1732406400\", not a number");
                                        }
                                    }
                                    if key == "value" {
                                        if value.is_number() {
                                            error!("    [WARN] EXPECTED: string, RECEIVED: number");
                                            error!("    [WARN] This field should be a STRING like \"10000\", not a number");
                                        }
                                    }
                                    if key == "nonce" {
                                        if let Some(s) = value.as_str() {
                                            if !s.starts_with("0x") || s.len() != 66 {
                                                error!("    [WARN] EXPECTED: 0x-prefixed 64-char hex string (66 chars total)");
                                                error!(
                                                    "    [WARN] RECEIVED: string with length {}",
                                                    s.len()
                                                );
                                            }
                                        }
                                    }
                                }
                            } else {
                                error!("Missing paymentPayload.payload.authorization");
                            }

                            // Also log signature if present
                            if let Some(signature) = payload.get("signature") {
                                error!("Found paymentPayload.payload.signature: {:?}", signature);
                            }
                        } else {
                            error!("Missing paymentPayload.payload");
                        }
                    } else {
                        error!("Missing paymentPayload field in root");
                    }
                }
                Err(json_err) => {
                    error!("Raw JSON is malformed and cannot be parsed: {}", json_err);
                }
            }

            debug!("=== END SETTLE REQUEST DEBUG ===");

            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Failed to deserialize SettleRequest: {}", e),
                    "details": "Check server logs for detailed field-by-field analysis"
                })),
            )
                .into_response();
        }
    };

    debug!("=== END SETTLE REQUEST DEBUG ===");

    // Proceed with normal settlement logic
    info!(
        "Attempting to settle payment on network: {:?}",
        body.payment_payload.network
    );

    match facilitator.settle(&body).await {
        Ok(valid_response) => {
            // Log successful settlement with details
            if valid_response.success {
                if let Some(ref tx_hash) = valid_response.transaction {
                    info!(
                        "[OK] SETTLEMENT SUCCESSFUL - network={:?}, payer={:?}, tx_hash={:?}",
                        valid_response.network, valid_response.payer, tx_hash
                    );
                } else {
                    warn!(
                        "Settlement marked successful but no transaction hash - network={:?}, payer={:?}",
                        valid_response.network,
                        valid_response.payer
                    );
                }
            } else {
                error!(
                    "[FAIL] SETTLEMENT FAILED (success=false) - network={:?}, payer={:?}, error_reason={:?}",
                    valid_response.network,
                    valid_response.payer,
                    valid_response.error_reason
                );
            }
            (StatusCode::OK, Json(valid_response)).into_response()
        }
        Err(error) => {
            error!(
                "[FAIL] SETTLEMENT ERROR - error={:?}, network={:?}",
                error, body.payment_payload.network
            );
            warn!(
                error = ?error,
                body = %serde_json::to_string(&body).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Settlement failed"
            );
            error.into_response()
        }
    }
}

fn invalid_schema(payer: Option<MixedAddress>) -> VerifyResponse {
    VerifyResponse::invalid(payer, FacilitatorErrorReason::InvalidScheme)
}

impl IntoResponse for FacilitatorLocalError {
    fn into_response(self) -> Response {
        let error = self;

        let bad_request = (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid request".to_string(),
            }),
        )
            .into_response();

        match error {
            FacilitatorLocalError::SchemeMismatch(payer, ..) => {
                (StatusCode::OK, Json(invalid_schema(payer))).into_response()
            }
            FacilitatorLocalError::ReceiverMismatch(payer, ..)
            | FacilitatorLocalError::InvalidSignature(payer, ..)
            | FacilitatorLocalError::InvalidTiming(payer, ..)
            | FacilitatorLocalError::InsufficientValue(payer) => {
                (StatusCode::OK, Json(invalid_schema(Some(payer)))).into_response()
            }
            FacilitatorLocalError::NetworkMismatch(payer, ..)
            | FacilitatorLocalError::UnsupportedNetwork(payer) => (
                StatusCode::OK,
                Json(VerifyResponse::invalid(
                    payer,
                    FacilitatorErrorReason::InvalidNetwork,
                )),
            )
                .into_response(),
            FacilitatorLocalError::ContractCall(ref e) => {
                tracing::error!(error = %e, "ContractCall error");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Contract call failed: {}", e),
                    }),
                )
                    .into_response()
            }
            FacilitatorLocalError::InvalidAddress(ref e) => {
                tracing::error!(error = %e, "InvalidAddress error");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Invalid address: {}", e),
                    }),
                )
                    .into_response()
            }
            FacilitatorLocalError::ClockError(ref e) => {
                tracing::error!(error = ?e, "ClockError");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Clock error: {:?}", e),
                    }),
                )
                    .into_response()
            }
            FacilitatorLocalError::DecodingError(reason) => (
                StatusCode::OK,
                Json(VerifyResponse::invalid(
                    None,
                    FacilitatorErrorReason::FreeForm(reason),
                )),
            )
                .into_response(),
            FacilitatorLocalError::InsufficientFunds(payer) => (
                StatusCode::OK,
                Json(VerifyResponse::invalid(
                    Some(payer),
                    FacilitatorErrorReason::InsufficientFunds,
                )),
            )
                .into_response(),
            FacilitatorLocalError::BlockedAddress(addr, reason) => {
                tracing::warn!(address = %addr, reason = %reason, "Blocked address attempted payment");
                (
                    StatusCode::FORBIDDEN,
                    Json(ErrorResponse {
                        error: format!("Address blocked: {}", reason),
                    }),
                )
                    .into_response()
            }
            FacilitatorLocalError::Other(ref e) => {
                tracing::error!(error = %e, "Other facilitator error");
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("{}", e),
                    }),
                )
                    .into_response()
            }
        }
    }
}
