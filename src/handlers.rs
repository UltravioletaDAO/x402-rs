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
use axum::extract::{Extension, Path, Query, State};
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
use crate::chain::evm::MetaEvmProvider;
use crate::discovery::{DiscoveryError, DiscoveryRegistry};
use crate::fhe_proxy::FheProxy;
use crate::facilitator::Facilitator;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, VerifyRequest,
    VerifyResponse,
};
use crate::erc8004::{
    FeedbackRequest, FeedbackResponse, IReputationRegistry, IIdentityRegistry,
    get_contracts, is_erc8004_supported, supported_network_names,
    ReputationSummary, FeedbackEntry, AgentIdentity,
    RevokeFeedbackRequest, AppendResponseRequest, ReputationResponse,
};
use crate::types_v2::{
    DiscoveryFilters, DiscoveryResource, RegisterResourceRequest, SettleRequestEnvelope,
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
        // ERC-8004 Reputation endpoints
        .route("/feedback", get(get_feedback_info))
        .route("/feedback", post(post_feedback::<A>))
        .route("/feedback/revoke", post(post_revoke_feedback::<A>))
        .route("/feedback/response", post(post_append_response::<A>))
        .route("/reputation/{network}/{agent_id}", get(get_reputation::<A>))
        // ERC-8004 Identity endpoints
        .route("/identity/{network}/{agent_id}", get(get_identity::<A>))
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
        .route("/bsc.png", get(get_bsc_logo))
        .route("/sui.png", get(get_sui_logo))
        .route("/skale.png", get(get_skale_logo))
        .route("/scroll.png", get(get_scroll_logo))
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

    /// Filter by discovery source (self_registered, settlement, crawled, aggregated)
    pub source: Option<String>,

    /// Filter by source facilitator (e.g., "coinbase", "ultravioleta")
    pub source_facilitator: Option<String>,
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
            && params.source.is_none()
            && params.source_facilitator.is_none()
        {
            None
        } else {
            Some(DiscoveryFilters {
                category: params.category,
                network: params.network,
                provider: params.provider,
                tag: params.tag,
                source: params.source,
                source_facilitator: params.source_facilitator,
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

/// `GET /bsc.png`: Returns BSC (BNB Smart Chain) logo.
pub async fn get_bsc_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/bsc.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /sui.png`: Returns Sui logo.
pub async fn get_sui_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/sui.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /skale.png`: Returns SKALE logo.
pub async fn get_skale_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/skale.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /scroll.png`: Returns Scroll logo.
pub async fn get_scroll_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/scroll.png");
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
///
/// **Phase 2 Settlement Tracking**: After successful settlement, if `discoverable=true`
/// is set in the payment requirements extra field, the resource is auto-registered
/// in the Bazaar discovery registry.
#[instrument(skip_all)]
pub async fn post_settle<A>(
    State(facilitator): State<A>,
    Extension(discovery_registry): Extension<Arc<DiscoveryRegistry>>,
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
            debug!("  - payload type: Algorand (atomic group)");
            debug!(
                "  - payment_index: {}",
                algorand_payload.payment_index
            );
            debug!(
                "  - payment_group.len: {}",
                algorand_payload.payment_group.len()
            );
        }
        #[cfg(feature = "sui")]
        crate::types::ExactPaymentPayload::Sui(sui_payload) => {
            debug!("  - payload type: Sui (sponsored transaction)");
            debug!(
                "  - from: {}",
                sui_payload.from
            );
            debug!(
                "  - to: {}",
                sui_payload.to
            );
            debug!(
                "  - amount: {}",
                sui_payload.amount
            );
            debug!(
                "  - coin_object_id: {}",
                sui_payload.coin_object_id
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

                // Phase 2: Settlement Tracking - check if discoverable=true
                let is_discoverable = body
                    .payment_requirements
                    .extra
                    .as_ref()
                    .and_then(|e| e.get("discoverable"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if is_discoverable {
                    // Convert v1 PaymentRequirements to v2 for the accepts array
                    use crate::types_v2::PaymentRequirementsV1ToV2;
                    let (_resource_info, requirements_v2) = body.payment_requirements.to_v2();

                    // Create a DiscoveryResource from the settlement
                    let discovery_resource = DiscoveryResource::from_settlement(
                        body.payment_requirements.resource.clone(),
                        "http".to_string(), // Default to HTTP resource type
                        body.payment_requirements.description.clone(),
                        vec![requirements_v2],
                    );

                    // Track the settlement (register or increment count)
                    let registry = discovery_registry.clone();
                    let resource_url = discovery_resource.url.to_string();
                    tokio::spawn(async move {
                        match registry.track_settlement(discovery_resource).await {
                            Ok(is_new) => {
                                if is_new {
                                    info!(
                                        url = %resource_url,
                                        "Auto-registered new resource from settlement (discoverable=true)"
                                    );
                                } else {
                                    debug!(
                                        url = %resource_url,
                                        "Incremented settlement count for existing resource"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    url = %resource_url,
                                    error = %e,
                                    "Failed to track settlement in discovery registry"
                                );
                            }
                        }
                    });
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

// ============================================================================
// ERC-8004 Feedback Handlers
// ============================================================================

/// `GET /feedback`: Returns a machine-readable description of the `/feedback` endpoint.
///
/// This endpoint provides metadata about how to submit reputation feedback
/// using the ERC-8004 Trustless Agents protocol.
#[instrument(skip_all)]
pub async fn get_feedback_info() -> impl IntoResponse {
    let networks = supported_network_names();

    Json(json!({
        "endpoint": "/feedback",
        "description": "POST to submit ERC-8004 reputation feedback on-chain",
        "extension": "8004-reputation",
        "specification": "https://eips.ethereum.org/EIPS/eip-8004",
        "body": {
            "x402Version": "number (1 or 2)",
            "network": format!("string (e.g., '{}' or 'eip155:1')", networks.first().unwrap_or(&"ethereum-mainnet")),
            "feedback": {
                "agentId": "number - Agent's token ID in the Identity Registry",
                "value": "number - Feedback value (fixed-point, e.g., 87 means 87/100)",
                "valueDecimals": "number (0-18) - Decimal places for value interpretation",
                "tag1": "string - Primary categorization tag (e.g., 'starred', 'uptime', 'responseTime')",
                "tag2": "string - Secondary categorization tag",
                "endpoint": "string (optional) - Service endpoint that was used",
                "feedbackUri": "string (optional) - URI to off-chain feedback file (IPFS, HTTPS)",
                "feedbackHash": "string (optional) - Keccak256 hash of feedback content (32 bytes hex)",
                "proof": {
                    "transactionHash": "string - Settlement transaction hash",
                    "blockNumber": "number - Block number of settlement",
                    "network": "string - Network where settlement occurred",
                    "payer": "address - Address that paid",
                    "payee": "address - Address that received payment",
                    "amount": "string - Amount paid in token base units",
                    "token": "address - Token contract address",
                    "timestamp": "number - Unix timestamp",
                    "paymentHash": "string - Keccak256 hash of payment data"
                }
            }
        },
        "endpoints": {
            "POST /feedback": "Submit new feedback",
            "POST /feedback/revoke": "Revoke previously submitted feedback",
            "POST /feedback/response": "Append response to feedback (agent only)",
            "GET /reputation/:network/:agentId": "Get reputation summary for an agent",
            "GET /identity/:network/:agentId": "Get agent identity from Identity Registry"
        },
        "supportedNetworks": networks
    }))
}

/// `POST /feedback`: Submit ERC-8004 reputation feedback on-chain.
///
/// Given a valid [`FeedbackRequest`] with feedback parameters, this endpoint
/// submits the reputation feedback to the ERC-8004 Reputation Registry contract.
///
/// The feedback follows the official ERC-8004 specification with full parameter support:
/// - agentId: The agent's token ID in the Identity Registry
/// - value: Fixed-point feedback value (e.g., 87 with decimals=0 means 87/100)
/// - valueDecimals: Decimal places for value interpretation (0-18)
/// - tag1, tag2: Categorization tags (e.g., "starred", "uptime", "responseTime")
/// - endpoint: Service endpoint that was used (optional)
/// - feedbackURI: URI to off-chain feedback file (IPFS, HTTPS) (optional)
/// - feedbackHash: Keccak256 hash of feedback content (optional)
///
/// # Errors
///
/// - Returns 400 if the network doesn't support ERC-8004
/// - Returns 400 if required fields are missing
/// - Returns 500 if the on-chain submission fails
#[instrument(skip_all)]
pub async fn post_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse the request body
    let request: FeedbackRequest = match serde_json::from_slice(&raw_body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse feedback request: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(FeedbackResponse {
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some(format!("Invalid request format: {}", e)),
                    network: crate::network::Network::Ethereum, // Placeholder
                }),
            )
                .into_response();
        }
    };

    let network = request.network;

    // Check if the network supports ERC-8004
    if !is_erc8004_supported(&network) {
        let supported = supported_network_names();
        warn!(
            network = %network,
            "ERC-8004 feedback not supported on this network"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(FeedbackResponse {
                success: false,
                transaction: None,
                feedback_index: None,
                error: Some(format!(
                    "ERC-8004 is not supported on network {}. Supported networks: {:?}",
                    network, supported
                )),
                network,
            }),
        )
            .into_response();
    }

    // Get the contract addresses for this network
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(FeedbackResponse {
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some(format!("No ERC-8004 contracts configured for network {}", network)),
                    network,
                }),
            )
                .into_response();
        }
    };

    let feedback = &request.feedback;

    info!(
        network = %network,
        agent_id = feedback.agent_id,
        value = feedback.value,
        value_decimals = feedback.value_decimals,
        tag1 = %feedback.tag1,
        "Processing ERC-8004 feedback submission"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            error!(network = %network, "No EVM provider available for network");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some(format!("No EVM provider available for network {}", network)),
                    network,
                }),
            )
                .into_response();
        }
    };

    // Create the contract instance
    let reputation_registry =
        IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

    // Convert feedbackHash to bytes32 (default to zero if not provided)
    let feedback_hash = feedback.feedback_hash.unwrap_or_default();

    // Build and send the transaction using official ERC-8004 giveFeedback function
    let call = reputation_registry.giveFeedback(
        alloy::primitives::U256::from(feedback.agent_id),
        feedback.value,
        feedback.value_decimals,
        feedback.tag1.clone(),
        feedback.tag2.clone(),
        feedback.endpoint.clone(),
        feedback.feedback_uri.clone(),
        feedback_hash,
    );

    match call.send().await {
        Ok(pending_tx) => {
            // Wait for the transaction to be mined
            match pending_tx.get_receipt().await {
                Ok(receipt) => {
                    let tx_hash = receipt.transaction_hash;
                    info!(
                        network = %network,
                        tx = %tx_hash,
                        agent_id = feedback.agent_id,
                        "ERC-8004 feedback submitted successfully"
                    );

                    // TODO: Parse logs to extract feedbackIndex from NewFeedback event
                    let feedback_index = None; // Would need log parsing

                    (
                        StatusCode::OK,
                        Json(FeedbackResponse {
                            success: true,
                            transaction: Some(crate::types::TransactionHash::Evm(tx_hash.0)),
                            feedback_index,
                            error: None,
                            network,
                        }),
                    )
                        .into_response()
                }
                Err(e) => {
                    error!(
                        network = %network,
                        error = %e,
                        "Failed to get transaction receipt for feedback"
                    );
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            success: false,
                            transaction: None,
                            feedback_index: None,
                            error: Some(format!("Transaction failed: {}", e)),
                            network,
                        }),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            error!(
                network = %network,
                error = %e,
                "Failed to submit feedback transaction"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    success: false,
                    transaction: None,
                    feedback_index: None,
                    error: Some(format!("Failed to submit transaction: {}", e)),
                    network,
                }),
            )
                .into_response()
        }
    }
}

/// `POST /feedback/revoke`: Revoke previously submitted ERC-8004 feedback.
///
/// Allows a client to revoke their own feedback. Only the original submitter
/// can revoke their feedback.
///
/// # Request Body
/// ```json
/// {
///   "x402Version": 1,
///   "network": "ethereum-mainnet",
///   "agentId": 42,
///   "feedbackIndex": 1
/// }
/// ```
#[instrument(skip_all)]
pub async fn post_revoke_feedback<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse the request body
    let request: RevokeFeedbackRequest = match serde_json::from_slice(&raw_body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse revoke feedback request: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("Invalid request format: {}", e)
                })),
            )
                .into_response();
        }
    };

    let network = request.network;

    // Check if the network supports ERC-8004
    if !is_erc8004_supported(&network) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("ERC-8004 is not supported on network {}", network)
            })),
        )
            .into_response();
    }

    // Get contracts for this network
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("No ERC-8004 contracts for network {}", network)
                })),
            )
                .into_response();
        }
    };

    info!(
        network = %network,
        agent_id = request.agent_id,
        feedback_index = request.feedback_index,
        "Revoking ERC-8004 feedback"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": format!("No EVM provider available for network {}", network)
                })),
            )
                .into_response();
        }
    };

    // Create contract instance and call revokeFeedback
    let reputation_registry =
        IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

    let call = reputation_registry.revokeFeedback(
        alloy::primitives::U256::from(request.agent_id),
        request.feedback_index,
    );

    match call.send().await {
        Ok(pending_tx) => {
            match pending_tx.get_receipt().await {
                Ok(receipt) => {
                    let tx_hash = receipt.transaction_hash;
                    info!(
                        network = %network,
                        tx = %tx_hash,
                        "ERC-8004 feedback revoked successfully"
                    );

                    (
                        StatusCode::OK,
                        Json(json!({
                            "success": true,
                            "transaction": format!("0x{}", hex::encode(tx_hash.0)),
                            "network": network.to_string()
                        })),
                    )
                        .into_response()
                }
                Err(e) => {
                    error!(error = %e, "Failed to get transaction receipt");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false,
                            "error": format!("Transaction failed: {}", e)
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to send revoke transaction");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": format!("Failed to submit transaction: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// `POST /feedback/response`: Append a response to feedback.
///
/// Allows an agent (or authorized party) to respond to feedback they received.
///
/// # Request Body
/// ```json
/// {
///   "x402Version": 1,
///   "network": "ethereum-mainnet",
///   "agentId": 42,
///   "clientAddress": "0x...",
///   "feedbackIndex": 1,
///   "responseUri": "ipfs://QmResponse...",
///   "responseHash": "0x..."
/// }
/// ```
#[instrument(skip_all)]
pub async fn post_append_response<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse the request body
    let request: AppendResponseRequest = match serde_json::from_slice(&raw_body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse append response request: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("Invalid request format: {}", e)
                })),
            )
                .into_response();
        }
    };

    let network = request.network;

    // Check if the network supports ERC-8004
    if !is_erc8004_supported(&network) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": format!("ERC-8004 is not supported on network {}", network)
            })),
        )
            .into_response();
    }

    // Get contracts for this network
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("No ERC-8004 contracts for network {}", network)
                })),
            )
                .into_response();
        }
    };

    // Extract client address
    let client_addr = match &request.client_address {
        MixedAddress::Evm(addr) => addr.0,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "Client address must be an EVM address"
                })),
            )
                .into_response();
        }
    };

    info!(
        network = %network,
        agent_id = request.agent_id,
        feedback_index = request.feedback_index,
        "Appending response to ERC-8004 feedback"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": format!("No EVM provider available for network {}", network)
                })),
            )
                .into_response();
        }
    };

    // Create contract instance and call appendResponse
    let reputation_registry =
        IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

    let response_hash = request.response_hash.unwrap_or_default();

    let call = reputation_registry.appendResponse(
        alloy::primitives::U256::from(request.agent_id),
        client_addr,
        request.feedback_index,
        request.response_uri.clone(),
        response_hash,
    );

    match call.send().await {
        Ok(pending_tx) => {
            match pending_tx.get_receipt().await {
                Ok(receipt) => {
                    let tx_hash = receipt.transaction_hash;
                    info!(
                        network = %network,
                        tx = %tx_hash,
                        "ERC-8004 response appended successfully"
                    );

                    (
                        StatusCode::OK,
                        Json(json!({
                            "success": true,
                            "transaction": format!("0x{}", hex::encode(tx_hash.0)),
                            "network": network.to_string()
                        })),
                    )
                        .into_response()
                }
                Err(e) => {
                    error!(error = %e, "Failed to get transaction receipt");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false,
                            "error": format!("Transaction failed: {}", e)
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to send append response transaction");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": format!("Failed to submit transaction: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// Path parameters for reputation query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReputationPathParams {
    pub network: String,
    pub agent_id: u64,
}

/// Query parameters for reputation query
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReputationQueryParams {
    /// Filter by tag1
    #[serde(default)]
    pub tag1: String,
    /// Filter by tag2
    #[serde(default)]
    pub tag2: String,
    /// Include individual feedback entries
    #[serde(default)]
    pub include_feedback: bool,
}

/// `GET /reputation/:network/:agent_id`: Get reputation summary for an agent.
///
/// Returns the aggregated reputation summary from the ERC-8004 Reputation Registry.
///
/// # Query Parameters
/// - `tag1`: Filter by primary tag (optional)
/// - `tag2`: Filter by secondary tag (optional)
/// - `includeFeedback`: Include individual feedback entries (optional, default false)
///
/// # Example
/// ```text
/// GET /reputation/ethereum-mainnet/42?includeFeedback=true
/// ```
#[instrument(skip_all)]
pub async fn get_reputation<A>(
    State(facilitator): State<A>,
    Path(params): Path<ReputationPathParams>,
    Query(query): Query<ReputationQueryParams>,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse network from path
    let network: crate::network::Network = match params.network.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid network: {}", params.network)
                })),
            )
                .into_response();
        }
    };

    // Check if the network supports ERC-8004
    if !is_erc8004_supported(&network) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("ERC-8004 is not supported on network {}", network),
                "supportedNetworks": supported_network_names()
            })),
        )
            .into_response();
    }

    // Get contracts for this network
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("No ERC-8004 contracts for network {}", network)
                })),
            )
                .into_response();
        }
    };

    info!(
        network = %network,
        agent_id = params.agent_id,
        tag1 = %query.tag1,
        tag2 = %query.tag2,
        "Querying ERC-8004 reputation"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("No EVM provider available for network {}", network)
                })),
            )
                .into_response();
        }
    };

    // Create contract instance
    let reputation_registry =
        IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

    // Call getSummary on the contract
    let summary_call = reputation_registry.getSummary(
        alloy::primitives::U256::from(params.agent_id),
        vec![], // Empty client filter = all clients
        query.tag1.clone(),
        query.tag2.clone(),
    );

    match summary_call.call().await {
        Ok(result) => {
            let summary = ReputationSummary {
                agent_id: params.agent_id,
                count: result.count,
                summary_value: result.summaryValue,
                summary_value_decimals: result.summaryValueDecimals,
                network: network.clone(),
            };

            // Optionally fetch individual feedback entries
            let feedback_entries: Option<Vec<FeedbackEntry>> = if query.include_feedback {
                // Call readAllFeedback
                let feedback_call = reputation_registry.readAllFeedback(
                    alloy::primitives::U256::from(params.agent_id),
                    vec![], // All clients
                    query.tag1.clone(),
                    query.tag2.clone(),
                    false, // Don't include revoked
                );

                match feedback_call.call().await {
                    Ok(fb_result) => {
                        let entries: Vec<FeedbackEntry> = fb_result.clients
                            .iter()
                            .zip(fb_result.feedbackIndexes.iter())
                            .zip(fb_result.values.iter())
                            .zip(fb_result.valueDecimals.iter())
                            .zip(fb_result.tag1s.iter())
                            .zip(fb_result.tag2s.iter())
                            .zip(fb_result.revokedStatuses.iter())
                            .map(|((((((client, idx), val), dec), t1), t2), revoked)| {
                                FeedbackEntry {
                                    client: MixedAddress::Evm(crate::types::EvmAddress(*client)),
                                    feedback_index: *idx,
                                    value: *val,
                                    value_decimals: *dec,
                                    tag1: t1.clone(),
                                    tag2: t2.clone(),
                                    is_revoked: *revoked,
                                }
                            })
                            .collect();
                        Some(entries)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to fetch feedback entries, returning summary only");
                        None
                    }
                }
            } else {
                None
            };

            let response = ReputationResponse {
                agent_id: params.agent_id,
                summary,
                feedback: feedback_entries,
                network,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(
                network = %network,
                agent_id = params.agent_id,
                error = %e,
                "Failed to query reputation"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to query reputation: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// Path parameters for identity query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IdentityPathParams {
    pub network: String,
    pub agent_id: u64,
}

/// `GET /identity/:network/:agent_id`: Get agent identity from the ERC-8004 Identity Registry.
///
/// Returns the agent's identity information including:
/// - Owner address
/// - Agent URI (metadata file location)
/// - Payment wallet (if set)
///
/// # Example
/// ```text
/// GET /identity/ethereum-mainnet/42
/// ```
#[instrument(skip_all)]
pub async fn get_identity<A>(
    State(facilitator): State<A>,
    Path(params): Path<IdentityPathParams>,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse network from path
    let network: crate::network::Network = match params.network.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid network: {}", params.network)
                })),
            )
                .into_response();
        }
    };

    // Check if the network supports ERC-8004
    if !is_erc8004_supported(&network) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("ERC-8004 is not supported on network {}", network),
                "supportedNetworks": supported_network_names()
            })),
        )
            .into_response();
    }

    // Get contracts for this network
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("No ERC-8004 contracts for network {}", network)
                })),
            )
                .into_response();
        }
    };

    info!(
        network = %network,
        agent_id = params.agent_id,
        "Querying ERC-8004 agent identity"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("No EVM provider available for network {}", network)
                })),
            )
                .into_response();
        }
    };

    // Create contract instance
    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());

    let agent_id_u256 = alloy::primitives::U256::from(params.agent_id);

    // Check if agent exists
    let exists_call = identity_registry.exists(agent_id_u256);
    match exists_call.call().await {
        Ok(exists) => {
            if !exists {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": format!("Agent {} not found in Identity Registry", params.agent_id)
                    })),
                )
                    .into_response();
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to check if agent exists");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to check agent existence: {}", e)
                })),
            )
                .into_response();
        }
    }

    // Get owner, URI, and wallet in parallel
    let owner_call = identity_registry.ownerOf(agent_id_u256);
    let uri_call = identity_registry.tokenURI(agent_id_u256);
    let wallet_call = identity_registry.getAgentWallet(agent_id_u256);

    let (owner_result, uri_result, wallet_result) = tokio::join!(
        owner_call.call(),
        uri_call.call(),
        wallet_call.call()
    );

    let owner = match owner_result {
        Ok(o) => MixedAddress::Evm(crate::types::EvmAddress(o)),
        Err(e) => {
            error!(error = %e, "Failed to get agent owner");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to get agent owner: {}", e)
                })),
            )
                .into_response();
        }
    };

    let agent_uri = match uri_result {
        Ok(u) => u,
        Err(e) => {
            warn!(error = %e, "Failed to get agent URI, using empty string");
            String::new()
        }
    };

    let agent_wallet = match wallet_result {
        Ok(w) => {
            if w == alloy::primitives::Address::ZERO {
                None
            } else {
                Some(MixedAddress::Evm(crate::types::EvmAddress(w)))
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to get agent wallet");
            None
        }
    };

    let identity = AgentIdentity {
        agent_id: params.agent_id,
        owner,
        agent_uri,
        agent_wallet,
        network,
    };

    (StatusCode::OK, Json(identity)).into_response()
}
