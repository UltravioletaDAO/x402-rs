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
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{response::IntoResponse, Json, Router};
use base64::Engine;
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

use std::collections::HashMap;
use std::sync::Arc;

use crate::chain::{FacilitatorLocalError, NetworkProvider};
use crate::discovery::{DiscoveryError, DiscoveryRegistry};
use crate::fhe_proxy::FheProxy;
use crate::facilitator::Facilitator;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, VerifyRequest,
    VerifyResponse, X402Version,
};
use crate::types_v2::{
    DiscoveryFilters, RegisterResourceRequest, SettleRequestEnvelope,
    SupportedPaymentKindsResponseV1ToV2, VerifyRequestEnvelope,
};

// Global FHE proxy instance (lazy initialized)
use once_cell::sync::Lazy;
static FHE_PROXY: Lazy<FheProxy> = Lazy::new(FheProxy::new);

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
    A: Facilitator + HasProviderMap + Clone + Send + Sync + 'static,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    Router::new()
        .route("/", get(get_root))
        .route("/verify", get(get_verify_info))
        .route("/verify", post(post_verify::<A>))
        .route("/settle", get(get_settle_info))
        .route("/settle", post(post_settle::<A>))
        .route("/health", get(get_health))
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
        .route("/algorand.png", get(get_algorand_logo))
}

/// Discovery API routes for the Bazaar feature.
///
/// These routes are separate from the main facilitator routes because they use
/// a different state type (DiscoveryRegistry).
pub fn discovery_routes() -> Router<Arc<DiscoveryRegistry>> {
    Router::new()
        .route("/discovery/resources", get(get_discovery_resources))
        .route("/discovery/register", post(post_discovery_register))
}

// ============================================================================
// Discovery Handlers (Bazaar)
// ============================================================================

/// Query parameters for GET /discovery/resources
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryQueryParams {
    /// Maximum number of resources to return (default: 10, max: 100)
    #[serde(default = "default_limit")]
    pub limit: u32,

    /// Number of resources to skip (default: 0)
    #[serde(default)]
    pub offset: u32,

    /// Filter by category
    pub category: Option<String>,

    /// Filter by network (CAIP-2 format, e.g., "eip155:8453")
    pub network: Option<String>,

    /// Filter by provider name
    pub provider: Option<String>,

    /// Filter by tag
    pub tag: Option<String>,
}

fn default_limit() -> u32 {
    10
}

impl From<DiscoveryQueryParams> for Option<DiscoveryFilters> {
    fn from(params: DiscoveryQueryParams) -> Self {
        if params.category.is_none()
            && params.network.is_none()
            && params.provider.is_none()
            && params.tag.is_none()
        {
            None
        } else {
            Some(DiscoveryFilters {
                category: params.category,
                network: params.network,
                provider: params.provider,
                tag: params.tag,
            })
        }
    }
}

/// `GET /discovery/resources`: List discoverable paid resources.
///
/// Supports pagination via `limit` and `offset` query parameters.
/// Supports filtering by `category`, `network`, `provider`, and `tag`.
///
/// # Example
/// ```text
/// GET /discovery/resources?limit=10&offset=0&category=finance&network=eip155:8453
/// ```
#[instrument(skip_all, fields(limit, offset, category, network))]
pub async fn get_discovery_resources(
    State(registry): State<Arc<DiscoveryRegistry>>,
    Query(params): Query<DiscoveryQueryParams>,
) -> impl IntoResponse {
    debug!(
        limit = params.limit,
        offset = params.offset,
        category = ?params.category,
        network = ?params.network,
        "Discovery resources query"
    );

    let filters: Option<DiscoveryFilters> = params.clone().into();
    let response = registry.list(params.limit, params.offset, filters).await;

    info!(
        total = response.pagination.total,
        returned = response.items.len(),
        "Discovery query completed"
    );

    (StatusCode::OK, Json(response))
}

/// `POST /discovery/register`: Register a new paid resource.
///
/// Registers a resource in the discovery registry so it can be discovered
/// by clients via GET /discovery/resources.
///
/// # Request Body
/// ```json
/// {
///   "url": "https://api.example.com/premium-data",
///   "type": "http",
///   "description": "Premium market data API",
///   "accepts": [{
///     "scheme": "exact",
///     "network": "eip155:8453",
///     "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
///     "amount": "10000",
///     "payTo": "0x...",
///     "maxTimeoutSeconds": 60
///   }],
///   "metadata": {
///     "category": "finance",
///     "provider": "Example Corp",
///     "tags": ["market-data", "real-time"]
///   }
/// }
/// ```
#[instrument(skip_all, fields(url))]
pub async fn post_discovery_register(
    State(registry): State<Arc<DiscoveryRegistry>>,
    Json(request): Json<RegisterResourceRequest>,
) -> impl IntoResponse {
    let url = request.url.to_string();
    info!(url = %url, resource_type = %request.resource_type, "Registering new resource");

    let resource = request.into_resource();

    match registry.register(resource).await {
        Ok(()) => {
            info!(url = %url, "Resource registered successfully");
            (
                StatusCode::CREATED,
                Json(json!({
                    "success": true,
                    "message": "Resource registered successfully",
                    "url": url
                })),
            )
                .into_response()
        }
        Err(e) => {
            warn!(url = %url, error = %e, "Failed to register resource");
            discovery_error_response(e)
        }
    }
}

/// Convert a DiscoveryError to an HTTP response.
fn discovery_error_response(error: DiscoveryError) -> Response {
    match error {
        DiscoveryError::AlreadyExists(url) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "Resource already registered",
                "url": url,
                "hint": "Use PUT /discovery/resources/{url} to update an existing resource"
            })),
        )
            .into_response(),
        DiscoveryError::NotFound(url) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "Resource not found",
                "url": url
            })),
        )
            .into_response(),
        DiscoveryError::InvalidUrl(msg) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Invalid URL",
                "details": msg
            })),
        )
            .into_response(),
        DiscoveryError::InvalidResourceType(t) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Invalid resource type",
                "received": t,
                "expected": ["http", "mcp", "a2a"]
            })),
        )
            .into_response(),
        DiscoveryError::NoPaymentMethods => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "No payment methods specified",
                "hint": "The 'accepts' array must contain at least one payment method"
            })),
        )
            .into_response(),
        DiscoveryError::StorageError(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Storage error",
                "details": e.to_string()
            })),
        )
            .into_response(),
    }
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

/// `GET /algorand.png`: Returns Algorand logo.
pub async fn get_algorand_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/algorand.png");
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
///
/// Returns v2 format response with:
/// - `kinds`: List of supported payment schemes/networks (both v1 and CAIP-2 formats)
/// - `extensions`: List of supported extensions (includes "bazaar" for discovery API)
/// - `signers`: Map of namespace to facilitator signer addresses (currently empty, reserved for future use)
#[instrument(skip_all)]
pub async fn get_supported<A>(State(facilitator): State<A>) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    match facilitator.supported().await {
        Ok(supported) => {
            // Convert v1 response to v2 with bazaar extension
            let extensions = vec!["bazaar".to_string()];
            // Signers map is empty for now - will be populated in future version
            // when we add a method to get signer addresses from the facilitator
            let signers: HashMap<String, Vec<String>> = HashMap::new();
            let v2_response = supported.to_v2(extensions, signers);
            (StatusCode::OK, Json(json!(v2_response))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// `GET /health`: Health check endpoint for load balancers and monitoring.
///
/// Returns a simple JSON response indicating the service is healthy.
/// This is used by AWS ALB health checks and monitoring tools.
#[instrument(skip_all)]
pub async fn get_health() -> impl IntoResponse {
    Json(json!({
        "status": "healthy"
    }))
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
///
/// Supports both x402 v1 and v2 protocol formats. The version is auto-detected from the
/// request body structure.
///
/// **x402 v2 Header Support**: If the `PAYMENT-SIGNATURE` header is present, the payload
/// is extracted from the base64-decoded header value instead of the request body.
#[instrument(skip_all)]
pub async fn post_verify<A>(
    State(facilitator): State<A>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    // x402 v2: Check for PAYMENT-SIGNATURE header (base64-encoded JSON)
    // If present, decode and use it instead of the body
    let body_str: String = if let Some(payment_sig) = headers.get("payment-signature") {
        match payment_sig.to_str() {
            Ok(header_value) => {
                // Base64 decode the header value
                match base64::engine::general_purpose::STANDARD.decode(header_value) {
                    Ok(decoded_bytes) => {
                        match String::from_utf8(decoded_bytes) {
                            Ok(decoded_str) => {
                                info!("Using PAYMENT-SIGNATURE header (x402 v2 format)");
                                debug!("Decoded payload length: {} bytes", decoded_str.len());
                                decoded_str
                            }
                            Err(e) => {
                                error!("PAYMENT-SIGNATURE header is not valid UTF-8: {}", e);
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(json!({
                                        "error": "PAYMENT-SIGNATURE header is not valid UTF-8"
                                    })),
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to base64 decode PAYMENT-SIGNATURE header: {}", e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": format!("Failed to decode PAYMENT-SIGNATURE header: {}", e)
                            })),
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => {
                error!("PAYMENT-SIGNATURE header contains invalid characters: {}", e);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "PAYMENT-SIGNATURE header contains invalid characters"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        // Fall back to reading from body (v1 style or direct POST)
        match std::str::from_utf8(&raw_body) {
            Ok(s) => s.to_string(),
            Err(e) => {
                error!("Failed to decode verify body as UTF-8: {}", e);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "Invalid UTF-8 in request body"
                    })),
                )
                    .into_response();
            }
        }
    };
    let body_str = body_str.as_str();

    // Check for FHE scheme BEFORE trying to parse as standard types
    // FHE requests may have different payload structures that don't match standard x402 types
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(body_str) {
        let scheme = json_value
            .get("paymentPayload")
            .and_then(|pp| pp.get("scheme"))
            .and_then(|s| s.as_str());

        if scheme == Some("fhe-transfer") {
            info!("Detected fhe-transfer scheme, routing to Zama Lambda facilitator");

            match FHE_PROXY.verify(&json_value).await {
                Ok(fhe_response) => {
                    info!(is_valid = fhe_response.is_valid, "FHE verification complete");
                    return (StatusCode::OK, Json(fhe_response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "FHE verification failed");
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": format!("FHE facilitator error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }
    }

    // Try to deserialize as envelope (supports both v1 and v2)
    let envelope: VerifyRequestEnvelope = match serde_json::from_str(body_str) {
        Ok(env) => env,
        Err(e) => {
            // Try legacy v1 format directly
            match serde_json::from_str::<VerifyRequest>(body_str) {
                Ok(v1_req) => VerifyRequestEnvelope::V1(v1_req),
                Err(_) => {
                    error!("Failed to deserialize VerifyRequest (v1 or v2): {}", e);
                    // Log first 2000 chars of the payload for debugging
                    let truncated = if body_str.len() > 2000 {
                        format!("{}... (truncated)", &body_str[..2000])
                    } else {
                        body_str.to_string()
                    };
                    warn!("Received payload: {}", truncated);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": format!("Failed to deserialize VerifyRequest: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    // Extract version and convert to v1 request for processing
    let version = envelope.version();
    let format_name = match &envelope {
        VerifyRequestEnvelope::V1(_) => "v1",
        VerifyRequestEnvelope::V2(req) => {
            debug!("Processing x402 v2 verify request with CAIP-2 network: {}", req.network());
            "v2"
        }
        VerifyRequestEnvelope::X402r(req) => {
            debug!("Processing x402r verify request with CAIP-2 network: {}", req.network());
            "x402r"
        }
        VerifyRequestEnvelope::X402rNested(req) => {
            debug!("Processing x402r-nested verify request with CAIP-2 network: {}", req.network());
            "x402r-nested"
        }
    };
    debug!("Processing x402 {} verify request", format_name);

    let v1_request = match envelope.to_v1() {
        Ok(v1_req) => v1_req,
        Err(e) => {
            error!("Failed to convert {} request to v1: {}", format_name, e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Failed to process {} request: {}", format_name, e)
                })),
            )
                .into_response();
        }
    };

    info!(
        version = ?version,
        network = ?v1_request.payment_payload.network,
        scheme = ?v1_request.payment_payload.scheme,
        "Verifying payment"
    );

    // Note: FHE transfers are handled early (before type deserialization) to support
    // custom FHE payload structures. See the fhe-transfer check above.

    // Standard exact scheme - process locally
    match facilitator.verify(&v1_request).await {
        Ok(valid_response) => (StatusCode::OK, Json(valid_response)).into_response(),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                version = ?version,
                body = %serde_json::to_string(&v1_request).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Verification failed"
            );
            error.into_response()
        }
    }
}

/// Helper function to log detailed deserialization errors for settle requests.
/// This extracts field-level information from the raw JSON to help debug malformed requests.
fn log_settle_deserialization_error(body_str: &str, e: &serde_json::Error) {
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
}

/// `POST /settle`: Facilitator-side execution of a valid x402 payment on-chain.
///
/// Given a valid [`SettleRequest`], this endpoint attempts to execute the payment
/// via ERC-3009 `transferWithAuthorization`, and returns a [`SettleResponse`] with transaction details.
///
/// This endpoint is typically called after a successful `/verify` step.
///
/// Supports both x402 v1 and v2 protocol formats. The version is auto-detected from the
/// request body structure.
///
/// Also supports x402r escrow settlement when the `refund` extension is present.
///
/// **x402 v2 Header Support**: If the `PAYMENT-SIGNATURE` header is present, the payload
/// is extracted from the base64-decoded header value instead of the request body.
#[instrument(skip_all)]
pub async fn post_settle<A>(
    State(facilitator): State<A>,
    headers: HeaderMap,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // x402 v2: Check for PAYMENT-SIGNATURE header (base64-encoded JSON)
    // If present, decode and use it instead of the body
    let body_str: String = if let Some(payment_sig) = headers.get("payment-signature") {
        match payment_sig.to_str() {
            Ok(header_value) => {
                // Base64 decode the header value
                match base64::engine::general_purpose::STANDARD.decode(header_value) {
                    Ok(decoded_bytes) => {
                        match String::from_utf8(decoded_bytes) {
                            Ok(decoded_str) => {
                                info!("Using PAYMENT-SIGNATURE header for settle (x402 v2 format)");
                                debug!("Decoded payload length: {} bytes", decoded_str.len());
                                decoded_str
                            }
                            Err(e) => {
                                error!("PAYMENT-SIGNATURE header is not valid UTF-8: {}", e);
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(json!({
                                        "error": "PAYMENT-SIGNATURE header is not valid UTF-8"
                                    })),
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to base64 decode PAYMENT-SIGNATURE header: {}", e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": format!("Failed to decode PAYMENT-SIGNATURE header: {}", e)
                            })),
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => {
                error!("PAYMENT-SIGNATURE header contains invalid characters: {}", e);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "PAYMENT-SIGNATURE header contains invalid characters"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        // Fall back to reading from body (v1 style or direct POST)
        match std::str::from_utf8(&raw_body) {
            Ok(s) => s.to_string(),
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
        }
    };
    let body_str = body_str.as_str();

    debug!("=== SETTLE REQUEST DEBUG ===");
    debug!("Raw JSON body: {}", body_str);

    // Check for FHE scheme BEFORE trying to parse as standard types
    // FHE requests may have different payload structures that don't match standard x402 types
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(body_str) {
        let scheme = json_value
            .get("paymentPayload")
            .and_then(|pp| pp.get("scheme"))
            .and_then(|s| s.as_str());

        if scheme == Some("fhe-transfer") {
            info!("Detected fhe-transfer scheme, routing settle to Zama Lambda facilitator");

            match FHE_PROXY.settle(&json_value).await {
                Ok(fhe_response) => {
                    info!("FHE settlement complete");
                    return (StatusCode::OK, Json(fhe_response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "FHE settlement failed");
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({
                            "success": false,
                            "errorReason": format!("FHE facilitator error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }

        // Check for x402r escrow/refund extension
        if let Some(extensions) = json_value
            .get("paymentPayload")
            .and_then(|pp| pp.get("extensions"))
            .and_then(|ext| ext.as_object())
        {
            if extensions.contains_key("refund") {
                // Check if escrow feature is enabled
                if !crate::escrow::is_escrow_enabled() {
                    warn!("Escrow settlement requested but ENABLE_ESCROW is not set to true");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": "Escrow settlement is disabled. Set ENABLE_ESCROW=true to enable."
                        })),
                    )
                        .into_response();
                }

                info!("Detected x402r refund extension, routing to escrow settlement");

                match crate::escrow::settle_with_escrow(body_str, &facilitator).await {
                    Ok(escrow_response) => {
                        info!("Escrow settlement complete");
                        return (StatusCode::OK, Json(escrow_response)).into_response();
                    }
                    Err(e) => {
                        error!(error = %e, "Escrow settlement failed");
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "success": false,
                                "errorReason": format!("Escrow error: {}", e)
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    }

    // Try to deserialize as envelope (supports both v1 and v2)
    let envelope: SettleRequestEnvelope = match serde_json::from_str(body_str) {
        Ok(env) => env,
        Err(e) => {
            // Try legacy v1 format directly
            match serde_json::from_str::<SettleRequest>(body_str) {
                Ok(v1_req) => SettleRequestEnvelope::V1(v1_req),
                Err(deser_err) => {
                    // Log detailed error for debugging
                    error!("[FAIL] Deserialization FAILED for both v1 and v2 formats");
                    error!("v2 Serde error: {}", e);
                    error!("v1 Serde error: {}", deser_err);
                    log_settle_deserialization_error(body_str, &deser_err);
                    debug!("=== END SETTLE REQUEST DEBUG ===");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": format!("Failed to deserialize SettleRequest: {}", deser_err),
                            "details": "Check server logs for detailed field-by-field analysis"
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    // Extract version and convert to v1 request for processing
    let version = envelope.version();
    let format_name = match &envelope {
        SettleRequestEnvelope::V1(_) => "v1",
        SettleRequestEnvelope::V2(req) => {
            debug!("Processing x402 v2 settle request with CAIP-2 network: {}", req.network());
            "v2"
        }
        SettleRequestEnvelope::X402r(req) => {
            debug!("Processing x402r settle request with CAIP-2 network: {}", req.network());
            "x402r"
        }
        SettleRequestEnvelope::X402rNested(req) => {
            debug!("Processing x402r-nested settle request with CAIP-2 network: {}", req.network());
            "x402r-nested"
        }
    };
    debug!("Processing x402 {} settle request", format_name);

    let body = match envelope.to_v1() {
        Ok(v1_req) => v1_req,
        Err(e) => {
            error!("Failed to convert {} settle request to v1: {}", format_name, e);
            debug!("=== END SETTLE REQUEST DEBUG ===");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Failed to process {} settle request: {}", format_name, e)
                })),
            )
                .into_response();
        }
    };

    // Log the parsed request details
    debug!("[OK] Deserialization SUCCEEDED (version: {:?})", version);
    debug!("Parsed SettleRequest:");
    debug!("  - x402_version: {:?}", body.x402_version);
    debug!(
        "  - payment_payload.scheme: {:?}",
        body.payment_payload.scheme
    );
    debug!(
        "  - payment_payload.network: {:?}",
        body.payment_payload.network
    );

    // Log the authorization details based on payload type
    match &body.payment_payload.payload {
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
        #[cfg(feature = "algorand")]
        crate::types::ExactPaymentPayload::Algorand(algorand_payload) => {
            debug!("  - payload type: Algorand");
            debug!(
                "  - payment_index: {}",
                algorand_payload.payment_index
            );
            debug!(
                "  - payment_group.len: {}",
                algorand_payload.payment_group.len()
            );
        }
    }

    debug!("=== END SETTLE REQUEST DEBUG ===");

    // Proceed with normal settlement logic
    // Note: FHE transfers are handled early (before type deserialization) to support
    // custom FHE payload structures. See the fhe-transfer check above.
    info!(
        "Attempting to settle payment on network: {:?}, scheme: {:?}",
        body.payment_payload.network,
        body.payment_payload.scheme
    );

    // Standard exact scheme - process locally
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
