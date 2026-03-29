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

use alloy::providers::Provider as _;
use crate::chain::evm::MetaEvmProvider;
use crate::chain::{FacilitatorLocalError, NetworkProvider, NetworkProviderOps};
use crate::discovery::{DiscoveryError, DiscoveryRegistry};
use crate::erc8004::{
    get_contracts, is_erc8004_supported, parse_agent_id_value, supported_network_names,
    AgentIdentity, AppendResponseRequest, AtomStatsResponse, FeedbackEntry, FeedbackRequest,
    FeedbackResponse, IIdentityRegistry, IReputationRegistry, MetadataEntry, RegisterAgentRequest,
    RegisterAgentResponse, ReputationResponse, ReputationSummary, RevokeFeedbackRequest,
};
use crate::erc8004::solana as solana_erc8004;
use solana_sdk::signer::Signer as _;
use crate::facilitator::Facilitator;
use crate::fhe_proxy::FheProxy;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, VerifyRequest,
    VerifyResponse,
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
        // Escrow state query endpoint
        .route("/escrow/state", post(post_escrow_state::<A>))
        // ERC-8004 Registration endpoints
        .route("/register", get(get_register_info))
        .route("/register", post(post_register::<A>))
        // ERC-8004 Reputation endpoints
        .route("/feedback", get(get_feedback_info))
        .route("/feedback", post(post_feedback::<A>))
        .route("/feedback/revoke", post(post_revoke_feedback::<A>))
        .route("/feedback/response", post(post_append_response::<A>))
        .route("/reputation/{network}/{agent_id}", get(get_reputation::<A>))
        // ERC-8004 Identity endpoints
        .route("/identity/{network}/{agent_id}", get(get_identity::<A>))
        .route(
            "/identity/{network}/{agent_id}/metadata/{key}",
            get(get_identity_metadata::<A>),
        )
        .route(
            "/identity/{network}/total-supply",
            get(get_identity_total_supply::<A>),
        )
        .route("/health", get(get_health))
        .route("/version", get(get_version))
        .route("/supported", get(get_supported::<A>))
        .route("/accepts", post(post_accepts::<A>))
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
        .route("/usdc.png", get(get_usdc_logo))
        .route("/usdt.png", get(get_usdt_logo))
        .route("/eurc.png", get(get_eurc_logo))
        .route("/ausd.png", get(get_ausd_logo))
        .route("/pyusd.png", get(get_pyusd_logo))
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

/// `GET /usdc.png`: Returns USDC stablecoin logo.
pub async fn get_usdc_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/usdc.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /usdt.png`: Returns USDT stablecoin logo.
pub async fn get_usdt_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/usdt.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /eurc.png`: Returns EURC stablecoin logo.
pub async fn get_eurc_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/eurc.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /ausd.png`: Returns AUSD stablecoin logo.
pub async fn get_ausd_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/ausd.png");
    (
        StatusCode::OK,
        [("content-type", "image/png")],
        bytes.as_slice(),
    )
}

/// `GET /pyusd.png`: Returns PYUSD stablecoin logo.
pub async fn get_pyusd_logo() -> impl IntoResponse {
    let bytes = include_bytes!("../static/pyusd.png");
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

/// `POST /accepts`: Negotiation endpoint for Faremeter middleware compatibility.
///
/// Receives merchant payment requirements, matches them against the facilitator's
/// supported capabilities, and returns enriched requirements with facilitator data
/// (feePayer, tokens, escrow contracts, etc.).
///
/// This is the standard way `@faremeter/middleware` integrates with facilitators.
/// Without this endpoint, servers using the middleware get 404 errors.
///
/// # Request format
/// Same shape as a 402 response body:
/// ```json
/// {
///   "x402Version": 1,
///   "accepts": [{ "scheme": "exact", "network": "base", "asset": "0x...", ... }],
///   "error": ""
/// }
/// ```
///
/// # Response format
/// Enriched requirements (only those the facilitator supports):
/// ```json
/// {
///   "x402Version": 1,
///   "accepts": [{ ...original fields, "extra": { "feePayer": "...", "tokens": [...] } }],
///   "error": ""
/// }
/// ```
#[instrument(skip_all)]
pub async fn post_accepts<A>(
    State(facilitator): State<A>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse
where
    A: Facilitator,
    A::Error: IntoResponse,
{
    let x402_version = body
        .get("x402Version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    let accepts = match body.get("accepts").and_then(|a| a.as_array()) {
        Some(a) => a,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "x402Version": x402_version,
                    "accepts": [],
                    "error": "Missing or invalid 'accepts' array"
                })),
            )
                .into_response();
        }
    };

    // Get facilitator's supported kinds
    let supported = match facilitator.supported().await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    // Build lookup: (scheme_str, network_str) -> extra
    // Includes both v1 network names ("base") and v2 CAIP-2 ("eip155:8453")
    let mut extra_lookup: HashMap<(String, String), Option<serde_json::Value>> = HashMap::new();
    for kind in &supported.kinds {
        let scheme_str = match serde_json::to_value(&kind.scheme) {
            Ok(serde_json::Value::String(s)) => s,
            _ => continue,
        };
        let extra_json = kind.extra.as_ref().and_then(|e| serde_json::to_value(e).ok());
        extra_lookup.insert((scheme_str, kind.network.clone()), extra_json);
    }

    // Match and enrich each merchant requirement
    let mut enriched = Vec::new();
    for req in accepts {
        let scheme = req.get("scheme").and_then(|s| s.as_str()).unwrap_or("");
        let network = req.get("network").and_then(|n| n.as_str()).unwrap_or("");
        let key = (scheme.to_string(), network.to_string());

        if let Some(facilitator_extra) = extra_lookup.get(&key) {
            let mut enriched_req = req.clone();

            // Merge facilitator's extra into the requirement's extra
            if let Some(fac_extra) = facilitator_extra {
                let req_extra = enriched_req
                    .get("extra")
                    .cloned()
                    .unwrap_or(json!({}));
                let mut merged = match req_extra {
                    serde_json::Value::Object(obj) => obj,
                    _ => serde_json::Map::new(),
                };

                // Add facilitator fields without overwriting merchant-provided ones
                if let serde_json::Value::Object(fac_obj) = fac_extra {
                    for (k, v) in fac_obj {
                        if !merged.contains_key(k) {
                            merged.insert(k.clone(), v.clone());
                        }
                    }
                }

                enriched_req["extra"] = serde_json::Value::Object(merged);
            }

            enriched.push(enriched_req);
        }
        // Requirements that don't match any supported kind are silently dropped
    }

    info!(
        requested = accepts.len(),
        matched = enriched.len(),
        "POST /accepts: matched {}/{} requirements",
        enriched.len(),
        accepts.len()
    );

    (
        StatusCode::OK,
        Json(json!({
            "x402Version": x402_version,
            "accepts": enriched,
            "error": ""
        })),
    )
        .into_response()
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
                    Ok(decoded_bytes) => match String::from_utf8(decoded_bytes) {
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
                    },
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
                error!(
                    "PAYMENT-SIGNATURE header contains invalid characters: {}",
                    e
                );
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

    // Check for special schemes BEFORE trying to parse as standard types
    // These schemes may have different payload structures that don't match standard x402 types
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(body_str) {
        // Detect scheme from paymentPayload.scheme (v1) or paymentPayload.accepted.scheme (v2)
        let scheme = json_value
            .get("paymentPayload")
            .and_then(|pp| {
                pp.get("scheme")
                    .and_then(|s| s.as_str())
                    .or_else(|| pp.get("accepted").and_then(|a| a.get("scheme")).and_then(|s| s.as_str()))
            });

        if scheme == Some("fhe-transfer") {
            info!("Detected fhe-transfer scheme, routing to Zama Lambda facilitator");

            match FHE_PROXY.verify(&json_value).await {
                Ok(fhe_response) => {
                    info!(
                        is_valid = fhe_response.is_valid,
                        "FHE verification complete"
                    );
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

        // Check for upto scheme (Permit2-based variable amount settlement)
        if scheme == Some("upto") {
            if !crate::upto::is_enabled() {
                warn!("Upto scheme verify requested but ENABLE_UPTO is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "isValid": false,
                        "invalidReason": "Upto scheme is disabled. Set ENABLE_UPTO=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected upto scheme, routing to Permit2 verification");

            match crate::upto::verify_upto(body_str, &facilitator).await {
                Ok(response) => {
                    info!("Upto verification complete");
                    return (StatusCode::OK, Json(response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Upto verification failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": format!("Upto verification error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }

        // Check for escrow scheme (x402r PaymentOperator)
        if scheme == Some(crate::payment_operator::ESCROW_SCHEME) {
            if !crate::payment_operator::is_enabled() {
                warn!("Escrow scheme verify requested but ENABLE_PAYMENT_OPERATOR is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "isValid": false,
                        "invalidReason": "Escrow scheme is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected escrow scheme, routing to PaymentOperator verification");

            match crate::payment_operator::verify_escrow(body_str, &facilitator).await {
                Ok(response) => {
                    info!("Escrow verification complete");
                    return (StatusCode::OK, Json(response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Escrow verification failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": format!("Escrow verification error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }

        // Also check for top-level scheme (direct escrow request format)
        let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
        if top_level_scheme == Some(crate::payment_operator::ESCROW_SCHEME) {
            if !crate::payment_operator::is_enabled() {
                warn!("Escrow scheme verify requested but ENABLE_PAYMENT_OPERATOR is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "isValid": false,
                        "invalidReason": "Escrow scheme is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected top-level escrow scheme, routing to PaymentOperator verification");

            match crate::payment_operator::verify_escrow(body_str, &facilitator).await {
                Ok(response) => {
                    info!("Escrow verification complete");
                    return (StatusCode::OK, Json(response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Escrow verification failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "isValid": false,
                            "invalidReason": format!("Escrow verification error: {}", e)
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
            debug!(
                "Processing x402 v2 verify request with CAIP-2 network: {}",
                req.network()
            );
            "v2"
        }
        VerifyRequestEnvelope::X402r(req) => {
            debug!(
                "Processing x402r verify request with CAIP-2 network: {}",
                req.network()
            );
            "x402r"
        }
        VerifyRequestEnvelope::X402rNested(req) => {
            debug!(
                "Processing x402r-nested verify request with CAIP-2 network: {}",
                req.network()
            );
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
                    Ok(decoded_bytes) => match String::from_utf8(decoded_bytes) {
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
                    },
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
                error!(
                    "PAYMENT-SIGNATURE header contains invalid characters: {}",
                    e
                );
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

    // Check for special schemes BEFORE trying to parse as standard types
    // These schemes may have different payload structures that don't match standard x402 types
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(body_str) {
        // Detect scheme from paymentPayload.scheme (v1) or paymentPayload.accepted.scheme (v2)
        let scheme = json_value
            .get("paymentPayload")
            .and_then(|pp| {
                pp.get("scheme")
                    .and_then(|s| s.as_str())
                    .or_else(|| pp.get("accepted").and_then(|a| a.get("scheme")).and_then(|s| s.as_str()))
            });

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

        // Check for upto scheme (Permit2-based variable amount settlement)
        if scheme == Some("upto") {
            if !crate::upto::is_enabled() {
                warn!("Upto scheme settle requested but ENABLE_UPTO is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "errorReason": "Upto scheme is disabled. Set ENABLE_UPTO=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected upto scheme, routing to Permit2 settlement");

            match crate::upto::settle_upto(body_str, &facilitator).await {
                Ok(upto_response) => {
                    info!("Upto settlement complete");
                    return (StatusCode::OK, Json(upto_response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Upto settlement failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": format!("Upto scheme error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }

        // Check for escrow scheme in nested paymentPayload.scheme (v2 format)
        // This mirrors the verify handler's nested escrow detection
        if scheme == Some(crate::payment_operator::ESCROW_SCHEME) {
            if !crate::payment_operator::is_enabled() {
                warn!("Escrow scheme settlement requested but ENABLE_PAYMENT_OPERATOR is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "errorReason": "Escrow scheme settlement is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected nested escrow scheme in paymentPayload, routing to PaymentOperator settlement");

            match crate::payment_operator::settle_escrow(body_str, &facilitator).await {
                Ok(escrow_response) => {
                    info!("Escrow scheme settlement complete");
                    return (StatusCode::OK, Json(escrow_response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Escrow scheme settlement failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": format!("Escrow scheme error: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }

        // Check for x402r escrow scheme (top-level scheme field)
        // This is the new v2 scheme pattern from x402r-scheme reference implementation
        let top_level_scheme = json_value.get("scheme").and_then(|s| s.as_str());
        if top_level_scheme == Some(crate::payment_operator::ESCROW_SCHEME) {
            // Check if escrow scheme (PaymentOperator) is enabled
            if !crate::payment_operator::is_enabled() {
                warn!("Escrow scheme settlement requested but ENABLE_PAYMENT_OPERATOR is not set to true");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "success": false,
                        "errorReason": "Escrow scheme settlement is disabled. Set ENABLE_PAYMENT_OPERATOR=true to enable."
                    })),
                )
                    .into_response();
            }

            info!("Detected escrow scheme, routing to PaymentOperator settlement");

            match crate::payment_operator::settle_escrow(body_str, &facilitator).await {
                Ok(escrow_response) => {
                    info!("Escrow scheme settlement complete");
                    return (StatusCode::OK, Json(escrow_response)).into_response();
                }
                Err(e) => {
                    error!(error = %e, "Escrow scheme settlement failed");
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "success": false,
                            "errorReason": format!("Escrow scheme error: {}", e)
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

            // Note: PaymentOperator now uses scheme="escrow" at top level, not extensions
            // The old operator extension pattern is deprecated
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
            debug!(
                "Processing x402 v2 settle request with CAIP-2 network: {}",
                req.network()
            );
            "v2"
        }
        SettleRequestEnvelope::X402r(req) => {
            debug!(
                "Processing x402r settle request with CAIP-2 network: {}",
                req.network()
            );
            "x402r"
        }
        SettleRequestEnvelope::X402rNested(req) => {
            debug!(
                "Processing x402r-nested settle request with CAIP-2 network: {}",
                req.network()
            );
            "x402r-nested"
        }
    };
    debug!("Processing x402 {} settle request", format_name);

    let body = match envelope.to_v1() {
        Ok(v1_req) => v1_req,
        Err(e) => {
            error!(
                "Failed to convert {} settle request to v1: {}",
                format_name, e
            );
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
            debug!(
                "  - authorization.validAfter: {} (type: UnixTimestamp u64 string, parsed to: {})",
                evm_payload.authorization.valid_after.seconds_since_epoch(),
                evm_payload.authorization.valid_after.seconds_since_epoch()
            );
            debug!(
                "  - authorization.validBefore: {} (type: UnixTimestamp u64 string, parsed to: {})",
                evm_payload.authorization.valid_before.seconds_since_epoch(),
                evm_payload.authorization.valid_before.seconds_since_epoch()
            );
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
                &near_payload.signed_delegate_action
                    [..near_payload.signed_delegate_action.len().min(100)]
            );
        }
        crate::types::ExactPaymentPayload::Stellar(stellar_payload) => {
            debug!("  - payload type: Stellar");
            debug!("  - from: {}", stellar_payload.from);
            debug!("  - to: {}", stellar_payload.to);
            debug!("  - amount: {}", stellar_payload.amount);
        }
        #[cfg(feature = "algorand")]
        crate::types::ExactPaymentPayload::Algorand(algorand_payload) => {
            debug!("  - payload type: Algorand (atomic group)");
            debug!("  - payment_index: {}", algorand_payload.payment_index);
            debug!(
                "  - payment_group.len: {}",
                algorand_payload.payment_group.len()
            );
        }
        #[cfg(feature = "sui")]
        crate::types::ExactPaymentPayload::Sui(sui_payload) => {
            debug!("  - payload type: Sui (sponsored transaction)");
            debug!("  - from: {}", sui_payload.from);
            debug!("  - to: {}", sui_payload.to);
            debug!("  - amount: {}", sui_payload.amount);
            debug!("  - coin_object_id: {}", sui_payload.coin_object_id);
        }
        crate::types::ExactPaymentPayload::SolanaSettlementAccount(sa_payload) => {
            debug!("  - payload type: Solana Settlement Account (Crossmint)");
            debug!("  - transaction_signature: {}", sa_payload.transaction_signature);
            debug!("  - settle_secret_key: {}", if sa_payload.settle_secret_key.is_some() { "provided" } else { "none" });
            debug!("  - settlement_rent_destination: {:?}", sa_payload.settlement_rent_destination);
        }
    }

    debug!("=== END SETTLE REQUEST DEBUG ===");

    // Proceed with normal settlement logic
    // Note: FHE transfers are handled early (before type deserialization) to support
    // custom FHE payload structures. See the fhe-transfer check above.
    info!(
        "Attempting to settle payment on network: {:?}, scheme: {:?}",
        body.payment_payload.network, body.payment_payload.scheme
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
// Escrow State Query Handler
// ============================================================================

/// `POST /escrow/state`: Query the on-chain state of an escrow payment.
///
/// Returns capturable amount, refundable amount, and whether payment has been collected.
/// This is a read-only view call (no gas consumed).
#[instrument(skip_all)]
pub async fn post_escrow_state<A>(
    State(facilitator): State<A>,
    raw_body: Bytes,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    let body_str = match std::str::from_utf8(&raw_body) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid UTF-8 in request body" })),
            )
                .into_response();
        }
    };

    match crate::payment_operator::query_escrow_state(body_str, &facilitator).await {
        Ok(state_response) => {
            info!("Escrow state query complete");
            (StatusCode::OK, Json(json!(state_response))).into_response()
        }
        Err(e) => {
            error!(error = %e, "Escrow state query failed");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Escrow state query failed: {}", e)
                })),
            )
                .into_response()
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
            "network": format!("string (e.g., '{}' or 'eip155:1')", networks.first().map(|s| s.as_str()).unwrap_or("ethereum")),
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
            "POST /register": "Register a new ERC-8004 agent (with optional recipient for delegation)",
            "POST /feedback": "Submit new feedback",
            "POST /feedback/revoke": "Revoke previously submitted feedback",
            "POST /feedback/response": "Append response to feedback (agent only)",
            "GET /reputation/:network/:agentId": "Get reputation summary for an agent",
            "GET /identity/:network/:agentId": "Get agent identity from Identity Registry",
            "GET /identity/:network/:agentId/metadata/:key": "Read specific agent metadata",
            "GET /identity/:network/total-supply": "Get total registered agents on a network"
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
pub async fn post_feedback<A>(State(facilitator): State<A>, raw_body: Bytes) -> impl IntoResponse
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

    let feedback = &request.feedback;
    let agent_id_str = parse_agent_id_value(&feedback.agent_id)
        .unwrap_or_else(|| feedback.agent_id.to_string());

    info!(
        network = %network,
        agent_id = %agent_id_str,
        value = feedback.value,
        value_decimals = feedback.value_decimals,
        tag1 = %feedback.tag1,
        "Processing ERC-8004 feedback submission"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();

    match provider_map.by_network(&network) {
        Some(NetworkProvider::Solana(p)) => {
            // ── Solana feedback via Anchor give_feedback ──
            let programs = match solana_erc8004::get_program_ids(&network) {
                Some(prog) => prog,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("No Solana ERC-8004 programs for network {}", network)),
                            network,
                        }),
                    ).into_response();
                }
            };

            let asset_pubkey = match solana_erc8004::parse_agent_id(&agent_id_str) {
                Ok(pk) => pk,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("{}", e)),
                            network,
                        }),
                    ).into_response();
                }
            };

            let fee_payer = p.keypair().pubkey();
            let feedback_hash_bytes: Option<[u8; 32]> = feedback.feedback_hash.map(|h| h.0);
            let score: Option<u8> = None; // Score is optional, not in FeedbackParams

            let ix = solana_erc8004::build_give_feedback_ix(
                &programs,
                &asset_pubkey,
                &fee_payer,
                feedback.value,
                feedback.value_decimals,
                score,
                &feedback.tag1,
                &feedback.tag2,
                &feedback.endpoint,
                &feedback.feedback_uri,
                feedback_hash_bytes,
            );

            match solana_erc8004::send_erc8004_transaction(
                p.rpc_client(), p.keypair(), vec![ix],
            ).await {
                Ok(sig) => {
                    info!(
                        network = %network,
                        tx = %sig,
                        agent_id = %agent_id_str,
                        "ERC-8004 Solana feedback submitted successfully"
                    );
                    (
                        StatusCode::OK,
                        Json(FeedbackResponse {
                            success: true,
                            transaction: Some(crate::types::TransactionHash::Solana(sig.into())),
                            feedback_index: None,
                            error: None,
                            network,
                        }),
                    ).into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana feedback transaction failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("Transaction failed: {}", e)),
                            network,
                        }),
                    ).into_response()
                }
            }
        }
        Some(NetworkProvider::Evm(provider)) => {
            // ── EVM feedback via IReputationRegistry.giveFeedback ──
            let contracts = match get_contracts(&network) {
                Some(c) => c,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("No ERC-8004 contracts for network {}", network)),
                            network,
                        }),
                    ).into_response();
                }
            };

            let agent_id_u64: u64 = match agent_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("Invalid EVM agent ID (expected numeric): {}", agent_id_str)),
                            network,
                        }),
                    ).into_response();
                }
            };

            let reputation_registry =
                IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

            let feedback_hash = feedback.feedback_hash.unwrap_or_default();

            let call = reputation_registry.giveFeedback(
                alloy::primitives::U256::from(agent_id_u64),
                feedback.value,
                feedback.value_decimals,
                feedback.tag1.clone(),
                feedback.tag2.clone(),
                feedback.endpoint.clone(),
                feedback.feedback_uri.clone(),
                feedback_hash,
            );

            // Legacy chains (SKALE) need explicit gasPrice to avoid EIP-1559 rejection
            let send_result = if !provider.is_eip1559() {
                let gp = provider.inner().get_gas_price().await.map_err(|e| format!("{e:?}"));
                match gp {
                    Ok(gas_price) => call.gas_price(gas_price).send().await,
                    Err(e) => {
                        error!(error = %e, "Failed to get gas price");
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("Failed to get gas price: {}", e)), network,
                        })).into_response();
                    }
                }
            } else {
                call.send().await
            };

            match send_result {
                Ok(pending_tx) => {
                    match pending_tx.get_receipt().await {
                        Ok(receipt) => {
                            let tx_hash = receipt.transaction_hash;
                            info!(
                                network = %network,
                                tx = %tx_hash,
                                agent_id = %agent_id_str,
                                "ERC-8004 feedback submitted successfully"
                            );
                            let feedback_index = None;
                            (
                                StatusCode::OK,
                                Json(FeedbackResponse {
                                    success: true,
                                    transaction: Some(crate::types::TransactionHash::Evm(tx_hash.0)),
                                    feedback_index,
                                    error: None,
                                    network,
                                }),
                            ).into_response()
                        }
                        Err(e) => {
                            error!(network = %network, error = %e, "Failed to get transaction receipt");
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(FeedbackResponse {
                                    success: false, transaction: None, feedback_index: None,
                                    error: Some(format!("Transaction failed: {}", e)),
                                    network,
                                }),
                            ).into_response()
                        }
                    }
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Failed to submit feedback transaction");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(FeedbackResponse {
                            success: false, transaction: None, feedback_index: None,
                            error: Some(format!("Failed to submit transaction: {}", e)),
                            network,
                        }),
                    ).into_response()
                }
            }
        }
        _ => {
            error!(network = %network, "No provider available for network");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FeedbackResponse {
                    success: false, transaction: None, feedback_index: None,
                    error: Some(format!("No provider available for network {}", network)),
                    network,
                }),
            ).into_response()
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

    let agent_id_str = parse_agent_id_value(&request.agent_id)
        .unwrap_or_else(|| request.agent_id.to_string());

    info!(
        network = %network,
        agent_id = %agent_id_str,
        feedback_index = request.feedback_index,
        "Revoking ERC-8004 feedback"
    );

    let provider_map = facilitator.provider_map();

    match provider_map.by_network(&network) {
        Some(NetworkProvider::Solana(p)) => {
            let programs = match solana_erc8004::get_program_ids(&network) {
                Some(prog) => prog,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("No Solana ERC-8004 programs for {}", network)
                    }))).into_response();
                }
            };

            let asset_pubkey = match solana_erc8004::parse_agent_id(&agent_id_str) {
                Ok(pk) => pk,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("{}", e)
                    }))).into_response();
                }
            };

            // Decode seal_hash from hex string (required for Solana)
            let seal_hash: [u8; 32] = match &request.seal_hash {
                Some(hex_str) => {
                    let bytes = hex::decode(hex_str.trim_start_matches("0x")).unwrap_or_default();
                    if bytes.len() != 32 {
                        return (StatusCode::BAD_REQUEST, Json(json!({
                            "success": false, "error": "sealHash must be 32 bytes (64 hex chars)"
                        }))).into_response();
                    }
                    bytes.try_into().unwrap()
                }
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": "sealHash is required for Solana revocations"
                    }))).into_response();
                }
            };

            let ix = solana_erc8004::build_revoke_feedback_ix(
                &programs, &asset_pubkey, request.feedback_index, seal_hash,
            );

            match solana_erc8004::send_erc8004_transaction(p.rpc_client(), p.keypair(), vec![ix]).await {
                Ok(sig) => {
                    info!(network = %network, tx = %sig, "ERC-8004 Solana feedback revoked");
                    (StatusCode::OK, Json(json!({
                        "success": true, "transaction": sig.to_string(), "network": network.to_string()
                    }))).into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana revoke failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                        "success": false, "error": format!("Transaction failed: {}", e)
                    }))).into_response()
                }
            }
        }
        Some(NetworkProvider::Evm(provider)) => {
            let contracts = match get_contracts(&network) {
                Some(c) => c,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("No ERC-8004 contracts for {}", network)
                    }))).into_response();
                }
            };

            let agent_id_u64: u64 = match agent_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("Invalid EVM agent ID: {}", agent_id_str)
                    }))).into_response();
                }
            };

            let reputation_registry =
                IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());

            let call = reputation_registry.revokeFeedback(
                alloy::primitives::U256::from(agent_id_u64),
                request.feedback_index,
            );

            // Legacy chains (SKALE) need explicit gasPrice
            let send_result = if !provider.is_eip1559() {
                match provider.inner().get_gas_price().await {
                    Ok(gas_price) => call.gas_price(gas_price).send().await,
                    Err(e) => {
                        error!(error = %e, "Failed to get gas price");
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                            "success": false, "error": format!("Failed to get gas price: {}", e)
                        }))).into_response();
                    }
                }
            } else {
                call.send().await
            };

            match send_result {
                Ok(pending_tx) => match pending_tx.get_receipt().await {
                    Ok(receipt) => {
                        let tx_hash = receipt.transaction_hash;
                        info!(network = %network, tx = %tx_hash, "ERC-8004 feedback revoked");
                        (StatusCode::OK, Json(json!({
                            "success": true,
                            "transaction": format!("0x{}", hex::encode(tx_hash.0)),
                            "network": network.to_string()
                        }))).into_response()
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get transaction receipt");
                        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                            "success": false, "error": format!("Transaction failed: {}", e)
                        }))).into_response()
                    }
                },
                Err(e) => {
                    error!(error = %e, "Failed to send revoke transaction");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                        "success": false, "error": format!("Failed to submit transaction: {}", e)
                    }))).into_response()
                }
            }
        }
        _ => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "success": false, "error": format!("No provider for network {}", network)
            }))).into_response()
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

    let agent_id_str = parse_agent_id_value(&request.agent_id)
        .unwrap_or_else(|| request.agent_id.to_string());

    info!(
        network = %network,
        agent_id = %agent_id_str,
        feedback_index = request.feedback_index,
        "Appending response to ERC-8004 feedback"
    );

    let provider_map = facilitator.provider_map();

    match provider_map.by_network(&network) {
        Some(NetworkProvider::Solana(p)) => {
            let programs = match solana_erc8004::get_program_ids(&network) {
                Some(prog) => prog,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("No Solana ERC-8004 programs for {}", network)
                    }))).into_response();
                }
            };

            let asset_pubkey = match solana_erc8004::parse_agent_id(&agent_id_str) {
                Ok(pk) => pk,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("{}", e)
                    }))).into_response();
                }
            };

            // Extract Solana client address
            let client_pubkey = match &request.client_address {
                MixedAddress::Solana(pk) => *pk,
                _ => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": "Client address must be a Solana pubkey for Solana networks"
                    }))).into_response();
                }
            };

            let response_hash_bytes: [u8; 32] = request.response_hash.map(|h| h.0).unwrap_or([0u8; 32]);

            // Decode seal_hash from hex string (required for Solana)
            let seal_hash: [u8; 32] = match &request.seal_hash {
                Some(hex_str) => {
                    let bytes = hex::decode(hex_str.trim_start_matches("0x")).unwrap_or_default();
                    if bytes.len() != 32 {
                        return (StatusCode::BAD_REQUEST, Json(json!({
                            "success": false, "error": "sealHash must be 32 bytes (64 hex chars)"
                        }))).into_response();
                    }
                    bytes.try_into().unwrap()
                }
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": "sealHash is required for Solana responses"
                    }))).into_response();
                }
            };

            let fee_payer_pubkey = p.keypair().pubkey();
            let ix = solana_erc8004::build_append_response_ix(
                &programs, &asset_pubkey, &client_pubkey, &fee_payer_pubkey,
                request.feedback_index, &request.response_uri, response_hash_bytes, seal_hash,
            );

            match solana_erc8004::send_erc8004_transaction(p.rpc_client(), p.keypair(), vec![ix]).await {
                Ok(sig) => {
                    info!(network = %network, tx = %sig, "ERC-8004 Solana response appended");
                    (StatusCode::OK, Json(json!({
                        "success": true, "transaction": sig.to_string(), "network": network.to_string()
                    }))).into_response()
                }
                Err(e) => {
                    error!(network = %network, error = %e, "Solana append response failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                        "success": false, "error": format!("Transaction failed: {}", e)
                    }))).into_response()
                }
            }
        }
        Some(NetworkProvider::Evm(provider)) => {
            let contracts = match get_contracts(&network) {
                Some(c) => c,
                None => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("No ERC-8004 contracts for {}", network)
                    }))).into_response();
                }
            };

            let client_addr = match &request.client_address {
                MixedAddress::Evm(addr) => addr.0,
                _ => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": "Client address must be an EVM address"
                    }))).into_response();
                }
            };

            let agent_id_u64: u64 = match agent_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, Json(json!({
                        "success": false, "error": format!("Invalid EVM agent ID: {}", agent_id_str)
                    }))).into_response();
                }
            };

            let reputation_registry =
                IReputationRegistry::new(contracts.reputation_registry, provider.inner().clone());
            let response_hash = request.response_hash.unwrap_or_default();

            let call = reputation_registry.appendResponse(
                alloy::primitives::U256::from(agent_id_u64),
                client_addr,
                request.feedback_index,
                request.response_uri.clone(),
                response_hash,
            );

            // Legacy chains (SKALE) need explicit gasPrice
            let send_result = if !provider.is_eip1559() {
                match provider.inner().get_gas_price().await {
                    Ok(gas_price) => call.gas_price(gas_price).send().await,
                    Err(e) => {
                        error!(error = %e, "Failed to get gas price");
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                            "success": false, "error": format!("Failed to get gas price: {}", e)
                        }))).into_response();
                    }
                }
            } else {
                call.send().await
            };

            match send_result {
                Ok(pending_tx) => match pending_tx.get_receipt().await {
                    Ok(receipt) => {
                        let tx_hash = receipt.transaction_hash;
                        info!(network = %network, tx = %tx_hash, "ERC-8004 response appended");
                        (StatusCode::OK, Json(json!({
                            "success": true,
                            "transaction": format!("0x{}", hex::encode(tx_hash.0)),
                            "network": network.to_string()
                        }))).into_response()
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to get transaction receipt");
                        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                            "success": false, "error": format!("Transaction failed: {}", e)
                        }))).into_response()
                    }
                },
                Err(e) => {
                    error!(error = %e, "Failed to send append response transaction");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                        "success": false, "error": format!("Failed to submit transaction: {}", e)
                    }))).into_response()
                }
            }
        }
        _ => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
                "success": false, "error": format!("No provider for network {}", network)
            }))).into_response()
        }
    }
}

/// Path parameters for reputation query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReputationPathParams {
    pub network: String,
    /// Agent ID: u64 for EVM, base58 pubkey for Solana
    pub agent_id: String,
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
    /// Comma-separated client addresses to filter by.
    /// If omitted, auto-discovers all clients via getClients().
    #[serde(default)]
    pub client_addresses: String,
}

/// `GET /reputation/:network/:agent_id`: Get reputation summary for an agent.
///
/// Returns the aggregated reputation summary from the ERC-8004 Reputation Registry.
///
/// # Query Parameters
/// - `tag1`: Filter by primary tag (optional)
/// - `tag2`: Filter by secondary tag (optional)
/// - `includeFeedback`: Include individual feedback entries (optional, default false)
/// - `clientAddresses`: Comma-separated client addresses to filter by (optional).
///   If omitted, auto-discovers all clients via `getClients()` on-chain call.
///
/// # Example
/// ```text
/// GET /reputation/base/42?includeFeedback=true
/// GET /reputation/base/42?clientAddresses=0xAAA,0xBBB&tag1=quality
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

    info!(
        network = %network,
        agent_id = %params.agent_id,
        tag1 = %query.tag1,
        tag2 = %query.tag2,
        "Querying ERC-8004 reputation"
    );

    // ---- Solana branch: read from ATOM Engine + AgentAccount ----
    if solana_erc8004::is_solana_erc8004_supported(&network) {
        let provider_map = facilitator.provider_map();
        let solana_provider = match provider_map.by_network(&network) {
            Some(NetworkProvider::Solana(p)) => p,
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana provider available for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let asset_pubkey = match solana_erc8004::parse_agent_id(&params.agent_id) {
            Ok(pk) => pk,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": format!("Invalid Solana agent ID: {}", e)
                    })),
                )
                    .into_response();
            }
        };

        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(p) => p,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana ERC-8004 program IDs for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let rpc = solana_provider.rpc_client();

        // Read AgentAccount for basic feedback counts (via SEAL digests)
        let agent_result =
            solana_erc8004::read_agent_account(rpc, &asset_pubkey, &programs.agent_registry).await;

        let feedback_count_from_agent = match &agent_result {
            Ok(agent) => agent.feedback_count,
            Err(solana_erc8004::SolanaErc8004Error::AccountNotFound(msg)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": msg })),
                )
                    .into_response();
            }
            Err(e) => {
                error!(error = %e, "Failed to read Solana agent account for reputation");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to query agent: {}", e)
                    })),
                )
                    .into_response();
            }
        };

        // Read ATOM Engine stats (may not exist if agent has no feedback yet)
        let atom_stats_response =
            match solana_erc8004::read_atom_stats(rpc, &asset_pubkey, &programs.atom_engine).await {
                Ok(stats) => Some(AtomStatsResponse {
                    trust_tier: stats.trust_tier,
                    trust_tier_name: solana_erc8004::trust_tier_name(stats.trust_tier).to_string(),
                    quality_score: stats.quality_score,
                    confidence: stats.confidence,
                    risk_score: stats.risk_score,
                    diversity_ratio: stats.diversity_ratio,
                    positive_count: stats.positive_count,
                    negative_count: stats.negative_count,
                    feedback_count: stats.feedback_count,
                    last_feedback_slot: stats.last_feedback_slot,
                }),
                Err(solana_erc8004::SolanaErc8004Error::AccountNotFound(_)) => {
                    // ATOM stats not initialized yet (agent has no feedback)
                    None
                }
                Err(e) => {
                    warn!(error = %e, "Failed to read ATOM stats, returning without ATOM data");
                    None
                }
            };

        // Build summary from ATOM stats or fall back to agent account counts
        let (count, summary_value) = if let Some(ref atom) = atom_stats_response {
            (atom.feedback_count as u64, atom.quality_score as i128)
        } else {
            (feedback_count_from_agent, 0i128)
        };

        return (
            StatusCode::OK,
            Json(json!({
                "agentId": params.agent_id,
                "summary": {
                    "count": count,
                    "summaryValue": summary_value,
                    "summaryValueDecimals": 0,
                    "network": network
                },
                "atomStats": atom_stats_response,
                "network": network
            })),
        )
            .into_response();
    }

    // ---- EVM branch: read from ERC-8004 Solidity contracts ----

    // Parse agent_id as u64 for EVM
    let agent_id: u64 = match params.agent_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid EVM agent ID (expected numeric): {}", params.agent_id)
                })),
            )
                .into_response();
        }
    };

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

    let agent_id_u256 = alloy::primitives::U256::from(agent_id);

    // Resolve client addresses: parse from query param or auto-discover via getClients()
    let client_addresses: Vec<alloy::primitives::Address> = if query.client_addresses.is_empty() {
        // Auto-discover all clients who have given feedback to this agent
        match reputation_registry.getClients(agent_id_u256).call().await {
            Ok(clients) => {
                info!(
                    agent_id = agent_id,
                    client_count = clients.len(),
                    "Auto-discovered clients for reputation query"
                );
                clients
            }
            Err(e) => {
                info!(
                    agent_id = agent_id,
                    error = %e,
                    "No clients found for agent (may have no feedback yet)"
                );
                // Return zero summary - agent has no reputation data
                let summary = ReputationSummary {
                    agent_id,
                    count: 0,
                    summary_value: 0,
                    summary_value_decimals: 0,
                    network: network.clone(),
                };
                let response = ReputationResponse {
                    agent_id,
                    summary,
                    feedback: if query.include_feedback {
                        Some(vec![])
                    } else {
                        None
                    },
                    atom_stats: None,
                    network,
                };
                return (StatusCode::OK, Json(response)).into_response();
            }
        }
    } else {
        // Parse comma-separated addresses from query param
        let parsed: Vec<alloy::primitives::Address> = query
            .client_addresses
            .split(',')
            .filter_map(|s| s.trim().parse::<alloy::primitives::Address>().ok())
            .collect();
        if parsed.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid clientAddresses parameter: no valid addresses found"
                })),
            )
                .into_response();
        }
        parsed
    };

    // If getClients returned empty (agent exists but has no feedback), return zero summary
    if client_addresses.is_empty() {
        let summary = ReputationSummary {
            agent_id,
            count: 0,
            summary_value: 0,
            summary_value_decimals: 0,
            network: network.clone(),
        };
        let response = ReputationResponse {
            agent_id,
            summary,
            feedback: if query.include_feedback {
                Some(vec![])
            } else {
                None
            },
            atom_stats: None,
            network,
        };
        return (StatusCode::OK, Json(response)).into_response();
    }

    // Call getSummary with resolved client addresses
    let summary_call = reputation_registry.getSummary(
        agent_id_u256,
        client_addresses.clone(),
        query.tag1.clone(),
        query.tag2.clone(),
    );

    match summary_call.call().await {
        Ok(result) => {
            let summary = ReputationSummary {
                agent_id,
                count: result.count,
                summary_value: result.summaryValue,
                summary_value_decimals: result.summaryValueDecimals,
                network: network.clone(),
            };

            // Optionally fetch individual feedback entries
            let feedback_entries: Option<Vec<FeedbackEntry>> = if query.include_feedback {
                let feedback_call = reputation_registry.readAllFeedback(
                    agent_id_u256,
                    client_addresses,
                    query.tag1.clone(),
                    query.tag2.clone(),
                    false, // Don't include revoked
                );

                match feedback_call.call().await {
                    Ok(fb_result) => {
                        let entries: Vec<FeedbackEntry> = fb_result
                            .clients
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
                agent_id,
                summary,
                feedback: feedback_entries,
                atom_stats: None, // EVM has no ATOM Engine
                network,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(
                network = %network,
                agent_id = agent_id,
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
    /// Agent ID: u64 for EVM, base58 pubkey for Solana
    pub agent_id: String,
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

    info!(
        network = %network,
        agent_id = %params.agent_id,
        "Querying ERC-8004 agent identity"
    );

    // ---- Solana branch: read from 8004-solana Anchor program ----
    if solana_erc8004::is_solana_erc8004_supported(&network) {
        let provider_map = facilitator.provider_map();
        let solana_provider = match provider_map.by_network(&network) {
            Some(NetworkProvider::Solana(p)) => p,
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana provider available for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let asset_pubkey = match solana_erc8004::parse_agent_id(&params.agent_id) {
            Ok(pk) => pk,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": format!("Invalid Solana agent ID: {}", e)
                    })),
                )
                    .into_response();
            }
        };

        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(p) => p,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana ERC-8004 program IDs for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let rpc = solana_provider.rpc_client();
        match solana_erc8004::read_agent_account(rpc, &asset_pubkey, &programs.agent_registry).await
        {
            Ok(agent) => {
                let owner_pubkey = solana_erc8004::bytes_to_pubkey(&agent.owner);
                return (
                    StatusCode::OK,
                    Json(json!({
                        "agentId": params.agent_id,
                        "owner": owner_pubkey.to_string(),
                        "agentUri": agent.agent_uri,
                        "nftName": agent.nft_name,
                        "agentWallet": null,
                        "feedbackCount": agent.feedback_count,
                        "responseCount": agent.response_count,
                        "revokeCount": agent.revoke_count,
                        "network": network
                    })),
                )
                    .into_response();
            }
            Err(solana_erc8004::SolanaErc8004Error::AccountNotFound(msg)) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": msg
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                error!(error = %e, "Failed to read Solana agent account");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to query Solana agent: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    // ---- EVM branch: read from ERC-8004 Solidity contracts ----

    // Parse agent_id as u64 for EVM
    let agent_id: u64 = match params.agent_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid EVM agent ID (expected numeric): {}", params.agent_id)
                })),
            )
                .into_response();
        }
    };

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

    let agent_id_u256 = alloy::primitives::U256::from(agent_id);

    // Query owner, URI, and wallet in parallel.
    // We skip exists() because it's not part of standard ERC-721 and may not be
    // implemented on all proxy contracts. Instead, ownerOf() reverts for
    // non-existent tokens, which we catch below as a 404.
    let owner_call = identity_registry.ownerOf(agent_id_u256);
    let uri_call = identity_registry.tokenURI(agent_id_u256);
    let wallet_call = identity_registry.getAgentWallet(agent_id_u256);

    let (owner_result, uri_result, wallet_result) =
        tokio::join!(owner_call.call(), uri_call.call(), wallet_call.call());

    // ownerOf reverts for non-existent tokens (ERC-721 standard behavior)
    let owner = match owner_result {
        Ok(o) => MixedAddress::Evm(crate::types::EvmAddress(o)),
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("revert") || err_str.contains("ERC721") {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": format!("Agent {} not found in Identity Registry on {}", agent_id, network)
                    })),
                )
                    .into_response();
            }
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
        agent_id,
        owner,
        agent_uri,
        agent_wallet,
        network,
    };

    (StatusCode::OK, Json(identity)).into_response()
}

// ============================================================================
// ERC-8004 Agent Registration Endpoints
// ============================================================================

/// `GET /register`: Returns a machine-readable description of the `/register` endpoint.
#[instrument(skip_all)]
pub async fn get_register_info() -> impl IntoResponse {
    Json(json!({
        "endpoint": "/register",
        "description": "POST to register a new ERC-8004 agent on-chain",
        "extension": "8004-reputation",
        "supportedNetworks": supported_network_names(),
        "body": {
            "x402Version": "string - protocol version (1)",
            "network": "string - target network (e.g., 'base-mainnet', 'ethereum')",
            "agentUri": "string - URI pointing to agent registration file (IPFS, HTTPS)",
            "metadata": "array (optional) - key-value metadata entries [{key, value}]",
            "recipient": "string (optional) - address to receive the agent NFT. If omitted, the facilitator retains ownership."
        },
        "response": {
            "success": "boolean",
            "agentId": "number - the newly assigned agent ID (ERC-721 tokenId)",
            "transaction": "string - registration transaction hash",
            "transferTransaction": "string (optional) - transfer transaction hash if recipient was specified",
            "owner": "string - current owner of the agent NFT",
            "network": "string"
        },
        "notes": {
            "gasless": "The facilitator pays all gas fees for registration and transfer",
            "transferBehavior": "When recipient is specified, the facilitator mints the NFT then transfers it via ERC-721 safeTransferFrom. The agentWallet is cleared on transfer and must be re-set by the new owner.",
            "relatedEndpoints": {
                "GET /identity/:network/:agentId": "Read agent identity",
                "GET /identity/:network/:agentId/metadata/:key": "Read agent metadata",
                "GET /identity/:network/total-supply": "Get total registered agents",
                "POST /feedback": "Submit reputation feedback"
            }
        }
    }))
}

/// `POST /register`: Register a new ERC-8004 agent on-chain.
///
/// The facilitator pays gas for the registration transaction. If a `recipient`
/// address is provided, the NFT is minted to the facilitator and then transferred
/// to the recipient via ERC-721 `safeTransferFrom`.
#[instrument(skip_all, fields(network, agent_uri))]
pub async fn post_register<A>(State(facilitator): State<A>, raw_body: Bytes) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
    // Parse request body
    let request: RegisterAgentRequest = match serde_json::from_slice(&raw_body) {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to parse register request: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("Invalid request format: {}", e)),
                    network: crate::network::Network::Ethereum,
                }),
            )
                .into_response();
        }
    };

    let network = request.network;

    // Validate network supports ERC-8004
    if !is_erc8004_supported(&network) {
        let supported = supported_network_names();
        warn!(network = %network, "ERC-8004 registration not supported on this network");
        return (
            StatusCode::BAD_REQUEST,
            Json(RegisterAgentResponse {
                success: false,
                agent_id: None,
                transaction: None,
                transfer_transaction: None,
                owner: None,
                error: Some(format!(
                    "ERC-8004 is not supported on network {}. Supported networks: {:?}",
                    network, supported
                )),
                network,
            }),
        )
            .into_response();
    }

    info!(
        network = %network,
        agent_uri = %request.agent_uri,
        has_recipient = request.recipient.is_some(),
        "Processing ERC-8004 agent registration"
    );

    // Get the provider for this network
    let provider_map = facilitator.provider_map();

    // ── Solana registration via Anchor register() ──
    if let Some(NetworkProvider::Solana(p)) = provider_map.by_network(&network) {
        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(prog) => prog,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(RegisterAgentResponse {
                        success: false, agent_id: None, transaction: None,
                        transfer_transaction: None, owner: None,
                        error: Some(format!("No Solana ERC-8004 programs for {}", network)),
                        network,
                    }),
                ).into_response();
            }
        };

        // Read the collection pubkey from on-chain config
        let collection = match solana_erc8004::read_collection_pubkey(
            p.rpc_client(), &programs.agent_registry,
        ).await {
            Ok(c) => c,
            Err(e) => {
                error!(network = %network, error = %e, "Failed to read collection pubkey");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(RegisterAgentResponse {
                        success: false, agent_id: None, transaction: None,
                        transfer_transaction: None, owner: None,
                        error: Some(format!("Failed to read registry config: {}", e)),
                        network,
                    }),
                ).into_response();
            }
        };

        // Generate a new keypair for the NFT asset
        let asset_keypair = solana_sdk::signature::Keypair::new();
        let asset_pubkey = asset_keypair.pubkey();
        let fee_payer = p.keypair();

        let ix = solana_erc8004::build_register_ix(
            &programs, &collection, &asset_pubkey, &fee_payer.pubkey(), &request.agent_uri,
        );

        // Register requires both fee_payer and asset keypairs to sign
        match solana_erc8004::send_erc8004_transaction_with_signers(
            p.rpc_client(), fee_payer, &[fee_payer, &asset_keypair], vec![ix],
        ).await {
            Ok(sig) => {
                let agent_id = asset_pubkey.to_string();
                info!(
                    network = %network,
                    tx = %sig,
                    agent_id = %agent_id,
                    "ERC-8004 Solana agent registered successfully"
                );

                // Set metadata if provided
                if let Some(ref metadata) = request.metadata {
                    for entry in metadata {
                        let ix = solana_erc8004::build_set_metadata_pda_ix(
                            &programs, &asset_pubkey, &fee_payer.pubkey(),
                            &entry.key, entry.value.as_bytes(), false,
                        );
                        if let Err(e) = solana_erc8004::send_erc8004_transaction(
                            p.rpc_client(), fee_payer, vec![ix],
                        ).await {
                            warn!(
                                key = %entry.key, error = %e,
                                "Failed to set metadata (agent registered successfully)"
                            );
                        }
                    }
                }

                return (
                    StatusCode::OK,
                    Json(RegisterAgentResponse {
                        success: true,
                        agent_id: Some(agent_id),
                        transaction: Some(crate::types::TransactionHash::Solana(sig.into())),
                        transfer_transaction: None,
                        owner: Some(MixedAddress::Solana(fee_payer.pubkey())),
                        error: None,
                        network,
                    }),
                ).into_response();
            }
            Err(e) => {
                error!(network = %network, error = %e, "Solana registration failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(RegisterAgentResponse {
                        success: false, agent_id: None, transaction: None,
                        transfer_transaction: None, owner: None,
                        error: Some(format!("Registration failed: {}", e)),
                        network,
                    }),
                ).into_response();
            }
        }
    }

    // ── EVM registration via IIdentityRegistry.register() ──
    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(RegisterAgentResponse {
                    success: false, agent_id: None, transaction: None,
                    transfer_transaction: None, owner: None,
                    error: Some(format!("No ERC-8004 contracts for network {}", network)),
                    network,
                }),
            ).into_response();
        }
    };

    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            error!(network = %network, "No EVM provider available for network");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RegisterAgentResponse {
                    success: false, agent_id: None, transaction: None,
                    transfer_transaction: None, owner: None,
                    error: Some(format!("No EVM provider available for network {}", network)),
                    network,
                }),
            ).into_response();
        }
    };

    // Create Identity Registry contract instance
    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());

    // Build the registration call based on provided parameters
    let agent_uri = request.agent_uri.clone();
    let has_metadata = request.metadata.as_ref().map_or(false, |m| !m.is_empty());

    // Legacy chains (SKALE) need explicit gasPrice to avoid EIP-1559 rejection
    let legacy_gas_price = if !provider.is_eip1559() {
        match provider.inner().get_gas_price().await {
            Ok(gp) => Some(gp),
            Err(e) => {
                error!(error = %e, "Failed to get gas price for legacy chain");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(RegisterAgentResponse {
                        success: false, agent_id: None, transaction: None,
                        transfer_transaction: None, owner: None,
                        error: Some(format!("Failed to get gas price: {}", e)),
                        network,
                    }),
                ).into_response();
            }
        }
    } else {
        None
    };

    let register_result = if has_metadata {
        // Convert metadata params to contract MetadataEntry structs
        let metadata_entries: Vec<MetadataEntry> = request
            .metadata
            .unwrap_or_default()
            .into_iter()
            .map(|m| MetadataEntry {
                metadataKey: m.key,
                metadataValue: hex::decode(m.value.trim_start_matches("0x"))
                    .unwrap_or_else(|_| m.value.as_bytes().to_vec())
                    .into(),
            })
            .collect();

        info!(
            metadata_count = metadata_entries.len(),
            "Registering agent with URI and metadata"
        );

        // register_0 = register(string, MetadataEntry[]) - first overload in ABI
        let call = identity_registry.register_0(agent_uri, metadata_entries);
        if let Some(gp) = legacy_gas_price {
            call.gas_price(gp).send().await
        } else {
            call.send().await
        }
    } else if !request.agent_uri.is_empty() {
        info!("Registering agent with URI only");
        // register_1 = register(string) - second overload in ABI
        let call = identity_registry.register_1(agent_uri);
        if let Some(gp) = legacy_gas_price {
            call.gas_price(gp).send().await
        } else {
            call.send().await
        }
    } else {
        info!("Registering agent without URI or metadata");
        // register_2 = register() - third overload in ABI
        let call = identity_registry.register_2();
        if let Some(gp) = legacy_gas_price {
            call.gas_price(gp).send().await
        } else {
            call.send().await
        }
    };

    // Handle registration transaction
    let pending_tx = match register_result {
        Ok(tx) => tx,
        Err(e) => {
            error!(network = %network, error = %e, "Failed to send registration transaction");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("Failed to send registration transaction: {}", e)),
                    network,
                }),
            )
                .into_response();
        }
    };

    // Wait for receipt
    let receipt = match pending_tx.get_receipt().await {
        Ok(r) => r,
        Err(e) => {
            error!(network = %network, error = %e, "Failed to get registration receipt");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RegisterAgentResponse {
                    success: false,
                    agent_id: None,
                    transaction: None,
                    transfer_transaction: None,
                    owner: None,
                    error: Some(format!("Registration transaction failed: {}", e)),
                    network,
                }),
            )
                .into_response();
        }
    };

    let reg_tx_hash = receipt.transaction_hash;
    info!(network = %network, tx = %reg_tx_hash, "Registration transaction confirmed");

    // Parse Registered event from logs to get agentId
    let agent_id_num: Option<u64> = receipt.inner.logs().iter().find_map(|log| {
        log.log_decode::<IIdentityRegistry::Registered>()
            .ok()
            .map(|event| {
                let id: u64 = event.inner.data.agentId.try_into().unwrap_or(0);
                info!(agent_id = id, "Parsed agentId from Registered event");
                id
            })
    });

    let agent_id = match agent_id_num {
        Some(id) => id,
        None => {
            warn!("Could not parse agentId from Registered event logs, querying totalSupply");
            match identity_registry.totalSupply().call().await {
                Ok(supply) => {
                    let id: u64 = supply.try_into().unwrap_or(0);
                    id
                }
                Err(e) => {
                    error!(error = %e, "Failed to query totalSupply as fallback");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RegisterAgentResponse {
                            success: true,
                            agent_id: None,
                            transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                            transfer_transaction: None,
                            owner: None,
                            error: Some(
                                "Registration succeeded but failed to determine agentId"
                                    .to_string(),
                            ),
                            network,
                        }),
                    )
                        .into_response();
                }
            }
        }
    };
    let agent_id_str = agent_id.to_string();

    // Determine final owner - get the facilitator wallet address
    let facilitator_mixed = provider.signer_address();
    let facilitator_address = match &facilitator_mixed {
        MixedAddress::Evm(addr) => addr.0,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RegisterAgentResponse {
                    success: true,
                    agent_id: Some(agent_id_str.clone()),
                    transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                    transfer_transaction: None,
                    owner: None,
                    error: Some("Unexpected non-EVM signer address".to_string()),
                    network,
                }),
            )
                .into_response();
        }
    };
    let mut final_owner = facilitator_mixed;
    let mut transfer_tx: Option<crate::types::TransactionHash> = None;

    // If recipient is specified, transfer the NFT
    if let Some(ref recipient) = request.recipient {
        let recipient_address = match recipient {
            MixedAddress::Evm(addr) => addr.0,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(RegisterAgentResponse {
                        success: true,
                        agent_id: Some(agent_id_str.clone()),
                        transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                        transfer_transaction: None,
                        owner: Some(final_owner),
                        error: Some(
                            "Recipient must be an EVM address for ERC-8004 registration"
                                .to_string(),
                        ),
                        network,
                    }),
                )
                    .into_response();
            }
        };

        info!(
            agent_id = agent_id,
            from = %facilitator_address,
            to = %recipient_address,
            "Transferring agent NFT to recipient"
        );

        let transfer_call = identity_registry.safeTransferFrom(
            facilitator_address,
            recipient_address,
            alloy::primitives::U256::from(agent_id),
        );

        // Legacy chains (SKALE) need explicit gasPrice for transfer too
        let transfer_send = if !provider.is_eip1559() {
            match provider.inner().get_gas_price().await {
                Ok(gp) => transfer_call.gas_price(gp).send().await,
                Err(e) => {
                    error!(error = %e, "Failed to get gas price for transfer");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RegisterAgentResponse {
                            success: true,
                            agent_id: Some(agent_id_str.clone()),
                            transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                            transfer_transaction: None,
                            owner: Some(final_owner),
                            error: Some(format!("Failed to get gas price for transfer: {}", e)),
                            network,
                        }),
                    ).into_response();
                }
            }
        } else {
            transfer_call.send().await
        };

        match transfer_send {
            Ok(pending) => match pending.get_receipt().await {
                Ok(transfer_receipt) => {
                    let transfer_hash = transfer_receipt.transaction_hash;
                    info!(
                        network = %network,
                        tx = %transfer_hash,
                        agent_id = agent_id,
                        recipient = %recipient_address,
                        "Agent NFT transferred successfully"
                    );
                    transfer_tx = Some(crate::types::TransactionHash::Evm(transfer_hash.0));
                    final_owner = recipient.clone();
                }
                Err(e) => {
                    error!(error = %e, "Transfer receipt failed - agent registered but NOT transferred");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RegisterAgentResponse {
                            success: true,
                            agent_id: Some(agent_id_str.clone()),
                            transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                            transfer_transaction: None,
                            owner: Some(final_owner),
                            error: Some(format!(
                                "Agent registered (id={}) but transfer failed: {}",
                                agent_id_str, e
                            )),
                            network,
                        }),
                    )
                        .into_response();
                }
            },
            Err(e) => {
                error!(error = %e, "Failed to send transfer transaction");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(RegisterAgentResponse {
                        success: true,
                        agent_id: Some(agent_id_str.clone()),
                        transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
                        transfer_transaction: None,
                        owner: Some(final_owner),
                        error: Some(format!(
                            "Agent registered (id={}) but transfer failed: {}",
                            agent_id_str, e
                        )),
                        network,
                    }),
                )
                    .into_response();
            }
        }
    }

    info!(
        network = %network,
        agent_id = agent_id,
        owner = %final_owner,
        "ERC-8004 agent registration complete"
    );

    (
        StatusCode::OK,
        Json(RegisterAgentResponse {
            success: true,
            agent_id: Some(agent_id_str),
            transaction: Some(crate::types::TransactionHash::Evm(reg_tx_hash.0)),
            transfer_transaction: transfer_tx,
            owner: Some(final_owner),
            error: None,
            network,
        }),
    )
        .into_response()
}

// ============================================================================
// ERC-8004 Extended Identity Read Endpoints
// ============================================================================

/// Path parameters for metadata query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IdentityMetadataPathParams {
    pub network: String,
    /// Agent ID: u64 for EVM, base58 pubkey for Solana
    pub agent_id: String,
    pub key: String,
}

/// `GET /identity/:network/:agent_id/metadata/:key`: Read specific metadata from an agent.
#[instrument(skip_all, fields(network, agent_id, key))]
pub async fn get_identity_metadata<A>(
    State(facilitator): State<A>,
    Path(params): Path<IdentityMetadataPathParams>,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
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

    // ---- Solana branch: read from MetadataEntryPda ----
    if solana_erc8004::is_solana_erc8004_supported(&network) {
        let provider_map = facilitator.provider_map();
        let solana_provider = match provider_map.by_network(&network) {
            Some(NetworkProvider::Solana(p)) => p,
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana provider available for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let asset_pubkey = match solana_erc8004::parse_agent_id(&params.agent_id) {
            Ok(pk) => pk,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": format!("Invalid Solana agent ID: {}", e)
                    })),
                )
                    .into_response();
            }
        };

        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(p) => p,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana ERC-8004 program IDs for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        // Derive the MetadataEntryPda
        let (metadata_pda, _bump) = solana_erc8004::derive_metadata_pda(
            &asset_pubkey,
            &params.key,
            &programs.agent_registry,
        );

        let rpc = solana_provider.rpc_client();
        match rpc.get_account_data(&metadata_pda).await {
            Ok(data) => {
                // Skip 8-byte Anchor discriminator, then deserialize
                if data.len() < 8 {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({
                            "error": format!("Metadata key '{}' not set for agent {} on {}", params.key, params.agent_id, network)
                        })),
                    )
                        .into_response();
                }

                // Try to extract value as UTF-8 (metadata_value is Vec<u8> in the PDA)
                // Account layout after discriminator: asset(32) + metadata_key(string) + metadata_value(vec<u8>) + immutable(bool) + bump(u8)
                // For simplicity, return the raw data hex-encoded
                let hex_value = format!("0x{}", hex::encode(&data[8..]));
                let utf8_value = String::from_utf8(data[8..].to_vec()).ok();

                return (
                    StatusCode::OK,
                    Json(json!({
                        "agentId": params.agent_id,
                        "key": params.key,
                        "value": hex_value,
                        "valueUtf8": utf8_value,
                        "network": network
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("AccountNotFound")
                    || err_str.contains("could not find account")
                {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({
                            "error": format!("Metadata key '{}' not set for agent {} on {}", params.key, params.agent_id, network)
                        })),
                    )
                        .into_response();
                }
                error!(
                    network = %network,
                    agent_id = %params.agent_id,
                    key = %params.key,
                    error = %e,
                    "Failed to query Solana metadata"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to query metadata: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    // ---- EVM branch: read from ERC-8004 Solidity contracts ----

    // Parse agent_id as u64 for EVM
    let agent_id: u64 = match params.agent_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid EVM agent ID (expected numeric): {}", params.agent_id)
                })),
            )
                .into_response();
        }
    };

    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("No ERC-8004 contracts for network {}", network) })),
            )
                .into_response();
        }
    };

    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("No EVM provider available for network {}", network) })),
            )
                .into_response();
        }
    };

    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());
    let agent_id_u256 = alloy::primitives::U256::from(agent_id);

    // Query metadata directly (skip exists() which may not be implemented on all proxies)
    match identity_registry
        .getMetadata(agent_id_u256, params.key.clone())
        .call()
        .await
    {
        Ok(value) => {
            let hex_value = format!("0x{}", hex::encode(&value));
            let utf8_value = String::from_utf8(value.to_vec()).ok();

            (
                StatusCode::OK,
                Json(json!({
                    "agentId": agent_id,
                    "key": params.key,
                    "value": hex_value,
                    "valueUtf8": utf8_value,
                    "network": network
                })),
            )
                .into_response()
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("revert") || err_str.contains("ERC721") {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": format!("Agent {} not found or metadata key '{}' not set on {}", agent_id, params.key, network)
                    })),
                )
                    .into_response();
            }
            error!(
                network = %network,
                agent_id = agent_id,
                key = %params.key,
                error = %e,
                "Failed to query metadata"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to query metadata: {}", e)
                })),
            )
                .into_response()
        }
    }
}

/// Path parameters for total supply query
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TotalSupplyPathParams {
    pub network: String,
}

/// `GET /identity/:network/total-supply`: Get total number of registered agents on a network.
#[instrument(skip_all, fields(network))]
pub async fn get_identity_total_supply<A>(
    State(facilitator): State<A>,
    Path(params): Path<TotalSupplyPathParams>,
) -> impl IntoResponse
where
    A: Facilitator + HasProviderMap,
    A::Error: IntoResponse,
    A::Map: ProviderMap<Value = NetworkProvider>,
{
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

    // ---- Solana branch: read from RegistryConfig PDA ----
    if solana_erc8004::is_solana_erc8004_supported(&network) {
        let provider_map = facilitator.provider_map();
        let solana_provider = match provider_map.by_network(&network) {
            Some(NetworkProvider::Solana(p)) => p,
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana provider available for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let programs = match solana_erc8004::get_program_ids(&network) {
            Some(p) => p,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("No Solana ERC-8004 program IDs for network {}", network)
                    })),
                )
                    .into_response();
            }
        };

        let rpc = solana_provider.rpc_client();
        match solana_erc8004::read_registry_config(rpc, &programs.agent_registry).await {
            Ok(config) => {
                let total = config.base_index as u64;
                info!(network = %network, total_supply = total, "Queried Solana registry total supply");
                return (
                    StatusCode::OK,
                    Json(json!({
                        "totalSupply": total,
                        "network": network
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                error!(network = %network, error = %e, "Failed to query Solana registry config");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to query total supply: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    // ---- EVM branch: read from ERC-8004 Solidity contracts ----

    let contracts = match get_contracts(&network) {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("No ERC-8004 contracts for network {}", network) })),
            )
                .into_response();
        }
    };

    let provider_map = facilitator.provider_map();
    let provider = match provider_map.by_network(&network) {
        Some(NetworkProvider::Evm(p)) => p,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("No EVM provider available for network {}", network) })),
            )
                .into_response();
        }
    };

    let identity_registry =
        IIdentityRegistry::new(contracts.identity_registry, provider.inner().clone());

    match identity_registry.totalSupply().call().await {
        Ok(supply) => {
            let total: u64 = supply.try_into().unwrap_or(0);
            info!(network = %network, total_supply = total, "Queried identity total supply");
            (
                StatusCode::OK,
                Json(json!({
                    "totalSupply": total,
                    "network": network
                })),
            )
                .into_response()
        }
        Err(e) => {
            let error_str = format!("{}", e);
            // Empty revert data ("0x") means the function selector doesn't exist
            // on the current proxy implementation (ERC721Enumerable may have been removed)
            if error_str.contains("execution reverted") {
                warn!(
                    network = %network,
                    error = %e,
                    "totalSupply() not available on this contract version"
                );
                (
                    StatusCode::NOT_IMPLEMENTED,
                    Json(json!({
                        "error": "totalSupply() is not available on the current contract implementation",
                        "network": network,
                        "hint": "The Identity Registry may have been upgraded without ERC721Enumerable support"
                    })),
                )
                    .into_response()
            } else {
                error!(network = %network, error = %e, "Failed to query total supply");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": format!("Failed to query total supply: {}", e)
                    })),
                )
                    .into_response()
            }
        }
    }
}
